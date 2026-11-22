// SPDX-License-Identifier: MIT
//! Readers for the media the games ship on, without third-party tools:
//!
//! - [`inno`]: GOG installers (Inno Setup 5.5.0 and 5.6.2 unicode, data in the exe or in slices)
//! - [`inno3`]: old Inno Setup installers (3.0.6.1, ANSI), as on a CD
//! - [`nsis`]: Nullsoft installers of the 1.9x era
//! - [`msi`]: Windows Installer packages with an external cabinet ([`cfb`], [`cab`])
//! - [`iso`]: ISO 9660 CD images (primary volume or Joliet), cooked or raw ([`rawcd`])
//! - [`iscab`]: InstallShield 5 cabinets (`DATA1.HDR` + `DATA1.CAB`), as on the Gold CD
//! - [`zip`]: zip archives of installed copies (stored or deflated)
//! - [`lzma`]: the streaming LZMA1/LZMA2 decoder the Inno reader uses
//!
//! Every reader lists its entries with paths relative to the install directory and
//! extracts them below a destination directory. Paths are sanitised; nothing is ever
//! written outside the destination.

pub mod cab;
pub mod cfb;
pub mod codepage;
pub mod inno;
pub mod inno3;
pub mod iscab;
pub mod iso;
pub mod lzma;
pub mod md5;
pub mod msi;
pub mod nsis;
pub mod rawcd;
pub mod zip;

use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The input is not what the reader expected.
    Format(String),
    /// A valid input using a feature this reader does not implement.
    Unsupported(String),
    /// Data failed an integrity check (CRC, SHA-1, size).
    Checksum(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::Format(s) => write!(f, "bad format: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Checksum(s) => write!(f, "checksum mismatch: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn format_err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Format(msg.into()))
}

/// One file of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to the install directory, `/`-separated, original case.
    pub path: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// Common interface of all readers.
pub trait Archive {
    fn entries(&self) -> &[Entry];

    /// Directories the medium creates explicitly (they may stay empty, e.g. the C1 map
    /// folder `Data_m`). Extraction creates them as well.
    fn directories(&self) -> &[String] {
        &[]
    }

    /// Extract every entry for which `select` returns true below `dest`.
    /// Returns the number of files written.
    fn extract(&mut self, dest: &Path, select: &dyn Fn(&Entry) -> bool) -> Result<usize>;

    /// Extract every entry for which `select` returns true to `sink`, one file after the other
    /// (the directories first). Returns the number of files written.
    fn extract_to(
        &mut self,
        sink: &mut dyn Sink,
        select: &dyn Fn(&Entry) -> bool,
    ) -> Result<usize> {
        let _ = (sink, select);
        Err(Error::Unsupported(
            "extracting this medium to a sink".into(),
        ))
    }

    /// The registry entries the installer writes (only Inno installers have them).
    fn registry(&self) -> &[inno::RegistryEntry] {
        &[]
    }
}

/// The installer's registry entries as the runtime keeps a registry (`registry.txt`: one line per
/// value, `KEY<TAB>NAME<TAB>TYPE<TAB>HEX`, an empty key as `KEY<TAB><TAB>-<TAB>`), for
/// `registry-install.txt` next to the unpacked game. Entries a `Check:` function guards and entries
/// that delete are left out; Inno constants such as `{app}` stay for the runtime to fill in.
pub fn registry_file(entries: &[inno::RegistryEntry]) -> String {
    use inno::RegistryValue as V;
    let mut out = String::new();
    for e in entries.iter().filter(|e| !e.has_check && !e.deletes) {
        let key = format!("{}\\{}", e.root, e.key);
        let ansi = |s: &str| -> Vec<u8> {
            let mut b: Vec<u8> = s
                .chars()
                .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
                .collect();
            b.push(0);
            b
        };
        let number = |s: &str| -> Option<u64> {
            let s = s.trim();
            match s.strip_prefix('$') {
                Some(h) => u64::from_str_radix(h, 16).ok(),
                None => s.parse().ok(),
            }
        };
        let (kind, data): (u32, Vec<u8>) = match e.kind {
            V::None => {
                out.push_str(&format!("{key}\t\t-\t\n"));
                continue;
            }
            V::String => (1, ansi(&e.value)),
            V::ExpandString => (2, ansi(&e.value)),
            V::Dword => match number(&e.value) {
                Some(n) => (4, (n as u32).to_le_bytes().to_vec()),
                None => continue, // a constant ({code:…}) that only the installer could work out
            },
            V::Qword => match number(&e.value) {
                Some(n) => (11, n.to_le_bytes().to_vec()),
                None => continue,
            },
            V::Binary => (
                3,
                e.value
                    .split_whitespace()
                    .filter_map(|b| u8::from_str_radix(b, 16).ok())
                    .collect(),
            ),
            V::MultiString => {
                let mut b = Vec::new();
                for part in e.value.split("{break}") {
                    b.extend(ansi(part));
                }
                b.push(0);
                (7, b)
            }
        };
        let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("{key}\t{}\t{kind}\t{hex}\n", e.name));
    }
    out
}

/// Turn an archive path into a safe relative path: splits on `/` and `\`, drops empty
/// and `.` components, rejects `..`, drive letters and absolute paths.
pub fn safe_relative(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return format_err(format!("path escapes the destination: {path}")),
            p if p.contains(':') => return format_err(format!("drive or stream in path: {path}")),
            p => out.push(p),
        }
    }
    if out.as_os_str().is_empty() || out.components().any(|c| !matches!(c, Component::Normal(_))) {
        return format_err(format!("empty or invalid path: {path:?}"));
    }
    Ok(out)
}

/// Create `dest/<entry path>` (with parent directories) for writing.
pub(crate) fn create_output(dest: &Path, path: &str) -> Result<fs::File> {
    let full = dest.join(safe_relative(path)?);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(fs::File::create(full)?)
}

/// Create every directory of `dirs` below `dest`.
pub(crate) fn create_dirs(dest: &Path, dirs: &[String]) -> Result<()> {
    for d in dirs {
        fs::create_dir_all(dest.join(safe_relative(d)?))?;
    }
    Ok(())
}

/// Copy exactly `len` bytes from `r` to `w`.
pub(crate) fn copy_exact(r: &mut dyn Read, w: &mut dyn Write, len: u64) -> Result<()> {
    let n = io::copy(&mut r.take(len), w)?;
    if n != len {
        return format_err(format!("unexpected end of data ({n} of {len} bytes)"));
    }
    Ok(())
}

/// A window `[start, start + len)` of an underlying seekable reader.
pub struct SubReader<R> {
    inner: R,
    start: u64,
    len: u64,
    pos: u64,
}

impl<R: Read + Seek> SubReader<R> {
    pub fn new(inner: R, start: u64, len: u64) -> Self {
        SubReader {
            inner,
            start,
            len,
            pos: 0,
        }
    }
}

impl<R: Read + Seek> Read for SubReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        if left == 0 {
            return Ok(0);
        }
        let n = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let got = self.inner.read(&mut buf[..n])?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl<R: Read + Seek> Seek for SubReader<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        match new {
            Some(p) => {
                self.pos = p;
                Ok(p)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            )),
        }
    }
}

/// Where extracted files go, one after the other: `begin`, its bytes in `write`s, `end`. Paths are
/// relative to the install directory, `/`-separated and already checked (`safe_relative`).
pub trait Sink {
    fn dir(&mut self, path: &str) -> Result<()>;
    fn begin(&mut self, path: &str, size: u64) -> Result<()>;
    fn write(&mut self, data: &[u8]) -> Result<()>;
    fn end(&mut self) -> Result<()>;
}

/// A sink that writes below a directory.
pub struct DirSink<'a> {
    pub dest: &'a Path,
    current: Option<io::BufWriter<fs::File>>,
}

impl<'a> DirSink<'a> {
    pub fn new(dest: &'a Path) -> Self {
        DirSink {
            dest,
            current: None,
        }
    }
}

impl Sink for DirSink<'_> {
    fn dir(&mut self, path: &str) -> Result<()> {
        fs::create_dir_all(self.dest.join(safe_relative(path)?))?;
        Ok(())
    }
    fn begin(&mut self, path: &str, _size: u64) -> Result<()> {
        self.current = Some(io::BufWriter::new(create_output(self.dest, path)?));
        Ok(())
    }
    fn write(&mut self, data: &[u8]) -> Result<()> {
        match &mut self.current {
            Some(w) => Ok(w.write_all(data)?),
            None => format_err("sink: write outside a file"),
        }
    }
    fn end(&mut self) -> Result<()> {
        if let Some(mut w) = self.current.take() {
            w.flush()?;
        }
        Ok(())
    }
}

/// One reader shared by several [`SharedReader`]s, each with its own position (several files of a
/// CD image open at once: an InstallShield cabinet's volumes).
#[derive(Clone)]
pub struct SharedReader {
    inner: std::rc::Rc<std::cell::RefCell<Box<dyn ReadSeek>>>,
    pos: u64,
}

impl SharedReader {
    pub fn new(r: Box<dyn ReadSeek>) -> Self {
        SharedReader {
            inner: std::rc::Rc::new(std::cell::RefCell::new(r)),
            pos: 0,
        }
    }
}

impl Read for SharedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut r = self.inner.borrow_mut();
        r.seek(SeekFrom::Start(self.pos))?;
        let n = r.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SharedReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                let len = self.inner.borrow_mut().seek(SeekFrom::End(0))?;
                len.checked_add_signed(d)
            }
        };
        self.pos = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

/// Anything the readers can read randomly from.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

pub(crate) fn le16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| Error::Format(format!("truncated at {o:#x}")))
}

pub(crate) fn le32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| Error::Format(format!("truncated at {o:#x}")))
}

/// NUL-terminated byte string at `o`.
pub(crate) fn cstr(b: &[u8], o: usize) -> Result<&[u8]> {
    let tail = b
        .get(o..)
        .ok_or_else(|| Error::Format(format!("string offset {o:#x} out of range")))?;
    let end = tail
        .iter()
        .position(|&c| c == 0)
        .ok_or_else(|| Error::Format(format!("unterminated string at {o:#x}")))?;
    Ok(&tail[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn safe_paths() {
        assert_eq!(
            safe_relative("Data_m\\a\\.\\b.map").unwrap(),
            Path::new("Data_m/a/b.map")
        );
        assert_eq!(safe_relative("/abs//x").unwrap(), Path::new("abs/x"));
        assert!(safe_relative("a/../../etc").is_err());
        assert!(safe_relative("C:\\x").is_err());
        assert!(safe_relative("").is_err());
        assert!(safe_relative("./").is_err());
    }

    #[test]
    fn sub_reader_window() {
        let data: Vec<u8> = (0..100).collect();
        let mut r = SubReader::new(Cursor::new(data), 10, 20);
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, (10..30).collect::<Vec<u8>>());
        r.seek(SeekFrom::Start(15)).unwrap();
        let mut two = [0u8; 2];
        r.read_exact(&mut two).unwrap();
        assert_eq!(two, [25, 26]);
        assert_eq!(r.seek(SeekFrom::End(-1)).unwrap(), 19);
        assert!(r.seek(SeekFrom::Current(-100)).is_err());
    }
}

/// What [`open`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    /// Inno Setup installer (GOG, or an old one from a CD).
    Inno,
    /// Nullsoft installer.
    Nsis,
    /// CD image with an InstallShield 5 or 6 setup; entries are the installed files.
    IsoInstallShield,
    /// CD image with an installer program in its root (Inno Setup or NSIS); entries are the
    /// installed files.
    IsoInstaller,
    /// CD image without a recognised setup; entries are the files on the disc.
    Iso,
    /// Zip archive of an installed copy.
    Zip,
    /// Zip archive of a CD with an installer program in its root; entries are the installed
    /// files.
    ZipInstaller,
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MediaType::Inno => "Inno Setup installer",
            MediaType::Nsis => "NSIS installer",
            MediaType::IsoInstallShield => "CD image (InstallShield setup)",
            MediaType::IsoInstaller => "CD image (setup program)",
            MediaType::Iso => "CD image",
            MediaType::Zip => "zip archive",
            MediaType::ZipInstaller => "zip archive (setup program)",
        })
    }
}

/// Open a medium by content: zip (`PK\x03\x04`), ISO 9660 (`CD001` at sector 16, cooked or as a
/// raw 2352-byte-sector `.bin`), or an installer exe (`MZ`: Inno Setup or NSIS). For a CD with
/// `DATA1.HDR`/`DATA1.CAB` in its root, the InstallShield cabinet is opened; for a CD with an
/// installer program in its root, that installer: either way the entries are the installed files.
/// External Inno slices beside an installer exe are opened by name.
pub fn open(path: &Path) -> Result<(MediaType, Box<dyn Archive>)> {
    let mut f = fs::File::open(path)?;
    let mut magic = [0u8; 4];
    let got = f.read(&mut magic)?;
    drop(f);
    if got >= 2 && magic[..2] == *b"MZ" {
        let mut r: Box<dyn ReadSeek> = Box::new(io::BufReader::new(fs::File::open(path)?));
        if nsis::find_first_header(&mut r).is_some() {
            return Ok((MediaType::Nsis, Box::new(nsis::Installer::from_reader(r)?)));
        }
        if inno3::is_old_inno(&mut r) {
            return Ok((MediaType::Inno, Box::new(inno3::Installer::from_reader(r)?)));
        }
        return Ok((MediaType::Inno, Box::new(inno::Installer::open(path)?)));
    }
    open_readers(
        Box::new(io::BufReader::new(fs::File::open(path)?)),
        Vec::new(),
    )
}

/// Open a medium from any reader (in a browser: the chosen file in pieces): see [`open`].
pub fn open_reader(r: Box<dyn ReadSeek>) -> Result<(MediaType, Box<dyn Archive>)> {
    open_readers(r, Vec::new())
}

/// [`open_reader`] for an Inno Setup installer whose data is in external slice files
/// (`setup_….exe` and its `-1.bin`, …), given in slice order.
pub fn open_readers(
    mut r: Box<dyn ReadSeek>,
    slices: Vec<Box<dyn ReadSeek>>,
) -> Result<(MediaType, Box<dyn Archive>)> {
    let mut magic = [0u8; 4];
    r.seek(SeekFrom::Start(0))?;
    let got = r.read(&mut magic)?;
    r.seek(SeekFrom::Start(0))?;
    if got >= 4 && magic == *b"PK\x03\x04" && slices.is_empty() {
        let mut z = zip::Zip::from_reader(r)?;
        // A zip of a CD whose installer program is in its root: the installer, read into memory.
        let mut exes: Vec<(u64, usize)> = z
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                !e.path.contains('/')
                    && e.path.to_ascii_lowercase().ends_with(".exe")
                    && e.size < 1 << 30
            })
            .map(|(i, e)| (e.size, i))
            .collect();
        exes.sort_by(|a, b| b.cmp(a));
        for (_, i) in exes {
            let data = z.read_entry(i)?;
            let mut c = io::Cursor::new(data);
            if nsis::find_first_header(&mut c).is_some() || inno3::is_old_inno(&mut c) {
                let (_, archive) = open_installer(Box::new(c), Vec::new())?;
                return Ok((MediaType::ZipInstaller, archive));
            }
        }
        return Ok((MediaType::Zip, Box::new(z)));
    }
    if got >= 2 && magic[..2] == *b"MZ" {
        return open_installer(r, slices);
    }
    if slices.is_empty() {
        if let Some(offset) = rawcd::detect(&mut r) {
            return open_cd(Box::new(rawcd::RawCd::new(r, offset)?));
        }
        let mut iso_magic = [0u8; 5];
        let is_iso = r.seek(SeekFrom::Start(16 * 2048 + 1)).is_ok()
            && r.read(&mut iso_magic).unwrap_or(0) == 5
            && &iso_magic == b"CD001";
        if is_iso {
            return open_cd(r);
        }
    }
    format_err("not a GOG installer, CD image, installer or zip")
}

/// An installer exe: NSIS, old Inno Setup (3.x) or a GOG Inno Setup installer with its slices.
fn open_installer(
    mut r: Box<dyn ReadSeek>,
    slices: Vec<Box<dyn ReadSeek>>,
) -> Result<(MediaType, Box<dyn Archive>)> {
    if slices.is_empty() {
        if nsis::find_first_header(&mut r).is_some() {
            return Ok((MediaType::Nsis, Box::new(nsis::Installer::from_reader(r)?)));
        }
        if inno3::is_old_inno(&mut r) {
            return Ok((MediaType::Inno, Box::new(inno3::Installer::from_reader(r)?)));
        }
    }
    Ok((
        MediaType::Inno,
        Box::new(inno::Installer::from_readers(r, slices)?),
    ))
}

/// A CD image: its InstallShield setup, its installer program, or the files on it.
fn open_cd(r: Box<dyn ReadSeek>) -> Result<(MediaType, Box<dyn Archive>)> {
    let mut img = iso::Iso::from_reader(r)?;
    if let (Some(h), Some(_)) = (img.find("DATA1.HDR"), img.find("DATA1.CAB")) {
        let hdr = img.read_entry(h)?;
        // DATA1.CAB, DATA2.CAB, … as far as they go.
        let mut volumes = Vec::new();
        while let Some(v) = img.find(&format!("DATA{}.CAB", volumes.len() + 1)) {
            volumes.push(v);
        }
        let readers = img
            .into_entry_readers(&volumes)
            .into_iter()
            .map(|r| Some(Box::new(r) as Box<dyn ReadSeek>))
            .collect();
        let cabinet = iscab::Cabinet::open_volumes(&hdr, readers)?;
        return Ok((MediaType::IsoInstallShield, Box::new(cabinet)));
    }
    // A Windows Installer package with its cabinet beside it.
    let root = |ext: &str| -> Vec<usize> {
        img.entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.path.contains('/') && e.path.to_ascii_lowercase().ends_with(ext))
            .map(|(i, _)| i)
            .collect()
    };
    if let ([m], [c]) = (&root(".msi")[..], &root(".cab")[..]) {
        let (m, c) = (*m, *c);
        let msi = img.read_entry(m)?;
        let cab = img.into_entry_reader(c);
        return Ok((
            MediaType::IsoInstaller,
            Box::new(msi::Package::open(&msi, Box::new(cab))?),
        ));
    }
    // An installer program in the root (a launcher such as Autostart.exe is no installer).
    let mut exes: Vec<(String, usize)> = img
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.path.contains('/') && e.path.to_ascii_lowercase().ends_with(".exe"))
        .map(|(i, e)| (e.path.clone(), i))
        .collect();
    exes.sort();
    for (_, i) in exes {
        let mut sub = img.open_entry(i);
        let installer = nsis::find_first_header(&mut sub).is_some() || inno3::is_old_inno(&mut sub);
        if installer {
            let (_, archive) = open_installer(Box::new(img.into_entry_reader(i)), Vec::new())?;
            return Ok((MediaType::IsoInstaller, archive));
        }
    }
    Ok((MediaType::Iso, Box::new(img)))
}
