// SPDX-License-Identifier: MIT
//! The process: the executable (restored from its packing) and the lifted DLLs mapped into
//! guest memory, their imports bound to reserved addresses, the main thread's stack, TIB and
//! PEB. As the sister projects' `process.c`.

use crate::cpu::{Cpu, ESP};
use crate::imports;
use crate::mem::Mem;

/// Process data the runtime writes for the guest (command line, environment).
pub const PROC_DATA: u32 = 0x0001_0000;
pub const PROC_DATA_SIZE: u32 = 0x0001_0000;
/// The main thread's stack.
pub const STACK_LO: u32 = 0x0011_0000;
pub const STACK_SIZE: u32 = 0x0010_0000;
pub const TIB_ADDR: u32 = 0x7ffd_e000;
pub const PEB_ADDR: u32 = 0x7ffd_f000;
/// Where the entry point returns to: the runtime ends the process there.
pub const EXIT_RETURN: u32 = 0xefff_fff8;

/// A module mapped into guest memory.
#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub base: u32,
    pub size: u32,
    pub entry: u32,
    pub exports: Vec<(Option<String>, u32, u32)>,
}

#[derive(Debug)]
pub enum Error {
    Pe(String),
    Unpack(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Pe(s) => write!(f, "{s}"),
            Error::Unpack(s) => write!(f, "{s}"),
        }
    }
}

/// The image of a file as the loader maps it: a packed executable is restored first.
pub fn image_of(file: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, Error> {
    if rc_unpack::detect(file).is_some() {
        return rc_unpack::unpack(file)
            .map(std::borrow::Cow::Owned)
            .map_err(|e| Error::Unpack(e.to_string()));
    }
    Ok(std::borrow::Cow::Borrowed(file))
}

/// Map `image` at `base` (its preferred base if `None`), apply its relocations if it moved,
/// and bind its imports: to a lifted module's export if `modules` has one, else to the
/// reserved import address.
pub fn map(
    m: Mem,
    name: &str,
    image: &[u8],
    base: Option<u32>,
    modules: &[Module],
) -> Result<Module, Error> {
    let pe = rc_pe::parse(image).map_err(|e| Error::Pe(format!("{name}: {e}")))?;
    let base = base.unwrap_or(pe.image_base);
    // Headers, then each section's file bytes (to the end of its last page, as Windows maps).
    let headers = pe
        .sections
        .iter()
        .map(|s| s.raw_offset)
        .filter(|&o| o > 0)
        .min()
        .unwrap_or(0x400) as usize;
    m.write(base, &image[..headers.min(image.len())]);
    for s in &pe.sections {
        let start = s.raw_offset as usize;
        let len = s.raw_size as usize;
        let end = (start + len).min(image.len());
        if start < end {
            m.write(base.wrapping_add(s.virtual_address), &image[start..end]);
        }
    }
    let delta = base.wrapping_sub(pe.image_base);
    if delta != 0 {
        for &rva in &pe.relocs {
            let a = base.wrapping_add(rva);
            m.st32(a, m.ld32(a).wrapping_add(delta));
        }
    }
    for dll in &pe.imports {
        let lifted = modules
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(&dll.dll));
        for f in &dll.functions {
            let fname = match &f.name {
                rc_pe::ImportName::Name { name, .. } => name.clone(),
                rc_pe::ImportName::Ordinal(o) => format!("#{o}"),
            };
            let target = lifted
                .and_then(|lm| export_address(lm, &fname))
                .unwrap_or_else(|| imports::address(&dll.dll, &fname));
            m.st32(base.wrapping_add(f.iat_rva), target);
        }
    }
    Ok(Module {
        name: name.to_string(),
        base,
        size: pe.size_of_image,
        entry: base.wrapping_add(pe.entry_rva),
        exports: pe
            .exports
            .iter()
            .map(|e| (e.name.clone(), e.ordinal, base.wrapping_add(e.rva)))
            .collect(),
    })
}

/// A lifted module's export by name or `#ordinal`.
pub fn export_address(m: &Module, name: &str) -> Option<u32> {
    if let Some(o) = name.strip_prefix('#').and_then(|o| o.parse::<u32>().ok()) {
        return m.exports.iter().find(|e| e.1 == o).map(|e| e.2);
    }
    m.exports
        .iter()
        .find(|e| e.0.as_deref() == Some(name))
        .map(|e| e.2)
}

/// The main thread: stack, TIB (`fs`), PEB, and the return address that ends the process.
pub fn main_thread(c: &mut Cpu, image_base: u32) {
    let m = c.mem();
    c.r[ESP] = STACK_LO + STACK_SIZE - 16;
    m.st32(TIB_ADDR, 0xffff_ffff); // no exception handler
    m.st32(TIB_ADDR + 0x04, STACK_LO + STACK_SIZE); // stack base
    m.st32(TIB_ADDR + 0x08, STACK_LO); // stack limit
    m.st32(TIB_ADDR + 0x18, TIB_ADDR); // self
    m.st32(TIB_ADDR + 0x20, 1); // process id
    m.st32(TIB_ADDR + 0x24, 1); // thread id
    m.st32(TIB_ADDR + 0x30, PEB_ADDR);
    m.st32(PEB_ADDR + 0x08, image_base);
    c.fs_base = TIB_ADDR;
    c.push32(EXIT_RETURN);
}

static MODULES: std::sync::RwLock<Vec<Module>> = std::sync::RwLock::new(Vec::new());

/// Remember a mapped module (for `LoadLibraryA`, `GetModuleHandleA`, `GetProcAddress`).
pub fn add_module(m: Module) {
    MODULES.write().expect("modules").push(m);
}

/// The mapped modules, in load order.
pub fn modules() -> Vec<Module> {
    MODULES.read().expect("modules").clone()
}

/// The base of a mapped module by file name (case-insensitive).
pub fn module_base(name: &str) -> Option<u32> {
    MODULES
        .read()
        .expect("modules")
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .map(|m| m.base)
}

/// An export of the mapped module at `base`.
pub fn module_export(base: u32, name: &str) -> Option<u32> {
    MODULES
        .read()
        .expect("modules")
        .iter()
        .find(|m| m.base == base)
        .and_then(|m| export_address(m, name))
}
