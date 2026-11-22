// SPDX-License-Identifier: MIT
//! Inno Setup reader for the GOG installers: setup data versions **5.5.0 (unicode)** and
//! **5.6.2 (unicode)**, with the file data embedded in the installer exe or in external
//! slices (`setup_….exe` + `setup_…-1.bin`, …). Other versions are refused with
//! [`Error::Unsupported`] rather than guessed at.
//!
//! Layout (format facts as documented by innoextract; this is an independent implementation):
//!
//! - The setup loader's offset table (magic `rDlPtS…` / `nS5W7dT…`, CRC-32 protected) gives
//!   the absolute offsets of setup-0 (headers) and setup-1 (file data).
//! - setup-0: a 64-byte version string, then two compressed blocks. A block is a 9-byte
//!   header (CRC-32, stored size, compressed flag) and sub-blocks of `[CRC-32][≤4096 bytes]`,
//!   compressed as one LZMA1 stream. Block 1 holds the setup header and the entry lists,
//!   block 2 the data entries (file locations).
//! - File data lives in chunks at `setup-1 + chunk_offset`: magic `zlb\x1a`, then the
//!   chunk compressed with the header's method (LZMA1 for these installers). When the
//!   loader's setup-1 offset is 0 the data is in slice files instead: each begins with a
//!   12-byte header (`idska32\x1a`, the slice's size), and a data entry's chunk offset is
//!   counted from the start of its first slice file. Files are
//!   ranges of the decompressed chunk. Executables may be stored with Inno's x86
//!   call-instruction filter, which is reversed here. Every file is SHA-1 checked.
//! - GOG's Galaxy-style installers (2018+) install every file as `{tmp}\xx\yy\<md5>`, a
//!   part of at most 10 MiB, and their script puts the parts together. The file entries'
//!   conditions say how: `before_install('<md5>', '<path>', <parts>)` on a file's first part
//!   (`before_install_dependency` for redistributables), `after_install('<md5>', <stored size>,
//!   <size>)` on every part, the next parts following in order. Every part is a zlib stream
//!   (even when its stored size equals its size). These are read as the files they make.

use crate::{format_err, Archive, DirSink, Entry, Error, ReadSeek, Result, Sink, SubReader};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

const LOADER_MAGICS: [[u8; 12]; 2] = [
    *b"rDlPtS\xcd\xe6\xd7\x7b\x0b\x2a",
    *b"nS5W7dT\x83\xaa\x1b\x0f\x6a",
];
/// A setup data version: major, minor, patch.
type Version = (u8, u8, u8);

/// The setup data versions read here, with their version numbers.
const SUPPORTED_VERSIONS: [(&[u8], Version); 2] = [
    (b"Inno Setup Setup Data (5.5.0) (u)", (5, 5, 0)),
    (b"Inno Setup Setup Data (5.6.2) (u)", (5, 6, 2)),
];
const SLICE_MAGIC: &[u8; 8] = b"idska32\x1a";
const CHUNK_MAGIC: &[u8; 4] = b"zlb\x1a";

const COMPRESSION_STORED: u8 = 0;
const COMPRESSION_LZMA1: u8 = 3;
const COMPRESSION_LZMA2: u8 = 4;

const DATA_FLAG_CALL_FILTER: u16 = 1 << 4;
const DATA_FLAG_ENCRYPTED: u16 = 1 << 6;
const DATA_FLAG_COMPRESSED: u16 = 1 << 7;

#[derive(Debug, Clone)]
struct DataEntry {
    first_slice: u32,
    last_slice: u32,
    chunk_offset: u32,
    file_offset: u64,
    file_size: u64,
    chunk_size: u64,
    sha1: [u8; 20],
    flags: u16,
}

/// Facts from the setup header that callers may want to show.
#[derive(Debug, Clone)]
pub struct SetupInfo {
    pub app_name: String,
    pub app_version: String,
    pub compression: u8,
    pub file_count: usize,
    pub data_entry_count: usize,
}

pub struct Installer {
    file: Box<dyn ReadSeek>,
    data_offset: u64,
    /// The external slice files, by slice number (empty when the data is embedded); `None`
    /// for one that could not be opened (an error only when a chunk needs it).
    slices: Vec<Option<Box<dyn ReadSeek>>>,
    info: SetupInfo,
    entries: Vec<Entry>,
    directories: Vec<String>,
    /// Where the bytes of each entry in `entries` come from.
    location: Vec<Source>,
    data: Vec<DataEntry>,
    registry: Vec<RegistryEntry>,
    slices_per_disk: u32,
}

/// Where an entry's bytes are.
#[derive(Debug, Clone)]
enum Source {
    /// One data entry, as stored.
    Data(usize),
    /// GOG Galaxy parts, in order.
    Parts(Vec<Part>),
}

#[derive(Debug, Clone, Copy)]
struct Part {
    data: usize,
    /// Its size after inflating.
    size: u64,
}

/// One `[Files]` entry: what the reader needs of it.
struct FileEntry {
    dest: String,
    location: u32,
    after: String,
    before: String,
}

/// A value the installer writes into the registry (`[Registry]` in the setup script).
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// `HKCR`, `HKCU`, `HKLM`, `HKU` or `HKCC`.
    pub root: &'static str,
    pub key: String,
    /// Empty for the key's default value.
    pub name: String,
    pub kind: RegistryValue,
    /// Inno constants (`{app}`, …) are left in place.
    pub value: String,
    /// A `Check:` function decides at install time whether the entry is written.
    pub has_check: bool,
    /// `deletekey` or `deletevalue`: the entry removes rather than writes.
    pub deletes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryValue {
    /// Only the key is created.
    None,
    String,
    ExpandString,
    Dword,
    Binary,
    MultiString,
    Qword,
}

/// Little-endian cursor over a decompressed block.
struct P<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> P<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .b
            .get(self.o..self.o.saturating_add(n))
            .ok_or_else(|| Error::Format(format!("setup data truncated at {:#x}", self.o)))?;
        self.o += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let s = self.take(8)?;
        Ok(u64::from_le_bytes(s.try_into().expect("8 bytes")))
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    /// Length-prefixed byte string (binary, ANSI or UTF-16LE; the caller decodes).
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn skip_strings(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.bytes()?;
        }
        Ok(())
    }
    /// UTF-16LE string ("encoded string" in a unicode installer).
    fn wstr(&mut self) -> Result<String> {
        let b = self.bytes()?;
        if b.len() % 2 != 0 {
            return format_err(format!("odd UTF-16 string length at {:#x}", self.o));
        }
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

const WINVER_RANGE: usize = 20;
const ITEM_CONDITIONS: usize = 6; // components, tasks, languages, check, after/before install

struct Counts {
    slices_per_disk: u32,
    language: u32,
    message: u32,
    permission: u32,
    kind: u32,
    component: u32,
    task: u32,
    directory: u32,
    file: u32,
    data_entry: u32,
    icon: u32,
    ini: u32,
    registry: u32,
}

impl Installer {
    /// The installer at `path`; external slices are opened beside it
    /// (`setup_x.exe` → `setup_x-1.bin`, `setup_x-2.bin`, …).
    pub fn open(path: &Path) -> Result<Self> {
        let mut inst = Self::parse(Box::new(File::open(path)?), Vec::new())?;
        if inst.data_offset == 0 {
            let stem = path.with_extension("");
            let stem = stem.to_string_lossy();
            let count = inst
                .data
                .iter()
                .map(|d| d.last_slice + 1)
                .max()
                .unwrap_or(0);
            for i in 0..count {
                let name = slice_name(&stem, i, inst.slices_per_disk);
                inst.slices.push(
                    File::open(&name)
                        .ok()
                        .map(|f| Box::new(f) as Box<dyn ReadSeek>),
                );
            }
        }
        inst.add_gog_script_registry();
        Ok(inst)
    }

    /// The installer read from `file` (in a browser: the chosen file, read in pieces).
    pub fn from_reader(file: Box<dyn ReadSeek>) -> Result<Self> {
        Self::from_readers(file, Vec::new())
    }

    /// The installer read from `file`, its external slices (if any) from `slices`, in slice
    /// order (`-1.bin` first).
    pub fn from_readers(file: Box<dyn ReadSeek>, slices: Vec<Box<dyn ReadSeek>>) -> Result<Self> {
        let mut inst = Self::parse(file, slices)?;
        inst.add_gog_script_registry();
        Ok(inst)
    }

    /// GOG's Galaxy installers leave part of the set-up to `goggame-<id>.script` (JSON), run
    /// after the files are in place: its `setRegistry` actions are added to the installer's
    /// registry entries. Actions limited to some Windows versions (`os`) are compatibility
    /// settings for those and are left out. Without the script (or its data) nothing is added.
    fn add_gog_script_registry(&mut self) {
        let Some(i) = self.entries.iter().position(|e| {
            let p = e.path.to_ascii_lowercase();
            p.starts_with("goggame-") && p.ends_with(".script") && !p.contains('/')
        }) else {
            return;
        };
        let Source::Parts(_) = self.location[i] else {
            return;
        };
        let mut mem = MemSink(Vec::new());
        if self.extract_parts(i, &mut mem).is_err() {
            return;
        }
        self.registry.extend(gog_script_registry(&mem.0));
    }

    fn parse(mut file: Box<dyn ReadSeek>, slices: Vec<Box<dyn ReadSeek>>) -> Result<Self> {
        let (header_offset, data_offset) = find_offset_table(&mut file)?;
        let mut r = BufReader::new(&mut file);
        r.seek(SeekFrom::Start(header_offset))?;
        let mut version = [0u8; 64];
        r.read_exact(&mut version)?;
        let vend = version.iter().position(|&c| c == 0).unwrap_or(64);
        let Some(&(_, ver)) = SUPPORTED_VERSIONS
            .iter()
            .find(|(v, _)| *v == &version[..vend])
        else {
            return Err(Error::Unsupported(format!(
                "Inno Setup data version {:?} (only 5.5.0 and 5.6.2 unicode)",
                String::from_utf8_lossy(&version[..vend])
            )));
        };
        let block1 = read_block(&mut r)?;
        let block2 = read_block(&mut r)?;
        drop(r);

        let mut p = P { b: &block1, o: 0 };
        let (info, counts, compression) = parse_header(&mut p, ver)?;
        let dir_names = skip_entries(&mut p, &counts)?;
        let directories: Vec<String> = dir_names
            .iter()
            .filter_map(|d| strip_app(d))
            .map(|d| d.trim_end_matches('\\').replace('\\', "/"))
            .filter(|d| !d.is_empty())
            .collect();
        let mut files = Vec::with_capacity(counts.file as usize);
        for _ in 0..counts.file {
            files.push(parse_file_entry(&mut p)?);
        }

        skip_icons_and_ini(&mut p, &counts)?;
        let mut registry = Vec::with_capacity(counts.registry as usize);
        for _ in 0..counts.registry {
            registry.push(parse_registry_entry(&mut p)?);
        }

        let mut q = P { b: &block2, o: 0 };
        let mut data = Vec::with_capacity(counts.data_entry as usize);
        for _ in 0..counts.data_entry {
            data.push(parse_data_entry(&mut q)?);
        }

        let mut entries = Vec::new();
        let mut location = Vec::new();
        let mut seen = BTreeMap::new();
        let mut add = |path: String, size: u64, src: Source| {
            if seen.insert(path.to_ascii_lowercase(), ()).is_some() {
                return; // same destination listed twice (conditional variants): keep the first
            }
            entries.push(Entry { path, size });
            location.push(src);
        };
        // A Galaxy file being put together: path, parts expected, parts so far.
        let mut open: Option<(String, usize, Vec<Part>)> = None;
        let mut plain = Vec::new();
        let mut galaxy_files = false;
        for f in files {
            let Some(d) = data.get(f.location as usize) else {
                continue;
            };
            if let Some(part) = galaxy_part(&f.after) {
                let part = Part {
                    data: f.location as usize,
                    ..part
                };
                if Some(d.file_size) != galaxy_stored(&f.after) {
                    return format_err(format!("{}: part size disagrees", f.dest));
                }
                if let Some((path, count)) = galaxy_file(&f.before) {
                    if let Some((p, n, parts)) = open.take() {
                        return format_err(format!("{p}: {} of {n} parts", parts.len()));
                    }
                    open = Some((path, count, Vec::new()));
                }
                let Some((_, count, parts)) = open.as_mut() else {
                    return format_err(format!("{}: a part without its file", f.dest));
                };
                parts.push(part);
                if parts.len() == *count {
                    galaxy_files = true;
                    let (path, _, parts) = open.take().expect("open file");
                    let size = parts.iter().map(|p| p.size).sum();
                    add(path.replace('\\', "/"), size, Source::Parts(parts));
                }
                continue;
            }
            // Only files installed below {app}; {tmp}, {commonappdata} etc. are installer glue.
            let Some(rel) = strip_app(&f.dest) else {
                continue;
            };
            plain.push((rel.replace('\\', "/"), d.file_size, f.location as usize));
        }
        if let Some((p, n, parts)) = open {
            return format_err(format!("{p}: {} of {n} parts", parts.len()));
        }
        // Next to Galaxy files, the few files Inno installs itself (GOG client extras) go to
        // app/, as innoextract puts them, so they do not replace a Galaxy file of the same name.
        let prefix = if galaxy_files { "app/" } else { "" };
        for (path, size, loc) in plain {
            add(format!("{prefix}{path}"), size, Source::Data(loc));
        }

        if data_offset != 0 && !slices.is_empty() {
            return format_err("slices given for an installer with embedded data");
        }
        Ok(Installer {
            file,
            data_offset,
            slices: slices.into_iter().map(Some).collect(),
            slices_per_disk: counts.slices_per_disk.max(1),
            info: SetupInfo {
                compression,
                ..info
            },
            entries,
            registry,
            directories,
            location,
            data,
        })
    }

    /// The installer's registry entries, in the order it writes them.
    pub fn registry(&self) -> &[RegistryEntry] {
        &self.registry
    }

    pub fn info(&self) -> &SetupInfo {
        &self.info
    }

    /// Decompress one chunk and route its bytes to the selected files.
    fn extract_chunk(
        &mut self,
        chunk_offset: u32,
        wanted: &[(usize, Vec<String>)], // (data entry, destinations)
        sink: &mut dyn Sink,
    ) -> Result<()> {
        let first = &self.data[wanted[0].0];
        if first.flags & DATA_FLAG_ENCRYPTED != 0 {
            return Err(Error::Unsupported("encrypted installer".into()));
        }
        let compressed = first.flags & DATA_FLAG_COMPRESSED != 0;
        let chunk_size = first.chunk_size;
        let (first_slice, last_slice) = (first.first_slice, first.last_slice);
        let (file, start): (&mut dyn ReadSeek, u64) = if self.data_offset != 0 {
            if first_slice != 0 || last_slice != 0 {
                return format_err("embedded data entry in a slice other than the first");
            }
            (&mut *self.file, self.data_offset + u64::from(chunk_offset))
        } else {
            if last_slice != first_slice {
                return Err(Error::Unsupported(
                    "a chunk that spans two slice files".into(),
                ));
            }
            let f = self
                .slices
                .get_mut(first_slice as usize)
                .and_then(Option::as_mut)
                .ok_or_else(|| {
                    Error::Format(format!(
                        "slice file {} of the installer is missing",
                        slice_name("setup", first_slice, self.slices_per_disk)
                            .trim_start_matches("setup")
                    ))
                })?;
            let mut head = [0u8; 12];
            f.seek(SeekFrom::Start(0))?;
            f.read_exact(&mut head)?;
            if &head[..8] != SLICE_MAGIC {
                return format_err(format!("slice {first_slice}: no slice header"));
            }
            (&mut **f, u64::from(chunk_offset))
        };

        let mut magic = [0u8; 4];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut magic)?;
        if &magic != CHUNK_MAGIC {
            return format_err(format!("no chunk magic at {start:#x}"));
        }

        let mut targets = Vec::new();
        for (d, dests) in wanted {
            let de = &self.data[*d];
            let mut paths = Vec::new();
            for p in dests {
                crate::safe_relative(p)?;
                paths.push(p.clone());
            }
            let filtered = de.flags & DATA_FLAG_CALL_FILTER != 0;
            targets.push(Target {
                start: de.file_offset,
                end: de.file_offset + de.file_size,
                // Kept whole when the filter needs look-ahead or the data goes to more files.
                in_memory: filtered || paths.len() > 1,
                filtered,
                mem: Vec::new(),
                hasher: Sha1::new(),
                fed: 0,
                sha1: de.sha1,
                paths,
                begun: false,
            });
        }
        targets.sort_by_key(|t| t.start);
        let needed = targets.iter().map(|t| t.end).max().unwrap_or(0);

        let input = SubReader::new(file, start + 4, chunk_size);
        let mut router = Router {
            pos: 0,
            targets,
            first: 0,
            sink,
            error: None,
        };
        let method = if compressed {
            self.info.compression
        } else {
            COMPRESSION_STORED
        };
        match method {
            COMPRESSION_STORED => {
                std::io::copy(&mut input.take(needed), &mut router)?;
            }
            COMPRESSION_LZMA1 => {
                let mut input = input;
                crate::lzma::decode_lzma1(&mut input, &mut router, Some(needed))
                    .map_err(|e| Error::Format(format!("LZMA chunk at {start:#x}: {e}")))?;
            }
            COMPRESSION_LZMA2 => {
                let mut input = input;
                crate::lzma::decode_lzma2(&mut input, &mut router, Some(needed))
                    .map_err(|e| Error::Format(format!("LZMA2 chunk at {start:#x}: {e}")))?;
            }
            m => {
                return Err(Error::Unsupported(format!(
                    "Inno chunk compression method {m}"
                )))
            }
        }
        if let Some(e) = router.error.take() {
            return Err(e);
        }
        let Router { targets, sink, .. } = router;
        for t in targets {
            t.finish(sink)?;
        }
        Ok(())
    }
}

impl Installer {
    /// The stored bytes of data entry `d` (SHA-1 checked).
    fn stored(&mut self, d: usize) -> Result<Vec<u8>> {
        let mut mem = MemSink(Vec::new());
        let chunk = self.data[d].chunk_offset;
        self.extract_chunk(chunk, &[(d, vec!["part".into()])], &mut mem)?;
        Ok(mem.0)
    }

    /// Put a Galaxy file together from its parts, part by part into the sink.
    fn extract_parts(&mut self, i: usize, sink: &mut dyn Sink) -> Result<()> {
        let Source::Parts(parts) = self.location[i].clone() else {
            return Ok(());
        };
        let path = self.entries[i].path.clone();
        crate::safe_relative(&path)?;
        sink.begin(&path, self.entries[i].size)?;
        for part in parts {
            let stored = self.stored(part.data)?;
            let mut got = Vec::with_capacity(part.size as usize);
            flate2::read::ZlibDecoder::new(&stored[..])
                .take(part.size + 1)
                .read_to_end(&mut got)
                .map_err(|e| Error::Format(format!("{path}: part: {e}")))?;
            if got.len() as u64 != part.size {
                return Err(Error::Checksum(format!(
                    "{path}: a part of {} bytes, {} expected",
                    got.len(),
                    part.size
                )));
            }
            sink.write(&got)?;
        }
        sink.end()
    }
}

/// Collects one file in memory.
struct MemSink(Vec<u8>);

impl Sink for MemSink {
    fn dir(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn begin(&mut self, _: &str, size: u64) -> Result<()> {
        self.0 = Vec::with_capacity(size as usize);
        Ok(())
    }
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.0.extend_from_slice(data);
        Ok(())
    }
    fn end(&mut self) -> Result<()> {
        Ok(())
    }
}

/// The `setRegistry` actions of a GOG script (JSON) as registry entries.
fn gog_script_registry(json: &[u8]) -> Vec<RegistryEntry> {
    let text = String::from_utf8_lossy(json);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in v["actions"].as_array().into_iter().flatten() {
        let install = &a["install"];
        if install["action"] != "setRegistry" || a.get("os").is_some() {
            continue;
        }
        let args = &install["arguments"];
        let root = match args["root"].as_str() {
            Some("HKEY_LOCAL_MACHINE") => "HKLM",
            Some("HKEY_CURRENT_USER") => "HKCU",
            Some("HKEY_CLASSES_ROOT") => "HKCR",
            Some("HKEY_USERS") => "HKU",
            _ => continue,
        };
        let Some(key) = args["subkey"].as_str() else {
            continue;
        };
        // Uninstall information and program paths are not the game's concern.
        if key.contains("\\CurrentVersion\\") {
            continue;
        }
        let kind = match args["valueType"].as_str() {
            None if args.get("valueName").is_none() => RegistryValue::None,
            Some("string") => RegistryValue::String,
            Some("expandString") => RegistryValue::ExpandString,
            Some("dword") => RegistryValue::Dword,
            Some("qword") => RegistryValue::Qword,
            Some("binary") => RegistryValue::Binary,
            Some("multiString") => RegistryValue::MultiString,
            _ => continue,
        };
        out.push(RegistryEntry {
            root,
            key: key.to_string(),
            name: args["valueName"].as_str().unwrap_or("").to_string(),
            kind,
            value: args["valueData"].as_str().unwrap_or("").to_string(),
            has_check: false,
            deletes: false,
        });
    }
    out
}

/// The quoted and bare arguments of a script call `name(a, 'b', …)` in a condition (Pascal
/// strings: `''` is a quote).
fn script_call(cond: &str, name: &str) -> Option<Vec<String>> {
    let rest = cond
        .trim()
        .strip_prefix(name)?
        .trim_start()
        .strip_prefix('(')?;
    let inner = rest.trim_end().strip_suffix(')')?;
    let mut args = Vec::new();
    let mut s = inner;
    loop {
        s = s.trim_start();
        if let Some(mut q) = s.strip_prefix('\'') {
            let mut arg = String::new();
            loop {
                let end = q.find('\'')?;
                arg.push_str(&q[..end]);
                q = &q[end + 1..];
                match q.strip_prefix('\'') {
                    Some(r) => {
                        arg.push('\'');
                        q = r;
                    }
                    None => break,
                }
            }
            args.push(arg);
            s = q;
        } else {
            let end = s.find(',').unwrap_or(s.len());
            args.push(s[..end].trim().to_string());
            s = &s[end..];
        }
        s = s.trim_start();
        match s.strip_prefix(',') {
            Some(r) => s = r,
            None if s.is_empty() => return Some(args),
            None => return None,
        }
    }
}

/// `after_install('<md5>', <stored>, <size>)` → the part (data entry filled in later).
fn galaxy_part(after: &str) -> Option<Part> {
    let a = script_call(after, "after_install")
        .or_else(|| script_call(after, "after_install_dependency"))?;
    let size: u64 = a.get(2)?.parse().ok()?;
    Some(Part { data: 0, size })
}

fn galaxy_stored(after: &str) -> Option<u64> {
    let a = script_call(after, "after_install")
        .or_else(|| script_call(after, "after_install_dependency"))?;
    a.get(1)?.parse().ok()
}

/// `before_install('<md5>', '<path>', <parts>)` → (path, parts).
fn galaxy_file(before: &str) -> Option<(String, usize)> {
    let a = script_call(before, "before_install")
        .or_else(|| script_call(before, "before_install_dependency"))?;
    let count: usize = a.get(2)?.parse().ok()?;
    (count > 0 && !a.get(1)?.is_empty()).then(|| (a[1].clone(), count))
}

impl Archive for Installer {
    fn registry(&self) -> &[RegistryEntry] {
        &self.registry
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn directories(&self) -> &[String] {
        &self.directories
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
            crate::safe_relative(d)?;
            sink.dir(d)?;
        }
        // Group the selected entries by chunk, then by data entry, so each chunk is
        // decompressed once and identical data is written to all its destinations.
        let mut by_chunk: BTreeMap<u32, BTreeMap<usize, Vec<String>>> = BTreeMap::new();
        let mut galaxy = Vec::new();
        let mut n = 0;
        for (i, e) in self.entries.iter().enumerate() {
            if !select(e) {
                continue;
            }
            match &self.location[i] {
                Source::Data(d) => by_chunk
                    .entry(self.data[*d].chunk_offset)
                    .or_default()
                    .entry(*d)
                    .or_default()
                    .push(e.path.clone()),
                Source::Parts(_) => galaxy.push(i),
            }
            n += 1;
        }
        for (chunk, files) in by_chunk {
            let wanted: Vec<(usize, Vec<String>)> = files.into_iter().collect();
            self.extract_chunk(chunk, &wanted, sink)?;
        }
        for i in galaxy {
            self.extract_parts(i, sink)?;
        }
        Ok(n)
    }
}

struct Target {
    start: u64,
    end: u64,
    filtered: bool,
    in_memory: bool,
    /// The whole file when `in_memory`.
    mem: Vec<u8>,
    hasher: Sha1,
    fed: u64,
    sha1: [u8; 20],
    /// Where the data goes (more than one when the installer stores identical files once).
    paths: Vec<String>,
    begun: bool,
}

impl Target {
    fn feed(&mut self, b: &[u8], sink: &mut dyn Sink) -> Result<()> {
        self.fed += b.len() as u64;
        if self.in_memory {
            self.mem.extend_from_slice(b);
            return Ok(());
        }
        if !self.begun {
            sink.begin(&self.paths[0], self.end - self.start)?;
            self.begun = true;
        }
        self.hasher.update(b);
        sink.write(b)?;
        // Done once its last byte is in: the next file follows.
        if self.fed == self.end - self.start {
            sink.end()?;
        }
        Ok(())
    }

    fn finish(mut self, sink: &mut dyn Sink) -> Result<()> {
        if self.fed != self.end - self.start {
            return Err(Error::Checksum(format!(
                "{}: got {} of {} bytes",
                self.paths[0],
                self.fed,
                self.end - self.start
            )));
        }
        if self.in_memory {
            if self.filtered {
                unfilter_calls(&mut self.mem);
            }
            self.hasher.update(&self.mem);
        }
        let digest: [u8; 20] = self.hasher.finalize().into();
        if digest != self.sha1 {
            return Err(Error::Checksum(format!("{}: SHA-1", self.paths[0])));
        }
        if self.in_memory {
            for p in &self.paths {
                sink.begin(p, self.mem.len() as u64)?;
                sink.write(&self.mem)?;
                sink.end()?;
            }
        } else if !self.begun {
            // an empty file
            sink.begin(&self.paths[0], 0)?;
            sink.end()?;
        }
        Ok(())
    }
}

/// Write sink over a decompressed chunk that hands each file its byte range.
struct Router<'s> {
    pos: u64,
    targets: Vec<Target>,
    first: usize,
    sink: &'s mut dyn Sink,
    /// The sink's error (Write can only carry an io::Error).
    error: Option<Error>,
}

impl Write for Router<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let (lo, hi) = (self.pos, self.pos + buf.len() as u64);
        while self.first < self.targets.len() && self.targets[self.first].end <= lo {
            self.first += 1;
        }
        for t in &mut self.targets[self.first..] {
            if t.start >= hi {
                break;
            }
            let s = t.start.max(lo);
            let e = t.end.min(hi);
            if s < e {
                if let Err(e) = t.feed(&buf[(s - lo) as usize..(e - lo) as usize], self.sink) {
                    self.error = Some(e);
                    return Err(std::io::Error::other("sink"));
                }
            }
        }
        self.pos = hi;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reverse Inno Setup's x86 CALL/JMP address transform (5.2.0+ variant with the
/// high-byte flip). The transform works in 64 KiB blocks relative to the file start;
/// an instruction whose 4-byte operand would cross a block or the file end is untouched.
pub fn unfilter_calls(buf: &mut [u8]) {
    let n = buf.len();
    let mut i = 0usize;
    while i < n {
        let op = buf[i];
        if op != 0xe8 && op != 0xe9 {
            i += 1;
            continue;
        }
        let in_block = i % 0x10000;
        let block_len = (n - (i - in_block)).min(0x10000);
        if in_block + 4 >= block_len {
            // Operand would cross the end of the 64 KiB block (or the file): the encoder
            // stopped scanning this block here.
            i = i - in_block + block_len;
            continue;
        }
        let a = i + 1;
        if buf[a + 3] == 0x00 || buf[a + 3] == 0xff {
            let addr = ((a + 4) as u32) & 0x00ff_ffff;
            let rel =
                (u32::from(buf[a]) | u32::from(buf[a + 1]) << 8 | u32::from(buf[a + 2]) << 16)
                    .wrapping_sub(addr);
            if rel & 0x0080_0000 != 0 {
                buf[a + 3] = !buf[a + 3];
            }
            buf[a] = rel as u8;
            buf[a + 1] = (rel >> 8) as u8;
            buf[a + 2] = (rel >> 16) as u8;
        }
        i = a + 4;
    }
}

/// Forward transform (what the Inno compiler does). Only used by the tests.
#[cfg(test)]
fn filter_calls(buf: &mut [u8]) {
    let n = buf.len();
    let mut i = 0usize;
    while i < n {
        let op = buf[i];
        if op != 0xe8 && op != 0xe9 {
            i += 1;
            continue;
        }
        let in_block = i % 0x10000;
        let block_len = (n - (i - in_block)).min(0x10000);
        if in_block + 4 >= block_len {
            i = i - in_block + block_len;
            continue;
        }
        let a = i + 1;
        if buf[a + 3] == 0x00 || buf[a + 3] == 0xff {
            let addr = ((a + 4) as u32) & 0x00ff_ffff;
            let rel = u32::from(buf[a]) | u32::from(buf[a + 1]) << 8 | u32::from(buf[a + 2]) << 16;
            if rel & 0x0080_0000 != 0 {
                buf[a + 3] = !buf[a + 3];
            }
            let enc = rel.wrapping_add(addr);
            buf[a] = enc as u8;
            buf[a + 1] = (enc >> 8) as u8;
            buf[a + 2] = (enc >> 16) as u8;
        }
        i = a + 4;
    }
}

/// The file name of slice `i` (0-based) after the installer's stem: `stem-1.bin`, or with
/// several slices per disk `stem-1a.bin`, `stem-1b.bin`, ….
fn slice_name(stem: &str, i: u32, per_disk: u32) -> String {
    if per_disk <= 1 {
        format!("{stem}-{}.bin", i + 1)
    } else {
        let letter = char::from(b'a' + (i % per_disk) as u8);
        format!("{stem}-{}{letter}.bin", i / per_disk + 1)
    }
}

/// `{app}\rest` → `rest` (case-insensitive), anything else → None.
fn strip_app(dest: &str) -> Option<&str> {
    let head = dest.get(..6)?;
    if head.eq_ignore_ascii_case("{app}\\") && dest.len() > 6 {
        Some(&dest[6..])
    } else {
        None
    }
}

/// Locate the setup loader's offset table. Returns (setup-0 offset, setup-1 offset).
fn find_offset_table(f: &mut dyn ReadSeek) -> Result<(u64, u64)> {
    let mut head = Vec::new();
    f.seek(SeekFrom::Start(0))?;
    f.take(16 << 20).read_to_end(&mut head)?;
    for magic in &LOADER_MAGICS {
        let mut from = 0;
        while let Some(rel) = head[from..].windows(12).position(|w| w == magic) {
            let at = from + rel;
            from = at + 1;
            let Some(t) = head.get(at..at + 44) else {
                break;
            };
            let word = |i: usize| u32::from_le_bytes(t[i..i + 4].try_into().expect("4 bytes"));
            if word(12) != 1 || crc32fast::hash(&t[..40]) != word(40) {
                continue; // revision 1 and a valid table CRC, else it's a false hit
            }
            return Ok((u64::from(word(32)), u64::from(word(36))));
        }
    }
    format_err("no Inno Setup loader offset table (not an Inno installer, or too old/new)")
}

/// Read one setup-0 block: header, CRC-checked sub-blocks, LZMA1 decompression.
fn read_block(r: &mut impl Read) -> Result<Vec<u8>> {
    let mut h = [0u8; 9];
    r.read_exact(&mut h)?;
    let crc = u32::from_le_bytes(h[0..4].try_into().expect("4 bytes"));
    if crc32fast::hash(&h[4..9]) != crc {
        return Err(Error::Checksum("setup block header".into()));
    }
    let stored = u32::from_le_bytes(h[4..8].try_into().expect("4 bytes")) as usize;
    let compressed = h[8] != 0;
    let mut raw = vec![0u8; stored];
    r.read_exact(&mut raw)?;
    let mut payload = Vec::with_capacity(stored);
    let mut rest = &raw[..];
    while !rest.is_empty() {
        if rest.len() < 5 {
            return format_err("truncated setup sub-block");
        }
        let crc = u32::from_le_bytes(rest[0..4].try_into().expect("4 bytes"));
        let n = (rest.len() - 4).min(4096);
        let chunk = &rest[4..4 + n];
        if crc32fast::hash(chunk) != crc {
            return Err(Error::Checksum("setup sub-block".into()));
        }
        payload.extend_from_slice(chunk);
        rest = &rest[4 + n..];
    }
    if !compressed {
        return Ok(payload);
    }
    // The stream has no end marker and no stored size: decode until the input runs out.
    let mut out = Vec::new();
    crate::lzma::decode_lzma1(&mut &payload[..], &mut out, None)?;
    Ok(out)
}

fn parse_header(p: &mut P<'_>, ver: Version) -> Result<(SetupInfo, Counts, u8)> {
    let app_name = p.wstr()?;
    p.skip_strings(8)?; // versioned name, id, copyright, publisher, url, phone, support, updates
    let app_version = p.wstr()?;
    // default dir, group, base filename, uninstall files dir, name, icon, mutex,
    // default user/organisation/serial, readme, contact, comments, modify path,
    // create uninstall key, uninstallable, close applications filter,
    // (5.6: setup mutex, then changes environment and changes associations, which became
    // expressions instead of flags), license/info before/info after (ANSI), compiled code
    p.skip_strings(if ver >= (5, 6, 0) { 24 } else { 21 })?;
    let mut c = [0u32; 16];
    for v in &mut c {
        *v = p.u32()?;
    }
    let mut counts = Counts {
        slices_per_disk: 1,
        language: c[0],
        message: c[1],
        permission: c[2],
        kind: c[3],
        component: c[4],
        task: c[5],
        directory: c[6],
        file: c[7],
        data_entry: c[8],
        icon: c[9],
        ini: c[10],
        registry: c[11],
    };
    p.skip(WINVER_RANGE)?;
    if ver >= (5, 5, 7) {
        // back colour, back colour 2; the image back colour gave way to the image alpha format
        p.skip(4 * 2 + 1)?;
    } else {
        p.skip(4 * 3)?; // back colour, back colour 2, image back colour
    }
    p.skip(20 + 8)?; // password SHA-1 + salt
    p.skip(8)?; // extra disk space
    counts.slices_per_disk = p.u32()?;
    // uninstall log mode, dir exists warning, privileges, language dialog, detection
    p.skip(5)?;
    let compression = p.u8()?;
    if compression > 4 {
        return format_err(format!(
            "unknown compression {compression} (header misparsed?)"
        ));
    }
    p.skip(2)?; // architectures allowed / installed in 64-bit mode
    p.skip(2)?; // disable dir page, disable program group page
    p.skip(8)?; // uninstall display size
    p.skip(6)?; // option flags (46 bits)
    let info = SetupInfo {
        app_name,
        app_version,
        compression,
        file_count: counts.file as usize,
        data_entry_count: counts.data_entry as usize,
    };
    Ok((info, counts, compression))
}

/// Skip languages, messages, permissions, types, components and tasks; return the
/// directory entry names.
fn skip_entries(p: &mut P<'_>, c: &Counts) -> Result<Vec<String>> {
    for _ in 0..c.language {
        p.skip_strings(10)?;
        p.skip(4 * 5 + 1)?;
    }
    for _ in 0..c.message {
        p.skip_strings(2)?;
        p.skip(4)?;
    }
    for _ in 0..c.permission {
        p.skip_strings(1)?;
    }
    for _ in 0..c.kind {
        p.skip_strings(4)?;
        p.skip(WINVER_RANGE + 1 + 1 + 8)?;
    }
    for _ in 0..c.component {
        p.skip_strings(5)?;
        p.skip(8 + 4 + 1 + WINVER_RANGE + 1 + 8)?;
    }
    for _ in 0..c.task {
        p.skip_strings(6)?;
        p.skip(4 + 1 + WINVER_RANGE + 1)?;
    }
    let mut dirs = Vec::with_capacity(c.directory as usize);
    for _ in 0..c.directory {
        dirs.push(p.wstr()?);
        p.skip_strings(ITEM_CONDITIONS)?;
        p.skip(4 + WINVER_RANGE + 2 + 1)?;
    }
    Ok(dirs)
}

/// A `[Files]` entry: destination, data entry index and the script conditions.
fn parse_file_entry(p: &mut P<'_>) -> Result<FileEntry> {
    p.skip_strings(1)?; // source
    let dest = p.wstr()?;
    // font name, assembly name, components, tasks, languages, check (GOG: languages and
    // systems a file is for; the first entry of a destination is kept)
    p.skip_strings(6)?;
    let after = p.wstr()?;
    let before = p.wstr()?;
    p.skip(WINVER_RANGE)?;
    let location = p.u32()?;
    p.skip(4 + 8 + 2 + 4 + 1)?; // attributes, external size, permission, flags, type
    Ok(FileEntry {
        dest,
        location,
        after,
        before,
    })
}

/// Icons (shortcuts) and INI entries: skipped.
fn skip_icons_and_ini(p: &mut P<'_>, c: &Counts) -> Result<()> {
    for _ in 0..c.icon {
        // name, file, parameters, working dir, icon file, comment; conditions; app user model id
        p.skip_strings(6 + ITEM_CONDITIONS + 1)?;
        p.skip(WINVER_RANGE)?;
        p.skip(4 + 4 + 1 + 2 + 1)?; // icon index, show command, close on exit, hot key, flags
    }
    for _ in 0..c.ini {
        p.skip_strings(4 + ITEM_CONDITIONS)?; // file, section, key, value; conditions
        p.skip(WINVER_RANGE + 1)?; // flags
    }
    Ok(())
}

fn parse_registry_entry(p: &mut P<'_>) -> Result<RegistryEntry> {
    let key = p.wstr()?;
    let name = p.wstr()?;
    let value = p.bytes()?;
    p.skip_strings(3)?; // components, tasks, languages
    let check = p.bytes()?;
    p.skip_strings(2)?; // after install, before install
    p.skip(WINVER_RANGE)?;
    let hive = p.u32()?;
    p.skip(2)?; // permission index
    let kind = p.u8()?;
    let flags = p.u16()?;
    let root = match hive & !0x8000_0000 {
        0 => "HKCR",
        1 => "HKCU",
        2 => "HKLM",
        3 => "HKU",
        5 => "HKCC",
        h => return format_err(format!("registry entry with unknown root {h} (misparsed?)")),
    };
    let kind = match kind {
        0 => RegistryValue::None,
        1 => RegistryValue::String,
        2 => RegistryValue::ExpandString,
        3 => RegistryValue::Dword,
        4 => RegistryValue::Binary,
        5 => RegistryValue::MultiString,
        6 => RegistryValue::Qword,
        k => return format_err(format!("registry entry of unknown type {k} (misparsed?)")),
    };
    // The value is UTF-16 for strings; numbers and binary data are written as text too.
    let units: Vec<u16> = value
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    Ok(RegistryEntry {
        root,
        key,
        name,
        kind,
        value: String::from_utf16_lossy(&units),
        has_check: !check.is_empty(),
        // flags: create value if missing, uninstall delete value, uninstall clear value,
        // uninstall delete key, uninstall delete key if empty, preserve string type,
        // delete key, delete value, …
        deletes: flags & (1 << 6 | 1 << 7) != 0,
    })
}

fn parse_data_entry(p: &mut P<'_>) -> Result<DataEntry> {
    let first_slice = p.u32()?;
    let last_slice = p.u32()?;
    let chunk_offset = p.u32()?;
    let file_offset = p.u64()?;
    let file_size = p.u64()?;
    let chunk_size = p.u64()?;
    let sha1: [u8; 20] = p.take(20)?.try_into().expect("20 bytes");
    p.skip(8 + 4 + 4)?; // timestamp, file version
    let flags = p.u16()?;
    Ok(DataEntry {
        first_slice,
        last_slice,
        chunk_offset,
        file_offset,
        file_size,
        chunk_size,
        sha1,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_filter_round_trip() {
        // Pseudo-random code-like bytes with many E8/E9 opcodes, crossing 64 KiB blocks.
        let mut x = 0x1234_5678u32;
        let mut data: Vec<u8> = (0..200_003)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                match i % 7 {
                    0 => 0xe8,
                    3 => 0x00,
                    5 => 0xff,
                    _ => x as u8,
                }
            })
            .collect();
        // Opcodes right before a block boundary and at the end must stay untouched.
        data[0xfffc] = 0xe8;
        let n = data.len();
        data[n - 3] = 0xe9;
        let orig = data.clone();
        filter_calls(&mut data);
        assert_ne!(data, orig);
        unfilter_calls(&mut data);
        assert_eq!(data, orig);
    }

    #[test]
    fn unfilter_known_vector() {
        // call rel32 = 0x000010 at offset 0: encoded as (0x10 + 5) = 0x15.
        let mut b = [0xe8, 0x15, 0x00, 0x00, 0x00, 0x90];
        unfilter_calls(&mut b);
        assert_eq!(b, [0xe8, 0x10, 0x00, 0x00, 0x00, 0x90]);
        // Negative displacement: rel = -0x10 → flip of the high byte.
        let mut b = [0xe8, 0xf5, 0xff, 0xff, 0x00, 0x90];
        unfilter_calls(&mut b);
        assert_eq!(b, [0xe8, 0xf0, 0xff, 0xff, 0xff, 0x90]);
    }

    #[test]
    fn gog_script_actions() {
        let json = br#"{"actions": [
            {"install": {"action": "setRegistry", "arguments": {"root": "HKEY_LOCAL_MACHINE",
              "subkey": "Software\\X\\1.0", "valueData": "{app}", "valueName": "AppPath",
              "valueType": "string"}}, "languages": ["*"]},
            {"install": {"action": "setRegistry", "arguments": {"root": "HKEY_CURRENT_USER",
              "subkey": "Software\\X", "valueData": "$00000001", "valueName": "full_screen",
              "valueType": "dword"}}, "os": ["win10"]},
            {"install": {"action": "setRegistry", "arguments": {"root": "HKEY_LOCAL_MACHINE",
              "subkey": "Software\\X"}}},
            {"install": {"action": "savePath", "arguments": {}}}
        ]}"#;
        let r = gog_script_registry(json);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].root, r[0].key.as_str()), ("HKLM", "Software\\X\\1.0"));
        assert_eq!(
            (r[0].name.as_str(), r[0].value.as_str()),
            ("AppPath", "{app}")
        );
        assert_eq!(r[0].kind, RegistryValue::String);
        assert_eq!(r[1].kind, RegistryValue::None);
    }

    #[test]
    fn galaxy_conditions() {
        let p = galaxy_part("after_install('69009fff6ddf9e4a788e539c5d62281a', 117225, 261120)")
            .unwrap();
        assert_eq!(p.size, 261120);
        assert_eq!(galaxy_stored("after_install('x', 5, 6)"), Some(5));
        assert_eq!(
            galaxy_file("before_install('cb65', 'Data\\music.h4r', 7)"),
            Some(("Data\\music.h4r".into(), 7))
        );
        assert_eq!(
            galaxy_file("before_install_dependency('14af', '__redist\\ISI\\s.exe', 1)"),
            Some(("__redist\\ISI\\s.exe".into(), 1))
        );
        assert_eq!(galaxy_file(""), None);
        assert_eq!(
            script_call("check_if_install('de-DE#','32#64#','')", "check_if_install"),
            Some(vec!["de-DE#".to_string(), "32#64#".into(), String::new()])
        );
        assert_eq!(
            script_call("f(1, 'a,b' , x)", "f"),
            Some(vec!["1".into(), "a,b".into(), "x".into()])
        );
        assert_eq!(
            galaxy_file("before_install('73c0', 'maps\\The Dragon''s Wrath.h4c', 1)"),
            Some(("maps\\The Dragon's Wrath.h4c".into(), 1))
        );
    }

    #[test]
    fn slice_names() {
        assert_eq!(slice_name("setup_x", 0, 1), "setup_x-1.bin");
        assert_eq!(slice_name("setup_x", 1, 1), "setup_x-2.bin");
        assert_eq!(slice_name("s", 0, 2), "s-1a.bin");
        assert_eq!(slice_name("s", 3, 2), "s-2b.bin");
    }

    #[test]
    fn app_paths() {
        assert_eq!(strip_app("{app}\\DataX\\a.lib"), Some("DataX\\a.lib"));
        assert_eq!(strip_app("{APP}\\Game.exe"), Some("Game.exe"));
        assert_eq!(strip_app("{tmp}\\x.dll"), None);
        assert_eq!(strip_app("{app}\\"), None);
    }
}

/// The registry entries of every Inno installer in `orig/` parse into plausible entries.
#[cfg(test)]
mod orig_tests {
    use super::*;

    #[test]
    #[ignore = "needs the installers in orig/"]
    fn registry_entries_of_the_installers() {
        let orig = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../orig");
        let mut seen = 0;
        for e in std::fs::read_dir(&orig).expect("orig/") {
            let path = e.expect("entry").path();
            if path.extension().is_none_or(|x| x != "exe") {
                continue;
            }
            let inst = Installer::open(&path).expect("installer");
            for r in inst.registry() {
                assert!(
                    r.key.chars().all(|c| !c.is_control()),
                    "{}: key {:?}",
                    path.display(),
                    r.key
                );
                assert!(
                    r.name.chars().all(|c| !c.is_control()),
                    "{}: name {:?}",
                    path.display(),
                    r.name
                );
                if std::env::var_os("RC_SHOW_REGISTRY").is_some() {
                    let v: String = r
                        .value
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { 'x' } else { c })
                        .collect();
                    println!(
                        "{} {}\\{} [{}] {:?} {} chk={} del={}",
                        path.file_name().unwrap().to_string_lossy(),
                        r.root,
                        r.key,
                        r.name,
                        r.kind,
                        v,
                        r.has_check,
                        r.deletes
                    );
                }
            }
            seen += 1;
        }
        assert!(seen > 0);
    }
}
