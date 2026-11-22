// SPDX-License-Identifier: MIT
//! Minimal, bounds-checked PE32 (i386) parser.
//!
//! It reads exactly what the recompiler needs: headers, sections, imports (by name and by
//! ordinal), exports, the relocation directory and the Rich header. It never panics on
//! malformed input. Every read is checked and reported as [`Error`].

use std::fmt;

pub const IMAGE_FILE_MACHINE_I386: u16 = 0x014c;
pub const IMAGE_FILE_RELOCS_STRIPPED: u16 = 0x0001;
pub const IMAGE_FILE_DLL: u16 = 0x2000;
pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;

const DIR_EXPORT: usize = 0;
const DIR_IMPORT: usize = 1;
const DIR_BASERELOC: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read went past the end of the file.
    Truncated {
        offset: usize,
        len: usize,
    },
    NotMz,
    NotPe,
    /// Not a PE32 optional header (PE32+ or something else).
    BadOptionalMagic(u16),
    UnsupportedMachine(u16),
    /// An RVA that no section (or the headers) covers.
    UnmappedRva(u32),
    /// A string that is not NUL-terminated inside the file.
    BadString(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { offset, len } => {
                write!(f, "truncated: {len} bytes at offset {offset:#x}")
            }
            Error::NotMz => write!(f, "no MZ signature"),
            Error::NotPe => write!(f, "no PE signature"),
            Error::BadOptionalMagic(m) => write!(f, "not a PE32 image (optional magic {m:#x})"),
            Error::UnsupportedMachine(m) => write!(f, "unsupported machine {m:#x}"),
            Error::UnmappedRva(r) => write!(f, "rva {r:#x} not inside any section"),
            Error::BadString(r) => write!(f, "unterminated string at rva {r:#x}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
    /// The image's section alignment (the page size the loader maps sections in).
    pub alignment: u32,
}

impl Section {
    pub fn is_executable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_EXECUTE != 0
    }
    /// Size the section occupies in memory (MSVC sometimes leaves `virtual_size` zero). Windows
    /// maps whole pages, so the file's bytes after `virtual_size` up to the end of its last page
    /// are there too (1C's patch to the Russian Crazy School keeps code and a table there).
    pub fn mapped_size(&self) -> u32 {
        if self.virtual_size == 0 {
            return self.raw_size;
        }
        let page = self.alignment.max(1);
        let page_end = self.virtual_size.div_ceil(page).saturating_mul(page);
        self.virtual_size.max(self.raw_size.min(page_end))
    }
    pub fn contains_rva(&self, rva: u32) -> bool {
        rva >= self.virtual_address && rva - self.virtual_address < self.mapped_size()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportName {
    Name { hint: u16, name: String },
    Ordinal(u16),
}

impl fmt::Display for ImportName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportName::Name { name, .. } => f.write_str(name),
            ImportName::Ordinal(o) => write!(f, "#{o}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub name: ImportName,
    /// RVA of this function's IAT slot. The game calls through `[image_base + iat_rva]`.
    pub iat_rva: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDll {
    pub dll: String,
    pub functions: Vec<Import>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub ordinal: u32,
    pub name: Option<String>,
    pub rva: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RichEntry {
    pub product: u16,
    pub build: u16,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pe {
    pub machine: u16,
    pub timestamp: u32,
    pub characteristics: u16,
    pub linker_version: (u8, u8),
    pub entry_rva: u32,
    pub image_base: u32,
    pub size_of_image: u32,
    pub subsystem: u16,
    pub sections: Vec<Section>,
    pub imports: Vec<ImportDll>,
    pub exports: Vec<Export>,
    /// A base relocation directory is present and non-empty.
    pub has_reloc_dir: bool,
    /// RVAs of the `IMAGE_REL_BASED_HIGHLOW` fixups (32-bit absolute addresses), sorted.
    /// Other fixup types do not occur in PE32 i386 images and are ignored, except
    /// `ABSOLUTE` padding entries, which are skipped.
    pub relocs: Vec<u32>,
    pub rich: Option<Vec<RichEntry>>,
}

impl Pe {
    pub fn is_dll(&self) -> bool {
        self.characteristics & IMAGE_FILE_DLL != 0
    }
    pub fn relocs_stripped(&self) -> bool {
        self.characteristics & IMAGE_FILE_RELOCS_STRIPPED != 0 || !self.has_reloc_dir
    }
    pub fn entry_va(&self) -> u32 {
        self.image_base.wrapping_add(self.entry_rva)
    }
    pub fn section_by_name(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }
    pub fn section_for_rva(&self, rva: u32) -> Option<&Section> {
        self.sections.iter().find(|s| s.contains_rva(rva))
    }
    pub fn import_count(&self) -> usize {
        self.imports.iter().map(|d| d.functions.len()).sum()
    }
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn bytes(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.data.get(offset..end))
            .ok_or(Error::Truncated { offset, len })
    }
    fn u8(&self, o: usize) -> Result<u8> {
        Ok(self.bytes(o, 1)?[0])
    }
    fn u16(&self, o: usize) -> Result<u16> {
        let b = self.bytes(o, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, o: usize) -> Result<u32> {
        let b = self.bytes(o, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Parse a PE32 i386 image from its file bytes.
pub fn parse(data: &[u8]) -> Result<Pe> {
    let r = Reader { data };
    if r.bytes(0, 2)? != b"MZ" {
        return Err(Error::NotMz);
    }
    let pe_off = r.u32(0x3c)? as usize;
    if r.bytes(pe_off, 4)? != b"PE\0\0" {
        return Err(Error::NotPe);
    }
    let coff = pe_off + 4;
    let machine = r.u16(coff)?;
    if machine != IMAGE_FILE_MACHINE_I386 {
        return Err(Error::UnsupportedMachine(machine));
    }
    let nsections = r.u16(coff + 2)? as usize;
    let timestamp = r.u32(coff + 4)?;
    let opt_size = r.u16(coff + 16)? as usize;
    let characteristics = r.u16(coff + 18)?;

    let opt = coff + 20;
    let magic = r.u16(opt)?;
    if magic != 0x10b {
        return Err(Error::BadOptionalMagic(magic));
    }
    let linker_version = (r.u8(opt + 2)?, r.u8(opt + 3)?);
    let entry_rva = r.u32(opt + 16)?;
    let image_base = r.u32(opt + 28)?;
    let size_of_image = r.u32(opt + 56)?;
    let size_of_headers = r.u32(opt + 60)?;
    let subsystem = r.u16(opt + 68)?;
    let ndirs = (r.u32(opt + 92)? as usize).min(16);
    let mut dirs = [(0u32, 0u32); 16];
    for (i, d) in dirs.iter_mut().enumerate().take(ndirs) {
        *d = (r.u32(opt + 96 + 8 * i)?, r.u32(opt + 100 + 8 * i)?);
    }

    let section_alignment = r.u32(opt + 32)?;
    let sec_table = opt + opt_size;
    let mut sections = Vec::with_capacity(nsections);
    for i in 0..nsections {
        let o = sec_table + 40 * i;
        let raw_name = r.bytes(o, 8)?;
        let end = raw_name.iter().position(|&c| c == 0).unwrap_or(8);
        sections.push(Section {
            name: String::from_utf8_lossy(&raw_name[..end]).into_owned(),
            virtual_size: r.u32(o + 8)?,
            virtual_address: r.u32(o + 12)?,
            raw_size: r.u32(o + 16)?,
            raw_offset: r.u32(o + 20)?,
            characteristics: r.u32(o + 36)?,
            alignment: section_alignment,
        });
    }

    let img = Image {
        r: &r,
        sections: &sections,
        size_of_headers,
    };
    let imports = if dirs[DIR_IMPORT].1 != 0 {
        img.imports(dirs[DIR_IMPORT].0)?
    } else {
        Vec::new()
    };
    let exports = if dirs[DIR_EXPORT].1 != 0 {
        img.exports(dirs[DIR_EXPORT].0)?
    } else {
        Vec::new()
    };

    let relocs = if dirs[DIR_BASERELOC].1 != 0 {
        img.relocs(dirs[DIR_BASERELOC].0, dirs[DIR_BASERELOC].1)?
    } else {
        Vec::new()
    };

    Ok(Pe {
        machine,
        timestamp,
        characteristics,
        linker_version,
        entry_rva,
        image_base,
        size_of_image,
        subsystem,
        sections,
        imports,
        exports,
        has_reloc_dir: dirs[DIR_BASERELOC].0 != 0 && dirs[DIR_BASERELOC].1 != 0,
        relocs,
        rich: parse_rich(data, pe_off),
    })
}

struct Image<'a, 'b> {
    r: &'b Reader<'a>,
    sections: &'b [Section],
    size_of_headers: u32,
}

impl Image<'_, '_> {
    fn off(&self, rva: u32) -> Result<usize> {
        if rva < self.size_of_headers {
            return Ok(rva as usize);
        }
        self.sections
            .iter()
            .find(|s| s.contains_rva(rva))
            .map(|s| (rva - s.virtual_address + s.raw_offset) as usize)
            .ok_or(Error::UnmappedRva(rva))
    }
    fn u32(&self, rva: u32) -> Result<u32> {
        self.r.u32(self.off(rva)?)
    }
    fn u16(&self, rva: u32) -> Result<u16> {
        self.r.u16(self.off(rva)?)
    }
    fn cstr(&self, rva: u32) -> Result<String> {
        let o = self.off(rva)?;
        let tail = self.r.data.get(o..).ok_or(Error::BadString(rva))?;
        let end = tail
            .iter()
            .take(4096)
            .position(|&c| c == 0)
            .ok_or(Error::BadString(rva))?;
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    fn imports(&self, dir_rva: u32) -> Result<Vec<ImportDll>> {
        let mut out = Vec::new();
        for i in 0u32.. {
            let d = dir_rva + 20 * i;
            let (ilt, name, iat) = (self.u32(d)?, self.u32(d + 12)?, self.u32(d + 16)?);
            if name == 0 && iat == 0 {
                break;
            }
            let dll = self.cstr(name)?;
            // Prefer the import lookup table; bound or stripped images only have the IAT.
            let table = if ilt != 0 { ilt } else { iat };
            let mut functions = Vec::new();
            for j in 0u32.. {
                let v = self.u32(table + 4 * j)?;
                if v == 0 {
                    break;
                }
                let name = if v & 0x8000_0000 != 0 {
                    ImportName::Ordinal((v & 0xffff) as u16)
                } else {
                    ImportName::Name {
                        hint: self.u16(v)?,
                        name: self.cstr(v + 2)?,
                    }
                };
                functions.push(Import {
                    name,
                    iat_rva: iat + 4 * j,
                });
            }
            out.push(ImportDll { dll, functions });
        }
        Ok(out)
    }

    /// Walk the base relocation blocks (page RVA, block size, 16-bit entries).
    fn relocs(&self, dir_rva: u32, dir_size: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut o = 0u32;
        while o + 8 <= dir_size {
            let page = self.u32(dir_rva + o)?;
            let size = self.u32(dir_rva + o + 4)?;
            if size < 8 {
                break;
            }
            for i in 0..(size - 8) / 2 {
                let e = self.u16(dir_rva + o + 8 + 2 * i)?;
                if e >> 12 == 3 {
                    out.push(page.wrapping_add(u32::from(e & 0x0fff)));
                }
            }
            o = o.saturating_add(size);
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    fn exports(&self, dir_rva: u32) -> Result<Vec<Export>> {
        let base = self.u32(dir_rva + 16)?;
        let nfuncs = self.u32(dir_rva + 20)?;
        let nnames = self.u32(dir_rva + 24)?;
        let funcs = self.u32(dir_rva + 28)?;
        let names = self.u32(dir_rva + 32)?;
        let ords = self.u32(dir_rva + 36)?;
        let mut out: Vec<Export> = (0..nfuncs)
            .map(|i| {
                Ok(Export {
                    ordinal: base + i,
                    name: None,
                    rva: self.u32(funcs + 4 * i)?,
                })
            })
            .collect::<Result<_>>()?;
        for i in 0..nnames {
            let idx = self.u16(ords + 2 * i)? as usize;
            let name = self.cstr(self.u32(names + 4 * i)?)?;
            if let Some(e) = out.get_mut(idx) {
                e.name = Some(name);
            }
        }
        out.retain(|e| e.rva != 0);
        Ok(out)
    }
}

/// Decode the undocumented MSVC "Rich" header between the DOS stub and the PE header.
fn parse_rich(data: &[u8], pe_off: usize) -> Option<Vec<RichEntry>> {
    let head = data.get(..pe_off)?;
    let rich = head.windows(4).rposition(|w| w == b"Rich")?;
    let key = u32::from_le_bytes(data.get(rich + 4..rich + 8)?.try_into().ok()?);
    let marker = (0x536e_6144u32 ^ key).to_le_bytes(); // "DanS" xor key
    let start = head[..rich].windows(4).position(|w| w == marker)?;
    let mut entries = Vec::new();
    let mut o = start + 16; // "DanS" + three zero dwords (xored)
    while o + 8 <= rich {
        let id = u32::from_le_bytes(data[o..o + 4].try_into().ok()?) ^ key;
        let count = u32::from_le_bytes(data[o + 4..o + 8].try_into().ok()?) ^ key;
        entries.push(RichEntry {
            product: (id >> 16) as u16,
            build: (id & 0xffff) as u16,
            count,
        });
        o += 8;
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small but valid PE32 image: `.text` with a `ret`, `.idata` with two imports
    /// (one by name, one by ordinal), and `.edata` with one named export.
    fn build_pe(with_rich: bool) -> Vec<u8> {
        let mut f = vec![0u8; 0x600];
        f[0..2].copy_from_slice(b"MZ");
        let pe_off = 0x80usize;
        f[0x3c..0x40].copy_from_slice(&(pe_off as u32).to_le_bytes());
        if with_rich {
            let key = 0x1234_5678u32;
            let mut o = 0x40;
            for v in [0x536e_6144u32 ^ key, key, key, key] {
                f[o..o + 4].copy_from_slice(&v.to_le_bytes());
                o += 4;
            }
            let id = (0x000bu32 << 16) | 8966; // product 11, build 8966
            f[o..o + 4].copy_from_slice(&(id ^ key).to_le_bytes());
            f[o + 4..o + 8].copy_from_slice(&(42 ^ key).to_le_bytes());
            o += 8;
            f[o..o + 4].copy_from_slice(b"Rich");
            f[o + 4..o + 8].copy_from_slice(&key.to_le_bytes());
        }
        let w16 = |f: &mut Vec<u8>, o: usize, v: u16| f[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |f: &mut Vec<u8>, o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());

        f[pe_off..pe_off + 4].copy_from_slice(b"PE\0\0");
        let coff = pe_off + 4;
        w16(&mut f, coff, IMAGE_FILE_MACHINE_I386);
        w16(&mut f, coff + 2, 3);
        w32(&mut f, coff + 4, 1_000_000_000);
        w16(&mut f, coff + 16, 224);
        w16(&mut f, coff + 18, 0x010f); // relocs stripped, executable
        let opt = coff + 20;
        w16(&mut f, opt, 0x10b);
        f[opt + 2] = 6;
        f[opt + 3] = 0;
        w32(&mut f, opt + 16, 0x1000);
        w32(&mut f, opt + 28, 0x0040_0000);
        w32(&mut f, opt + 56, 0x4000);
        w32(&mut f, opt + 60, 0x200);
        w16(&mut f, opt + 68, 2);
        w32(&mut f, opt + 92, 16);
        w32(&mut f, opt + 96, 0x3000); // export dir
        w32(&mut f, opt + 100, 0x40);
        w32(&mut f, opt + 104, 0x2000); // import dir
        w32(&mut f, opt + 108, 0x28);

        let mut sec = |i: usize, name: &[u8], va: u32, raw: u32, ch: u32| {
            let o = opt + 224 + 40 * i;
            f[o..o + name.len()].copy_from_slice(name);
            w32(&mut f, o + 8, 0x100);
            w32(&mut f, o + 12, va);
            w32(&mut f, o + 16, 0x200);
            w32(&mut f, o + 20, raw);
            w32(&mut f, o + 36, ch);
        };
        sec(0, b".text", 0x1000, 0x200, 0x6000_0020);
        sec(1, b".idata", 0x2000, 0x400, 0xc000_0040);
        sec(2, b".edata", 0x3000, 0x400 + 0x100, 0x4000_0040);
        f[0x200] = 0xc3;

        // .idata at file 0x400 / rva 0x2000
        let rva = |r: u32| 0x400 + (r - 0x2000) as usize;
        w32(&mut f, rva(0x2000), 0x2040); // ILT
        w32(&mut f, rva(0x200c), 0x2070); // name
        w32(&mut f, rva(0x2010), 0x2050); // IAT
        w32(&mut f, rva(0x2040), 0x2080); // by name
        w32(&mut f, rva(0x2044), 0x8000_0005); // by ordinal
        w32(&mut f, rva(0x2050), 0x2080);
        w32(&mut f, rva(0x2054), 0x8000_0005);
        f[rva(0x2070)..rva(0x2070) + 13].copy_from_slice(b"KERNEL32.dll\0");
        w16(&mut f, rva(0x2080), 7);
        f[rva(0x2082)..rva(0x2082) + 12].copy_from_slice(b"ExitProcess\0");

        // .edata overlaps raw 0x500 / rva 0x3000 (raw_size 0x200 spans it).
        let e = |r: u32| 0x500 + (r - 0x3000) as usize;
        w32(&mut f, e(0x3010), 1); // ordinal base
        w32(&mut f, e(0x3014), 1); // functions
        w32(&mut f, e(0x3018), 1); // names
        w32(&mut f, e(0x301c), 0x3028);
        w32(&mut f, e(0x3020), 0x302c);
        w32(&mut f, e(0x3024), 0x3030);
        w32(&mut f, e(0x3028), 0x1000);
        w32(&mut f, e(0x302c), 0x3034);
        w16(&mut f, e(0x3030), 0);
        f[e(0x3034)..e(0x3034) + 7].copy_from_slice(b"DoWork\0");
        f
    }

    #[test]
    fn parses_base_relocations() {
        let mut f = build_pe(false);
        let opt = 0x80 + 4 + 20;
        // Data directory 5 (base relocations) → rva 0x3080 (inside .edata, file 0x580).
        f[opt + 136..opt + 140].copy_from_slice(&0x3080u32.to_le_bytes());
        f[opt + 140..opt + 144].copy_from_slice(&16u32.to_le_bytes());
        let o = 0x580;
        f[o..o + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        f[o + 4..o + 8].copy_from_slice(&16u32.to_le_bytes());
        for (i, e) in [0x3010u16, 0x0000, 0x3004, 0xa008].iter().enumerate() {
            f[o + 8 + 2 * i..o + 10 + 2 * i].copy_from_slice(&e.to_le_bytes());
        }
        let pe = parse(&f).expect("valid image");
        assert!(pe.has_reloc_dir);
        assert_eq!(pe.relocs, vec![0x1004, 0x1010]);
        assert!(parse(&build_pe(false)).unwrap().relocs.is_empty());
    }

    #[test]
    fn parses_headers_sections_imports_exports() {
        let pe = parse(&build_pe(true)).expect("valid image");
        assert_eq!(pe.image_base, 0x40_0000);
        assert_eq!(pe.entry_va(), 0x40_1000);
        assert_eq!(pe.linker_version, (6, 0));
        assert!(pe.relocs_stripped());
        assert!(!pe.is_dll());
        assert_eq!(
            pe.sections
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            [".text", ".idata", ".edata"]
        );
        assert!(pe.section_by_name(".text").unwrap().is_executable());
        assert_eq!(pe.section_for_rva(0x2010).unwrap().name, ".idata");

        assert_eq!(pe.imports.len(), 1);
        let k = &pe.imports[0];
        assert_eq!(k.dll, "KERNEL32.dll");
        assert_eq!(
            k.functions[0].name,
            ImportName::Name {
                hint: 7,
                name: "ExitProcess".into()
            }
        );
        assert_eq!(k.functions[0].iat_rva, 0x2050);
        assert_eq!(k.functions[1].name, ImportName::Ordinal(5));
        assert_eq!(k.functions[1].iat_rva, 0x2054);
        assert_eq!(k.functions[1].name.to_string(), "#5");

        assert_eq!(
            pe.exports,
            vec![Export {
                ordinal: 1,
                name: Some("DoWork".into()),
                rva: 0x1000
            }]
        );
        assert_eq!(
            pe.rich,
            Some(vec![RichEntry {
                product: 11,
                build: 8966,
                count: 42
            }])
        );
    }

    #[test]
    fn no_rich_header() {
        assert_eq!(parse(&build_pe(false)).unwrap().rich, None);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert_eq!(parse(b""), Err(Error::Truncated { offset: 0, len: 2 }));
        assert_eq!(parse(b"ZM\0\0"), Err(Error::NotMz));
        let mut f = build_pe(false);
        f[0x80] = b'X';
        assert_eq!(parse(&f), Err(Error::NotPe));
        // Every truncation of a valid image must yield an error, never a panic.
        let full = build_pe(true);
        for len in 0..full.len() {
            let _ = parse(&full[..len]);
        }
    }
}
