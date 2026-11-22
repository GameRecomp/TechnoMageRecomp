// SPDX-License-Identifier: MIT
//! Old Inno Setup installers (ANSI): **3.0.6.1** ("My Inno Setup Extensions", the German Crazy
//! School CD), **4.2.2** (the Polish Crazy School) and **4.2.6** (the Dutch Sacked). Other 1.x–4.x versions are refused with
//! [`Error::Unsupported`] rather than guessed at; the GOG installers (5.5/5.6) are read by
//! [`crate::inno`].
//!
//! Layout (format facts as documented by innoextract; this is an independent implementation):
//!
//! - At file offset `0x30` the setup loader keeps `Inno`, the offset of its offset table and that
//!   offset's complement. The 3.0 table (magic `rDlPtS02\x87eVx`) holds eight words: total size,
//!   the setup program's offset, compressed and uncompressed size and Adler-32, the messages'
//!   offset, then the offsets of setup-0 (headers) and setup-1 (file data). The 4.1.6 table
//!   (`rDlPtS07\x87eVx`) drops the compressed size and the messages and ends with a CRC-32.
//! - setup-0: a 64-byte version string, then two blocks. A 3.0 block starts with a CRC-32 over the
//!   two words that follow, its compressed size (`0xffffffff`: stored) and its size; a 4.x block
//!   with a CRC-32 over its stored size and a "compressed" byte. The data follows in sub-blocks of
//!   `[CRC-32][≤4096 bytes]`, one zlib (3.0) or LZMA1 (4.2) stream in all. Block 1 holds the setup
//!   header, the languages, the wizard images and the entry lists, block 2 the data entries.
//! - Strings are a 32-bit length and Windows-1252 bytes; flag sets are bit sets of as many bytes
//!   as their flags need (3 bytes padded to 4).
//! - File data: chunks at `setup-1 + offset`: `zlb\x1a`, then the compressed chunk. In 3.0 every
//!   file is a zlib chunk of its own, checked by its Adler-32; in 4.2 files are ranges of solid
//!   chunks (the header's compression: zlib, bzip2 or LZMA1), checked by MD5, executables stored
//!   with the 4.1.8 call-instruction filter (relative `call`/`jmp` targets made absolute).

use crate::inno::{RegistryEntry, RegistryValue};
use crate::md5::Md5;
use crate::{format_err, Archive, DirSink, Entry, Error, ReadSeek, Result, Sink};
use flate2::read::ZlibDecoder;
use std::collections::BTreeMap;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

const LOADER_30: &[u8; 12] = b"rDlPtS02\x87eVx";
const LOADER_416: &[u8; 12] = b"rDlPtS07\x87eVx";
const CHUNK_MAGIC: &[u8; 4] = b"zlb\x1a";

/// The versions read here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ver {
    /// 3.0.6.1, My Inno Setup Extensions.
    V3061,
    /// 4.2.2.
    V422,
    /// 4.2.6.
    V426,
}

const VERSIONS: [(&[u8], Ver); 3] = [
    (b"My Inno Setup Extensions Setup Data (3.0.6.1)", Ver::V3061),
    (b"Inno Setup Setup Data (4.2.2)", Ver::V422),
    (b"Inno Setup Setup Data (4.2.6)", Ver::V426),
];

/// Windows version range: two (begin, end) pairs of (build u16, minor u8, major u8) for Windows
/// and NT, and the NT service pack (u8 minor, u8 major), each twice.
const WINVER_RANGE: usize = 20;

/// Data entry flags.
const DATA30_BZIPPED: u8 = 1 << 2;
const DATA422_CALL_FILTER: u8 = 1 << 4;
const DATA422_ENCRYPTED: u8 = 1 << 6;
const DATA425_COMPRESSED: u8 = 1 << 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compression {
    Stored,
    Zlib,
    Lzma1,
}

#[derive(Debug, Clone)]
struct DataEntry {
    chunk_offset: u32,
    /// Where the file starts in its decompressed chunk (always 0 in 3.0).
    file_offset: u64,
    file_size: u64,
    chunk_size: u64,
    check: Check,
    call_filter: bool,
    /// The chunk is compressed with the header's method (else stored).
    compressed: bool,
}

#[derive(Debug, Clone, Copy)]
enum Check {
    Adler32(u32),
    Md5([u8; 16]),
}

pub struct Installer {
    file: Box<dyn ReadSeek>,
    data_offset: u64,
    compression: Compression,
    entries: Vec<Entry>,
    /// The data entry of each entry.
    location: Vec<usize>,
    directories: Vec<String>,
    data: Vec<DataEntry>,
    registry: Vec<RegistryEntry>,
}

/// Whether `r` is an Inno Setup installer with the old loader (version 1.x–5.1.4): `Inno` and a
/// checked table offset at `0x30`.
pub fn is_old_inno(r: &mut dyn ReadSeek) -> bool {
    let Some(at) = table_offset(r) else {
        return false;
    };
    let mut magic = [0u8; 12];
    r.seek(SeekFrom::Start(at)).is_ok()
        && r.read_exact(&mut magic).is_ok()
        && (&magic == LOADER_30 || &magic == LOADER_416)
}

fn table_offset(r: &mut dyn ReadSeek) -> Option<u64> {
    let mut b = [0u8; 12];
    r.seek(SeekFrom::Start(0x30)).ok()?;
    r.read_exact(&mut b).ok()?;
    let at = u32::from_le_bytes(b[4..8].try_into().ok()?);
    let not = u32::from_le_bytes(b[8..12].try_into().ok()?);
    (&b[..4] == b"Inno" && at == !not).then_some(u64::from(at))
}

/// Little-endian cursor over a decompressed block.
struct P<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> P<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .b
            .get(self.at..self.at + n)
            .ok_or_else(|| Error::Format(format!("setup header truncated at {:#x}", self.at)))?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2")))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8")))
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> Result<String> {
        Ok(crate::codepage::cp1252(self.bytes()?))
    }
    fn skip_strings(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.bytes()?;
        }
        Ok(())
    }
    fn end(&self) -> bool {
        self.at == self.b.len()
    }
}

impl Installer {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(Box::new(BufReader::new(std::fs::File::open(path)?)))
    }

    pub fn from_reader(mut file: Box<dyn ReadSeek>) -> Result<Self> {
        let table = table_offset(&mut file)
            .ok_or_else(|| Error::Format("no old Inno Setup loader table".into()))?;
        // 44 bytes for the 3.0 table, 40 for the 4.1.6 one (which may end the file).
        let mut t = [0u8; 44];
        file.seek(SeekFrom::Start(table))?;
        file.read_exact(&mut t[..40])?;
        if &t[..12] == LOADER_30 {
            file.read_exact(&mut t[40..])?;
        }
        let w = |i: usize| u32::from_le_bytes(t[12 + i * 4..16 + i * 4].try_into().expect("4"));
        let (header_offset, data_offset) = match &t[..12] {
            m if m == LOADER_30 => (w(6), w(7)),
            m if m == LOADER_416 => {
                if crc32fast::hash(&t[..36]) != w(6) {
                    return Err(Error::Checksum("Inno Setup loader table".into()));
                }
                (w(4), w(5))
            }
            m => {
                return Err(Error::Unsupported(format!(
                    "Inno Setup loader {:?} (of the old ones only 3.0 and 4.1.6)",
                    String::from_utf8_lossy(m)
                )))
            }
        };
        let mut r = BufReader::new(&mut file);
        r.seek(SeekFrom::Start(u64::from(header_offset)))?;
        let mut version = [0u8; 64];
        r.read_exact(&mut version)?;
        let vend = version.iter().position(|&c| c == 0).unwrap_or(64);
        let Some(&(_, ver)) = VERSIONS.iter().find(|(v, _)| *v == &version[..vend]) else {
            return Err(Error::Unsupported(format!(
                "Inno Setup data version {:?} (of the old ones only 3.0.6.1 and 4.2.2)",
                String::from_utf8_lossy(&version[..vend])
            )));
        };
        let (b1, b2) = match ver {
            Ver::V3061 => (read_block_30(&mut r)?, read_block_30(&mut r)?),
            Ver::V422 | Ver::V426 => (read_block_42(&mut r)?, read_block_42(&mut r)?),
        };
        drop(r);
        let mut inst = Installer {
            file,
            data_offset: u64::from(data_offset),
            compression: Compression::Zlib,
            entries: Vec::new(),
            location: Vec::new(),
            directories: Vec::new(),
            data: Vec::new(),
            registry: Vec::new(),
        };
        inst.parse(ver, &b1, &b2)?;
        Ok(inst)
    }

    fn parse(&mut self, ver: Ver, b1: &[u8], b2: &[u8]) -> Result<()> {
        let v4 = ver != Ver::V3061;
        let v426 = ver == Ver::V426;
        // Conditions: components, tasks, check (3.0); also languages, after and before install (4.2).
        let conditions = if v4 { 6 } else { 3 };
        let mut p = P { b: b1, at: 0 };
        // 23 strings: app name … default dir … base file name, licence and info texts,
        // uninstall settings, mutex, user defaults, serial, compiled code; 4.2.4 adds the readme
        // file, contact, comments and modify path.
        p.skip_strings(if v426 { 27 } else { 23 })?;
        p.skip(32)?; // lead bytes (a 256-bit set)
        let (languages, messages, permissions) = if v4 {
            (p.u32()?, p.u32()?, p.u32()?)
        } else {
            (1, 0, 0)
        };
        let (types, components, tasks) = (p.u32()?, p.u32()?, p.u32()?);
        let mut c = [0u32; 10];
        for v in &mut c {
            *v = p.u32()?;
        }
        let [dirs, files, data, icons, inis, regs, dels, undels, runs, unruns] = c;
        p.skip(WINVER_RANGE)?;
        p.skip(16)?; // back colours, image back colours
        let (bzip, encrypted);
        if v4 {
            p.skip(16 + 8)?; // password MD5 and salt
            p.skip(8 + 4)?; // extra disk space, slices per disk
                            // install mode, uninstall log mode and style, dir exists warning, privileges,
                            // language dialog and detection
            p.skip(7)?;
            self.compression = match (v426, p.u8()?) {
                (false, 0) | (true, 1) => Compression::Zlib,
                (false, 2) | (true, 3) => Compression::Lzma1,
                (true, 0) => Compression::Stored,
                (_, m) => return Err(Error::Unsupported(format!("Inno Setup 4 compression {m}"))),
            };
            let flags = p.take(6)?; // 42 flags
            bzip = false;
            encrypted = flags[5] & (1 << 1) != 0;
        } else {
            p.skip(4 + 4)?; // password CRC, extra disk space
            p.skip(5)?; // install mode, uninstall log mode and style, dir exists warning, privileges
            let flags = p.take(5)?; // 39 flags
            bzip = flags[4] & 1 != 0;
            encrypted = false;
        }
        if bzip || encrypted {
            return Err(Error::Unsupported(
                "bzip2-compressed or encrypted Inno Setup data".into(),
            ));
        }
        for _ in 0..languages {
            if v4 {
                p.skip_strings(10)?; // name, language name, four fonts, data, licence and info
                p.skip(24)?; // id, code page, four font sizes
            } else {
                p.skip_strings(5)?;
                p.skip(24)?; // id, five font sizes
            }
        }
        if !v4 {
            p.skip_strings(2)?; // wizard image and small wizard image
        }
        for _ in 0..messages {
            p.skip_strings(2)?;
            p.skip(4)?;
        }
        p.skip_strings(permissions as usize)?;
        for _ in 0..types {
            if !v4 {
                return Err(Error::Unsupported("Inno Setup 3 setup types".into()));
            }
            p.skip_strings(4)?;
            p.skip(WINVER_RANGE + 1 + 1 + 8)?;
        }
        for _ in 0..components {
            if !v4 {
                return Err(Error::Unsupported("Inno Setup 3 components".into()));
            }
            p.skip_strings(5)?;
            p.skip(8 + 4 + 1 + WINVER_RANGE + 1 + 8)?;
        }
        for _ in 0..tasks {
            p.skip_strings(if v4 { 6 } else { 5 })?;
            p.skip(4 + 1 + WINVER_RANGE + 1)?; // level, used, versions, flags
        }
        for _ in 0..dirs {
            let name = p.str()?;
            p.skip_strings(conditions)?;
            p.skip(4 + WINVER_RANGE + if v4 { 2 } else { 0 } + 1)?;
            if let Some(d) = strip_app(&name) {
                if !d.is_empty() {
                    crate::safe_relative(&d)?;
                    self.directories.push(d);
                }
            }
        }
        let mut file_entries = Vec::new();
        for _ in 0..files {
            p.skip_strings(1)?; // source
            let dest = p.str()?;
            p.skip_strings(1)?; // font name
            p.skip_strings(conditions)?;
            p.skip(WINVER_RANGE)?;
            let location = p.u32()?;
            p.skip(4)?; // attributes
            p.skip(if v4 { 8 + 2 } else { 4 })?; // external size (, permission)
            p.skip(4)?; // flags (20 or 23, padded to 4 bytes)
            let kind = p.u8()?; // 0 user file, 1 uninstaller, 2 regsvr
            file_entries.push((dest, location, kind));
        }
        for _ in 0..icons {
            p.skip_strings(6 + conditions)?;
            p.skip(WINVER_RANGE + 4 + 4 + 1 + 2 + 1)?;
        }
        for _ in 0..inis {
            p.skip_strings(4 + conditions)?;
            p.skip(WINVER_RANGE + 1)?;
        }
        for _ in 0..regs {
            let key = p.str()?;
            let name = p.str()?;
            let value = p.str()?;
            p.skip_strings(conditions)?;
            p.skip(WINVER_RANGE)?;
            let hive = p.u32()? & !0x8000_0000;
            if v4 {
                p.skip(2)?; // permission
            }
            let kind = p.u8()?;
            let flags = p.u16()?;
            let root = match hive {
                0 => "HKCR",
                1 => "HKCU",
                2 => "HKLM",
                3 => "HKU",
                5 => "HKCC",
                h => return format_err(format!("Inno registry hive {h}")),
            };
            let kind = match kind {
                0 => RegistryValue::None,
                1 => RegistryValue::String,
                2 => RegistryValue::ExpandString,
                3 => RegistryValue::Dword,
                4 => RegistryValue::Binary,
                5 => RegistryValue::MultiString,
                k => return format_err(format!("Inno registry value type {k}")),
            };
            self.registry.push(RegistryEntry {
                root,
                key,
                name,
                kind,
                value,
                has_check: false,
                deletes: flags & (1 << 6 | 1 << 7) != 0, // deletekey, deletevalue
            });
        }
        for _ in 0..dels + undels {
            p.skip_strings(1 + conditions)?;
            p.skip(WINVER_RANGE + 1)?;
        }
        for _ in 0..runs + unruns {
            // name, parameters, working dir, run once id, status, description
            p.skip_strings(6 + conditions)?;
            p.skip(WINVER_RANGE + 4 + 1 + 1)?; // show command, wait, flags
        }
        if v4 {
            p.skip_strings(2)?; // wizard images
            if v426 && self.compression == Compression::Zlib {
                p.skip_strings(1)?; // the decompressor DLL
            }
        }
        if !p.end() {
            return format_err("unknown data at the end of the Inno Setup header block");
        }
        let mut q = P { b: b2, at: 0 };
        for _ in 0..data {
            let (_first_slice, _last_slice) = (q.u32()?, q.u32()?);
            let chunk_offset = q.u32()?;
            let entry = if v4 {
                let file_offset = q.u64()?;
                let file_size = q.u64()?;
                let chunk_size = q.u64()?;
                let md5: [u8; 16] = q.take(16)?.try_into().expect("16");
                q.skip(8 + 8)?; // file time, file version
                let flags = q.u8()?;
                if flags & DATA422_ENCRYPTED != 0 {
                    return Err(Error::Unsupported("an encrypted Inno Setup chunk".into()));
                }
                DataEntry {
                    chunk_offset,
                    file_offset,
                    file_size,
                    chunk_size,
                    check: Check::Md5(md5),
                    call_filter: flags & DATA422_CALL_FILTER != 0,
                    compressed: !v426 || flags & DATA425_COMPRESSED != 0,
                }
            } else {
                let file_size = q.u32()?;
                let chunk_size = q.u32()?;
                let adler32 = q.u32()?;
                q.skip(8 + 8)?;
                if q.u8()? & DATA30_BZIPPED != 0 {
                    return Err(Error::Unsupported(
                        "a bzip2-compressed Inno Setup 3 file".into(),
                    ));
                }
                DataEntry {
                    chunk_offset,
                    file_offset: 0,
                    file_size: u64::from(file_size),
                    chunk_size: u64::from(chunk_size),
                    check: Check::Adler32(adler32),
                    call_filter: false,
                    compressed: true,
                }
            };
            self.data.push(entry);
        }
        if !q.end() {
            return format_err("unknown data at the end of the Inno Setup data entries");
        }
        for (dest, location, kind) in file_entries {
            if kind != 0 {
                continue; // the uninstaller
            }
            let Some(path) = strip_app(&dest) else {
                continue; // outside the install directory ({sys}, …)
            };
            crate::safe_relative(&path)?;
            let d = self
                .data
                .get(location as usize)
                .ok_or_else(|| Error::Format(format!("{dest}: data entry {location} missing")))?;
            self.entries.push(Entry {
                path,
                size: d.file_size,
            });
            self.location.push(location as usize);
        }
        Ok(())
    }

    /// Decompress the chunk at `offset` and hand the files `wanted` (data entry, entry indices;
    /// sorted by their offset in the chunk) to `sink`.
    fn extract_chunk(
        &mut self,
        offset: u32,
        wanted: &[(usize, Vec<usize>)],
        sink: &mut dyn Sink,
    ) -> Result<()> {
        let chunk_size = self.data[wanted[0].0].chunk_size;
        let compression = if self.data[wanted[0].0].compressed {
            self.compression
        } else {
            Compression::Stored
        };
        self.file
            .seek(SeekFrom::Start(self.data_offset + u64::from(offset)))?;
        let mut magic = [0u8; 4];
        self.file.read_exact(&mut magic)?;
        if &magic != CHUNK_MAGIC {
            return format_err(format!("bad chunk magic at data offset {offset:#x}"));
        }
        let end = wanted
            .iter()
            .map(|(d, _)| self.data[*d].file_offset + self.data[*d].file_size)
            .max()
            .unwrap_or(0);
        let targets: Vec<Target> = wanted
            .iter()
            .map(|(d, ents)| {
                let e = &self.data[*d];
                Target {
                    start: e.file_offset,
                    end: e.file_offset + e.file_size,
                    paths: ents.iter().map(|&i| self.entries[i].path.clone()).collect(),
                    check: e.check,
                    filter: e.call_filter.then(CallFilter::default),
                }
            })
            .collect();
        for t in &targets {
            if t.paths.len() > 1 {
                return Err(Error::Unsupported(
                    "an Inno Setup file installed twice".into(),
                ));
            }
        }
        let mut router = Router {
            sink,
            targets,
            next: 0,
            pos: 0,
            open: None,
        };
        let mut raw = (&mut self.file).take(chunk_size);
        match compression {
            Compression::Stored => {
                io::copy(&mut (&mut raw).take(end), &mut router)?;
            }
            Compression::Zlib => {
                let mut z = ZlibDecoder::new(&mut raw).take(end);
                io::copy(&mut z, &mut router)?;
            }
            Compression::Lzma1 => {
                crate::lzma::decode_lzma1(&mut raw, &mut router, Some(end))?;
            }
        }
        router.finish()
    }
}

/// One file in a decompressed chunk.
struct Target {
    start: u64,
    end: u64,
    paths: Vec<String>,
    check: Check,
    filter: Option<CallFilter>,
}

/// Running state of one file being written.
struct Open {
    adler: Adler32,
    md5: Md5,
}

/// Routes a decompressed chunk to the files in it.
struct Router<'s> {
    sink: &'s mut dyn Sink,
    targets: Vec<Target>,
    next: usize,
    pos: u64,
    open: Option<Open>,
}

impl Router<'_> {
    fn feed(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() && self.next < self.targets.len() {
            let t = &mut self.targets[self.next];
            if self.pos < t.start {
                let skip = ((t.start - self.pos) as usize).min(buf.len());
                buf = &buf[skip..];
                self.pos += skip as u64;
                continue;
            }
            if self.open.is_none() {
                self.sink.begin(&t.paths[0], t.end - t.start)?;
                self.open = Some(Open {
                    adler: Adler32::default(),
                    md5: Md5::new(),
                });
            }
            let n = ((t.end - self.pos) as usize).min(buf.len());
            let mut piece = buf[..n].to_vec();
            if let Some(f) = &mut t.filter {
                f.apply(&mut piece);
            }
            let o = self.open.as_mut().expect("open");
            o.adler.update(&piece);
            o.md5.update(&piece);
            self.sink.write(&piece)?;
            buf = &buf[n..];
            self.pos += n as u64;
            if self.pos == t.end {
                self.close()?;
            }
        }
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        let t = &self.targets[self.next];
        let o = self.open.take().expect("open");
        let ok = match t.check {
            Check::Adler32(want) => o.adler.finish() == want,
            Check::Md5(want) => o.md5.finish() == want,
        };
        if !ok {
            return Err(Error::Checksum(format!("{}: checksum", t.paths[0])));
        }
        self.sink.end()?;
        self.next += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        // Empty files at the very end.
        while self.next < self.targets.len()
            && self.targets[self.next].start == self.targets[self.next].end
        {
            self.feed(&[])?;
            let t = &self.targets[self.next];
            self.sink.begin(&t.paths[0], 0)?;
            self.open = Some(Open {
                adler: Adler32::default(),
                md5: Md5::new(),
            });
            self.close()?;
        }
        if self.next != self.targets.len() {
            return format_err("Inno Setup chunk ends before its files");
        }
        Ok(())
    }
}

impl Write for Router<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.feed(buf)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Inno Setup 4.1.8's call-instruction filter, reversed: the 32-bit operand after each `e8`/`e9`
/// was made absolute (position-independent bytes compress better); subtract the position again.
#[derive(Default)]
struct CallFilter {
    offset: u32,
    addr: u32,
    left: u8,
    started: bool,
}

impl CallFilter {
    fn apply(&mut self, buf: &mut [u8]) {
        if !self.started {
            self.offset = 5;
            self.started = true;
        }
        for b in buf.iter_mut() {
            if self.left == 0 {
                if *b == 0xe8 || *b == 0xe9 {
                    self.addr = (!self.offset).wrapping_add(1);
                    self.left = 4;
                }
            } else {
                self.addr = self.addr.wrapping_add(u32::from(*b));
                *b = self.addr as u8;
                self.addr >>= 8;
                self.left -= 1;
            }
            self.offset = self.offset.wrapping_add(1);
        }
    }
}

/// Read one setup-0 block, 3.0 layout (zlib).
fn read_block_30(r: &mut impl Read) -> Result<Vec<u8>> {
    let mut h = [0u8; 12];
    r.read_exact(&mut h)?;
    let crc = u32::from_le_bytes(h[0..4].try_into().expect("4"));
    if crc32fast::hash(&h[4..12]) != crc {
        return Err(Error::Checksum("setup block header".into()));
    }
    let compressed = u32::from_le_bytes(h[4..8].try_into().expect("4"));
    let size = u32::from_le_bytes(h[8..12].try_into().expect("4"));
    let stored = if compressed == u32::MAX {
        size
    } else {
        compressed
    } as usize;
    let payload = read_sub_blocks(r, stored, false)?;
    if compressed == u32::MAX {
        return Ok(payload);
    }
    let mut out = Vec::with_capacity(size as usize);
    ZlibDecoder::new(&payload[..]).read_to_end(&mut out)?;
    if out.len() != size as usize {
        return format_err("setup block size differs from its header");
    }
    Ok(out)
}

/// Read one setup-0 block, 4.1.6+ layout (LZMA1).
fn read_block_42(r: &mut impl Read) -> Result<Vec<u8>> {
    let mut h = [0u8; 9];
    r.read_exact(&mut h)?;
    if crc32fast::hash(&h[4..9]) != u32::from_le_bytes(h[0..4].try_into().expect("4")) {
        return Err(Error::Checksum("setup block header".into()));
    }
    let stored = u32::from_le_bytes(h[4..8].try_into().expect("4")) as usize;
    let payload = read_sub_blocks(r, stored, true)?;
    if h[8] == 0 {
        return Ok(payload);
    }
    let mut out = Vec::new();
    crate::lzma::decode_lzma1(&mut &payload[..], &mut out, None)?;
    Ok(out)
}

/// The CRC-checked sub-blocks of a setup block: `stored` counts the CRCs when `with_crcs`.
fn read_sub_blocks(r: &mut impl Read, stored: usize, with_crcs: bool) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(stored);
    let mut left = stored;
    while left > 0 {
        let n = if with_crcs {
            if left < 5 {
                return format_err("truncated setup sub-block");
            }
            (left - 4).min(4096)
        } else {
            left.min(4096)
        };
        let mut crc = [0u8; 4];
        r.read_exact(&mut crc)?;
        let start = payload.len();
        payload.resize(start + n, 0);
        r.read_exact(&mut payload[start..])?;
        if crc32fast::hash(&payload[start..]) != u32::from_le_bytes(crc) {
            return Err(Error::Checksum("setup sub-block".into()));
        }
        left -= n + if with_crcs { 4 } else { 0 };
    }
    Ok(payload)
}

/// `{app}\x\y` → `x/y`; `None` for other destinations.
fn strip_app(dest: &str) -> Option<String> {
    let lower = dest.to_ascii_lowercase();
    let rest = lower.strip_prefix("{app}")?;
    if !(rest.is_empty() || rest.starts_with('\\')) {
        return None;
    }
    let rel = &dest[5..];
    let parts: Vec<&str> = rel.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    Some(parts.join("/"))
}

#[derive(Clone, Copy)]
struct Adler32 {
    a: u32,
    b: u32,
}

impl Default for Adler32 {
    fn default() -> Self {
        Adler32 { a: 1, b: 0 }
    }
}

impl Adler32 {
    fn update(&mut self, data: &[u8]) {
        const MOD: u32 = 65521;
        for chunk in data.chunks(5552) {
            for &x in chunk {
                self.a += u32::from(x);
                self.b += self.a;
            }
            self.a %= MOD;
            self.b %= MOD;
        }
    }
    fn finish(&self) -> u32 {
        self.b << 16 | self.a
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
        // By chunk, then by data entry (in chunk order), so each chunk is decompressed once.
        let mut by_chunk: BTreeMap<u32, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
        let mut n = 0;
        for (i, e) in self.entries.iter().enumerate() {
            if select(e) {
                let d = self.location[i];
                by_chunk
                    .entry(self.data[d].chunk_offset)
                    .or_default()
                    .entry(d)
                    .or_default()
                    .push(i);
                n += 1;
            }
        }
        for (chunk, files) in by_chunk {
            let mut wanted: Vec<(usize, Vec<usize>)> = files.into_iter().collect();
            wanted.sort_by_key(|(d, _)| self.data[*d].file_offset);
            self.extract_chunk(chunk, &wanted, sink)?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_known_value() {
        let mut a = Adler32::default();
        a.update(b"Wikipedia");
        assert_eq!(a.finish(), 0x11e6_0398);
        let mut big = Adler32::default();
        big.update(&vec![0xffu8; 100_000]);
        let mut parts = Adler32::default();
        for c in vec![0xffu8; 100_000].chunks(777) {
            parts.update(c);
        }
        assert_eq!(big.finish(), parts.finish());
    }

    #[test]
    fn call_filter_reverses_absolute_targets() {
        // At file offset 0 a call whose relative target 0x10 was stored as 0x10 + 5.
        let mut code = vec![0xe8, 0x15, 0, 0, 0, 0x90];
        CallFilter::default().apply(&mut code);
        assert_eq!(code, [0xe8, 0x10, 0, 0, 0, 0x90]);
    }

    #[test]
    fn app_paths() {
        assert_eq!(
            strip_app("{app}\\Levels\\a.col").as_deref(),
            Some("Levels/a.col")
        );
        assert_eq!(strip_app("{APP}").as_deref(), Some(""));
        assert_eq!(strip_app("{sys}\\x.dll"), None);
        assert_eq!(strip_app("{apps}\\x"), None);
    }

    #[test]
    fn block_round_trip() {
        use flate2::write::ZlibEncoder;
        use std::io::Write;
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let mut e = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(&data).unwrap();
        let z = e.finish().unwrap();
        let mut blk = Vec::new();
        let mut words = (z.len() as u32).to_le_bytes().to_vec();
        words.extend((data.len() as u32).to_le_bytes());
        blk.extend(crc32fast::hash(&words).to_le_bytes());
        blk.extend(&words);
        for c in z.chunks(4096) {
            blk.extend(crc32fast::hash(c).to_le_bytes());
            blk.extend(c);
        }
        assert_eq!(read_block_30(&mut &blk[..]).unwrap(), data);
    }
}
