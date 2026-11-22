// SPDX-License-Identifier: MIT
//! A PE image as the discoverer sees it: sections mapped at their virtual addresses
//! (zero-filled beyond the raw data) plus the import address table.

use rc_pe::{ImportName, Pe};
use std::collections::{BTreeMap, BTreeSet};

const IMAGE_SCN_CNT_CODE: u32 = 0x20;

pub struct Section {
    pub name: String,
    pub va: u32,
    pub size: u32,
    pub exec: bool,
    pub data: Vec<u8>,
}

pub struct Image {
    pub base: u32,
    pub entry: u32,
    pub sections: Vec<Section>,
    /// IAT slot VA → "dll!name" (lower-case DLL).
    pub imports: BTreeMap<u32, String>,
    /// Exported functions (VA, name).
    pub exports: Vec<(u32, String)>,
    pub is_dll: bool,
    /// VAs of the 32-bit fixup sites (empty for the executables, which have no relocations).
    pub relocs: BTreeSet<u32>,
    /// The base the image was linked for (differs from `base` after [`Image::load`] rebased it).
    pub preferred_base: u32,
}

impl Image {
    pub fn new(file: &[u8], pe: &Pe) -> Self {
        let base = pe.image_base;
        let mut sections: Vec<Section> = pe
            .sections
            .iter()
            .map(|s| {
                let size = s.mapped_size();
                let mut data = vec![0u8; size as usize];
                let n = (s.raw_size.min(size)) as usize;
                let off = s.raw_offset as usize;
                if let Some(src) = file.get(off..off.saturating_add(n).min(file.len())) {
                    data[..src.len()].copy_from_slice(src);
                }
                Section {
                    name: s.name.clone(),
                    va: base.wrapping_add(s.virtual_address),
                    size,
                    exec: s.is_executable() || s.characteristics & IMAGE_SCN_CNT_CODE != 0,
                    data,
                }
            })
            .collect();
        // GOG's rewritten `Cultures2.exe` marks every section as plain data; Windows runs it
        // anyway. Without any code section, the one holding the entry point is code.
        if !sections.iter().any(|s| s.exec) {
            let entry = pe.entry_va();
            if let Some(s) = sections
                .iter_mut()
                .find(|s| entry >= s.va && entry - s.va < s.size)
            {
                s.exec = true;
            }
        }
        let mut imports = BTreeMap::new();
        for d in &pe.imports {
            let dll = d.dll.to_ascii_lowercase();
            for f in &d.functions {
                let name = match &f.name {
                    ImportName::Name { name, .. } => name.clone(),
                    ImportName::Ordinal(o) => format!("#{o}"),
                };
                imports.insert(base.wrapping_add(f.iat_rva), format!("{dll}!{name}"));
            }
        }
        let exports = pe
            .exports
            .iter()
            .map(|e| {
                (
                    base.wrapping_add(e.rva),
                    e.name.clone().unwrap_or_else(|| format!("#{}", e.ordinal)),
                )
            })
            .collect();
        Image {
            base,
            entry: pe.entry_va(),
            sections,
            imports,
            exports,
            is_dll: pe.is_dll(),
            relocs: pe.relocs.iter().map(|&r| base.wrapping_add(r)).collect(),
            preferred_base: base,
        }
    }

    /// Map the image and, if `base` differs from the preferred base, rebase it: every fixup
    /// site gets the delta added, and all addresses (sections, IAT, exports, entry) move.
    pub fn load(file: &[u8], pe: &Pe, base: Option<u32>) -> Result<Self, String> {
        let mut img = Image::new(file, pe);
        if let Some(new) = base {
            img.rebase(new)?;
        }
        Ok(img)
    }

    /// Move the image to `new`: add the delta at every fixup site and shift all addresses.
    pub fn rebase(&mut self, new: u32) -> Result<(), String> {
        if new == self.base {
            return Ok(());
        }
        if self.relocs.is_empty() {
            return Err(format!(
                "cannot rebase to {new:#x}: the image has no relocations"
            ));
        }
        let delta = new.wrapping_sub(self.base);
        let sites: Vec<u32> = self.relocs.iter().copied().collect();
        for site in sites {
            let v = self
                .dword(site)
                .ok_or_else(|| format!("fixup at {site:#x} outside the sections"))?;
            self.write_dword(site, v.wrapping_add(delta));
        }
        let mv = |va: u32| va.wrapping_add(delta);
        self.base = new;
        self.entry = mv(self.entry);
        for s in &mut self.sections {
            s.va = mv(s.va);
        }
        self.imports = std::mem::take(&mut self.imports)
            .into_iter()
            .map(|(k, v)| (mv(k), v))
            .collect();
        for e in &mut self.exports {
            e.0 = mv(e.0);
        }
        self.relocs = self.relocs.iter().map(|&r| mv(r)).collect();
        Ok(())
    }

    fn write_dword(&mut self, va: u32, v: u32) {
        if let Some(s) = self
            .sections
            .iter_mut()
            .find(|s| va >= s.va && va - s.va < s.size)
        {
            let o = (va - s.va) as usize;
            if let Some(d) = s.data.get_mut(o..o + 4) {
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
    }

    /// Whether `va` lies inside the mapped image (any section).
    pub fn contains(&self, va: u32) -> bool {
        self.section_of(va).is_some()
    }

    pub fn section_of(&self, va: u32) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| va >= s.va && va - s.va < s.size)
    }

    pub fn is_exec(&self, va: u32) -> bool {
        self.section_of(va).is_some_and(|s| s.exec)
    }

    /// Bytes from `va` to the end of its section.
    pub fn tail(&self, va: u32) -> Option<&[u8]> {
        let s = self.section_of(va)?;
        Some(&s.data[(va - s.va) as usize..])
    }

    pub fn byte(&self, va: u32) -> Option<u8> {
        self.tail(va).and_then(|t| t.first().copied())
    }

    pub fn dword(&self, va: u32) -> Option<u32> {
        let t = self.tail(va)?;
        Some(u32::from_le_bytes(t.get(..4)?.try_into().ok()?))
    }

    /// Replace the bytes `from` at `va` by `to` (same length; a game project's code patch,
    /// applied before discovery). An error if the image does not hold `from` there.
    pub fn patch(&mut self, va: u32, from: &[u8], to: &[u8]) -> Result<(), String> {
        if from.len() != to.len() {
            return Err(format!("patch at {va:#010x}: lengths differ"));
        }
        let s = self
            .sections
            .iter_mut()
            .find(|s| va >= s.va && va - s.va < s.data.len() as u32)
            .ok_or_else(|| format!("patch at {va:#010x}: outside the image"))?;
        let at = (va - s.va) as usize;
        let here = s
            .data
            .get_mut(at..at + from.len())
            .ok_or_else(|| format!("patch at {va:#010x}: outside its section"))?;
        if here != from {
            return Err(format!("patch at {va:#010x}: the image holds other bytes"));
        }
        here.copy_from_slice(to);
        Ok(())
    }
}
