// SPDX-License-Identifier: MIT
//! Nullsoft installers (NSIS) of the 1.9x era, as Limbic Entertainment built them ("Limbic
//! Installer (NSIS)", Gefeuert's CD). Newer NSIS layouts are refused with
//! [`Error::Unsupported`] rather than guessed at.
//!
//! Layout, as measured on the supported installer (our own description):
//!
//! - The *first header* sits at a 512-byte boundary of the exe: flags, the signature
//!   `0xDEADBEEF`, `NullsoftInst`, the size of the header once inflated and the size of all data
//!   that follows the first header (to the end of the file).
//! - Then blocks: a 32-bit length whose top bit says "compressed", and that many bytes, raw
//!   deflate when compressed. The first block is the header; every installed file is one more
//!   block, and a file entry names its block by its offset counted from the end of the header
//!   block. Two entries may share a block.
//! - The inflated header: 60 words of settings (installer texts, colours, the section count in
//!   word 56, the entry count in word 13), the sections (five words each: name, install types,
//!   first entry, entry count, size in KiB), the entries (six words each: opcode and five
//!   parameters), then the string table, to which string parameters are offsets.
//! - Opcodes used: 14 creates a directory (and makes it the output directory when its second
//!   parameter is set), 21 extracts a file into the output directory (name, block offset, file
//!   time), 50 writes a registry value (root, key, name, value, type 1 = string), 42 makes a
//!   shortcut, 61 writes the uninstaller, 1 returns. Paths begin with a variable code: `0xF3` is
//!   the install directory (`$INSTDIR`), others are system folders (start menu, program files).

use crate::inno::{RegistryEntry, RegistryValue};
use crate::{format_err, Archive, DirSink, Entry, Error, ReadSeek, Result, Sink};
use flate2::read::DeflateDecoder;
use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const FIRST_MAGIC: &[u8; 16] = b"\xef\xbe\xad\xdeNullsoftInst";
const HEADER_WORDS: usize = 60;
const SECTION_WORDS: usize = 5;
const ENTRY_WORDS: usize = 6;
const WORD_ENTRIES: usize = 13;
const WORD_SECTIONS: usize = 56;

const OP_RETURN: i32 = 1;
const OP_CREATE_DIR: i32 = 14;
const OP_EXTRACT: i32 = 21;
const OP_SHORTCUT: i32 = 42;
const OP_WRITE_REG: i32 = 50;
const OP_WRITE_UNINSTALLER: i32 = 61;

/// The variable code of the install directory in a string.
const VAR_INSTDIR: u8 = 0xf3;

pub struct Installer {
    file: Box<dyn ReadSeek>,
    /// Where block offsets count from (the end of the header block).
    data_start: u64,
    entries: Vec<Entry>,
    /// The block of each entry.
    blocks: Vec<u64>,
    directories: Vec<String>,
    registry: Vec<RegistryEntry>,
}

/// The offset of the first header in `r`, if `r` is an NSIS installer.
pub fn find_first_header(r: &mut dyn ReadSeek) -> Option<u64> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    let mut at = 0u64;
    let mut buf = [0u8; 16];
    // The stub is small; the first header follows it within the first megabyte.
    while at + 28 <= len.min(1 << 20) {
        r.seek(SeekFrom::Start(at + 4)).ok()?;
        if r.read_exact(&mut buf).is_ok() && &buf == FIRST_MAGIC {
            return Some(at);
        }
        at += 512;
    }
    None
}

impl Installer {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(Box::new(io::BufReader::new(std::fs::File::open(path)?)))
    }

    pub fn from_reader(mut file: Box<dyn ReadSeek>) -> Result<Self> {
        let first = find_first_header(&mut file)
            .ok_or_else(|| Error::Format("no NSIS first header".into()))?;
        let total = file.seek(SeekFrom::End(0))?;
        let mut fh = [0u8; 28];
        file.seek(SeekFrom::Start(first))?;
        file.read_exact(&mut fh)?;
        let word =
            |b: &[u8], i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().expect("4"));
        let header_size = word(&fh, 5) as usize;
        let following = u64::from(word(&fh, 6));
        if first + following != total {
            return format_err(format!(
                "NSIS data ends at {:#x}, the file at {total:#x}",
                first + following
            ));
        }
        let blocks_start = first + 28;
        let (header, header_block) = read_block(&mut file, blocks_start)?;
        if header.len() != header_size {
            return format_err("NSIS header size differs from the first header's");
        }
        let data_start = blocks_start + header_block;
        let mut inst = Installer {
            file,
            data_start,
            entries: Vec::new(),
            blocks: Vec::new(),
            directories: Vec::new(),
            registry: Vec::new(),
        };
        inst.parse_header(&header)?;
        inst.measure()?;
        Ok(inst)
    }

    fn parse_header(&mut self, h: &[u8]) -> Result<()> {
        let word = |i: usize| -> Result<i32> {
            h.get(i * 4..i * 4 + 4)
                .map(|b| i32::from_le_bytes(b.try_into().expect("4")))
                .ok_or_else(|| Error::Format(format!("NSIS header truncated at word {i}")))
        };
        let sections = word(WORD_SECTIONS)?;
        let entries = word(WORD_ENTRIES)?;
        let strings = (HEADER_WORDS
            + usize::try_from(sections).unwrap_or(usize::MAX / 64) * SECTION_WORDS
            + usize::try_from(entries).unwrap_or(usize::MAX / 64) * ENTRY_WORDS)
            .saturating_mul(4);
        if sections <= 0 || entries <= 0 || strings >= h.len() {
            return Err(Error::Unsupported(
                "an NSIS header layout other than the 1.9x one Limbic's installers use".into(),
            ));
        }
        let string = |off: i32| -> Result<&[u8]> {
            let start = usize::try_from(off)
                .ok()
                .and_then(|o| strings.checked_add(o))
                .filter(|&s| s < h.len())
                .ok_or_else(|| Error::Format(format!("NSIS string offset {off} out of range")))?;
            crate::cstr(h, start)
        };
        let entry_base = HEADER_WORDS + sections as usize * SECTION_WORDS;
        let mut outdir = String::new();
        let mut seen_dirs = BTreeMap::new();
        for s in 0..sections as usize {
            let sec = HEADER_WORDS + s * SECTION_WORDS;
            let (code, count) = (word(sec + 2)?, word(sec + 3)?);
            if code < 0 || count < 0 || code + count > entries {
                return format_err(format!(
                    "NSIS section {s}: entries {code}+{count} out of range"
                ));
            }
            for e in code..code + count {
                let w = entry_base + e as usize * ENTRY_WORDS;
                let op = word(w)?;
                let p = |i: usize| word(w + 1 + i);
                match op {
                    OP_CREATE_DIR => {
                        // Only directories in the install directory matter here.
                        if let Some(dir) = install_path(string(p(0)?)?)? {
                            if !dir.is_empty() && seen_dirs.insert(dir.clone(), ()).is_none() {
                                self.directories.push(dir.clone());
                            }
                            if p(1)? != 0 {
                                outdir = dir;
                            }
                        } else if p(1)? != 0 {
                            return Err(Error::Unsupported(
                                "NSIS files extracted outside the install directory".into(),
                            ));
                        }
                    }
                    OP_EXTRACT => {
                        let name = latin1(string(p(1)?)?);
                        if name.bytes().any(|c| c >= 0xe0) || name.contains(['\\', '/']) {
                            return Err(Error::Unsupported(format!(
                                "NSIS file name with a variable or a directory: {name:?}"
                            )));
                        }
                        let path = if outdir.is_empty() {
                            name
                        } else {
                            format!("{outdir}/{name}")
                        };
                        crate::safe_relative(&path)?;
                        let block = u64::try_from(p(2)?)
                            .map_err(|_| Error::Format("negative NSIS data offset".into()))?;
                        self.entries.push(Entry { path, size: 0 });
                        self.blocks.push(block);
                    }
                    OP_WRITE_REG => {
                        let root = match p(0)? as u32 {
                            0x8000_0000 => "HKCR",
                            0x8000_0001 => "HKCU",
                            0x8000_0002 => "HKLM",
                            0x8000_0003 => "HKU",
                            0x8000_0005 => "HKCC",
                            r => return format_err(format!("NSIS registry root {r:#x}")),
                        };
                        let kind = match p(4)? {
                            1 => RegistryValue::String,
                            t => return Err(Error::Unsupported(format!("NSIS registry type {t}"))),
                        };
                        self.registry.push(RegistryEntry {
                            root,
                            key: latin1(string(p(1)?)?),
                            name: latin1(string(p(2)?)?),
                            kind,
                            value: expand(string(p(3)?)?),
                            has_check: false,
                            deletes: false,
                        });
                    }
                    OP_RETURN | OP_SHORTCUT | OP_WRITE_UNINSTALLER => {}
                    op => {
                        return Err(Error::Unsupported(format!("NSIS opcode {op} in a section")));
                    }
                }
            }
        }
        if self.entries.is_empty() {
            return format_err("NSIS installer without files");
        }
        Ok(())
    }

    /// Every entry's size: its block inflated (each block once).
    fn measure(&mut self) -> Result<()> {
        let mut sizes: BTreeMap<u64, u64> = BTreeMap::new();
        for b in self.blocks.clone() {
            if let std::collections::btree_map::Entry::Vacant(slot) = sizes.entry(b) {
                let mut count = CountSink(0);
                self.copy_block(b, &mut count)?;
                slot.insert(count.0);
            }
        }
        for (e, b) in self.entries.iter_mut().zip(&self.blocks) {
            e.size = sizes[b];
        }
        Ok(())
    }

    /// The bytes of the block at `offset`, to `out` (inflated when compressed).
    fn copy_block(&mut self, offset: u64, out: &mut dyn io::Write) -> Result<u64> {
        let at = self.data_start + offset;
        self.file.seek(SeekFrom::Start(at))?;
        let mut len = [0u8; 4];
        self.file.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len);
        let (compressed, size) = (len & 0x8000_0000 != 0, u64::from(len & 0x7fff_ffff));
        let mut raw = (&mut self.file).take(size);
        if compressed {
            let mut d = DeflateDecoder::new(&mut raw);
            let n = io::copy(&mut d, out)?;
            if d.total_in() != size {
                return Err(Error::Checksum(format!(
                    "NSIS block at {at:#x}: the deflate stream ends before the block"
                )));
            }
            Ok(n)
        } else {
            let n = io::copy(&mut raw, out)?;
            if n != size {
                return format_err(format!("NSIS block at {at:#x} truncated"));
            }
            Ok(n)
        }
    }
}

/// Read the block at `at`: its contents and its whole size (with the length word).
fn read_block(r: &mut dyn ReadSeek, at: u64) -> Result<(Vec<u8>, u64)> {
    r.seek(SeekFrom::Start(at))?;
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    let size = u64::from(len & 0x7fff_ffff);
    let mut raw = vec![0u8; size as usize];
    r.read_exact(&mut raw)?;
    let data = if len & 0x8000_0000 != 0 {
        let mut out = Vec::new();
        DeflateDecoder::new(&raw[..]).read_to_end(&mut out)?;
        out
    } else {
        raw
    };
    Ok((data, 4 + size))
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// A path string relative to the install directory (`/`-separated), or `None` when it is in
/// another folder (a system folder variable) or not a path at all.
fn install_path(s: &[u8]) -> Result<Option<String>> {
    let Some((&VAR_INSTDIR, rest)) = s.split_first() else {
        return Ok(None);
    };
    let rest = latin1(rest);
    let rel: Vec<&str> = rest.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let rel = rel.join("/");
    if !rel.is_empty() {
        crate::safe_relative(&rel)?;
    }
    Ok(Some(rel))
}

/// A registry value with the install directory as Inno's `{app}` (the runtime fills it in).
fn expand(s: &[u8]) -> String {
    let mut out = String::new();
    for &c in s {
        if c == VAR_INSTDIR {
            out.push_str("{app}");
        } else {
            out.push(char::from(c));
        }
    }
    out
}

struct CountSink(u64);

impl io::Write for CountSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Feeds a [`Sink`] as an [`io::Write`].
struct SinkWriter<'s>(&'s mut dyn Sink);

impl io::Write for SinkWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .write(buf)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Archive for Installer {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn directories(&self) -> &[String] {
        &self.directories
    }

    fn registry(&self) -> &[RegistryEntry] {
        &self.registry
    }

    fn extract(&mut self, dest: &Path, select: &dyn Fn(&Entry) -> bool) -> Result<usize> {
        self.extract_to(&mut DirSink::new(dest), select)
    }

    fn extract_to(
        &mut self,
        sink: &mut dyn Sink,
        select: &dyn Fn(&Entry) -> bool,
    ) -> Result<usize> {
        for d in &self.directories {
            sink.dir(d)?;
        }
        let mut n = 0;
        for i in 0..self.entries.len() {
            if !select(&self.entries[i]) {
                continue;
            }
            let (path, size, block) = (
                self.entries[i].path.clone(),
                self.entries[i].size,
                self.blocks[i],
            );
            sink.begin(&path, size)?;
            let got = self.copy_block(block, &mut SinkWriter(sink))?;
            if got != size {
                return Err(Error::Checksum(format!(
                    "{path}: {got} bytes, {size} expected"
                )));
            }
            sink.end()?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::{Cursor, Write};

    fn block(data: &[u8], compress: bool) -> Vec<u8> {
        let payload = if compress {
            let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        } else {
            data.to_vec()
        };
        let mut out = (payload.len() as u32 | if compress { 0x8000_0000 } else { 0 })
            .to_le_bytes()
            .to_vec();
        out.extend(payload);
        out
    }

    /// An installer in the 1.9x layout: one section that makes `Sub`, extracts two files (the
    /// second twice, as another name) and writes `{app}` into the registry.
    fn installer() -> Vec<u8> {
        let mut strings = Vec::new();
        let mut s = |t: &[u8]| {
            let o = strings.len() as i32;
            strings.extend_from_slice(t);
            strings.push(0);
            o
        };
        let title = s(b"Test");
        let sub = s(b"\xf3\\Sub");
        let a = s(b"a.txt");
        let b = s(b"b.bin");
        let c = s(b"c.bin");
        let key = s(b"Software\\Test");
        let name = s(b"Install_Dir");
        let app = s(b"\xf3");
        let files = [b"hello".to_vec(), vec![7u8; 3000]];
        let blk0 = block(&files[0], true);
        let blk1 = block(&files[1], false);
        let entries: Vec<[i32; 6]> = vec![
            [OP_CREATE_DIR, sub, 1, 0, 0, 0],
            [OP_EXTRACT, 0, a, 0, 0, 0],
            [OP_EXTRACT, 0, b, blk0.len() as i32, 0, 0],
            [OP_EXTRACT, 0, c, blk0.len() as i32, 0, 0],
            [OP_WRITE_REG, 0x8000_0002u32 as i32, key, name, app, 1],
            [OP_RETURN, 0, 0, 0, 0, 0],
        ];
        let mut words = vec![0i32; HEADER_WORDS];
        words[0] = title;
        words[WORD_ENTRIES] = entries.len() as i32;
        words[WORD_SECTIONS] = 1;
        words.extend([title, -1, 0, entries.len() as i32, 1]);
        for e in &entries {
            words.extend(e);
        }
        let mut header: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        header.extend(&strings);
        let mut data = block(&header, true);
        data.extend(blk0);
        data.extend(blk1);
        let mut exe = vec![0u8; 1024];
        exe[..2].copy_from_slice(b"MZ");
        let first = exe.len();
        exe.extend(0u32.to_le_bytes());
        exe.extend(FIRST_MAGIC);
        exe.extend((header.len() as u32).to_le_bytes());
        exe.extend(((28 + data.len()) as u32).to_le_bytes());
        exe.extend(data);
        assert_eq!(first, 1024);
        exe
    }

    #[test]
    fn reads_the_limbic_layout() {
        let mut inst = Installer::from_reader(Box::new(Cursor::new(installer()))).unwrap();
        let paths: Vec<_> = inst
            .entries()
            .iter()
            .map(|e| (e.path.as_str(), e.size))
            .collect();
        assert_eq!(
            paths,
            [("Sub/a.txt", 5), ("Sub/b.bin", 3000), ("Sub/c.bin", 3000)]
        );
        assert_eq!(inst.directories(), ["Sub"]);
        let r = &inst.registry()[0];
        assert_eq!(
            (r.root, r.key.as_str(), r.name.as_str(), r.value.as_str()),
            ("HKLM", "Software\\Test", "Install_Dir", "{app}")
        );
        struct Mem(Vec<(String, Vec<u8>)>);
        impl Sink for Mem {
            fn dir(&mut self, _: &str) -> Result<()> {
                Ok(())
            }
            fn begin(&mut self, p: &str, _: u64) -> Result<()> {
                self.0.push((p.into(), Vec::new()));
                Ok(())
            }
            fn write(&mut self, d: &[u8]) -> Result<()> {
                self.0.last_mut().unwrap().1.extend_from_slice(d);
                Ok(())
            }
            fn end(&mut self) -> Result<()> {
                Ok(())
            }
        }
        let mut m = Mem(Vec::new());
        assert_eq!(inst.extract_to(&mut m, &|_| true).unwrap(), 3);
        assert_eq!(m.0[0].1, b"hello");
        assert_eq!(m.0[2].1, vec![7u8; 3000]);
    }
}
