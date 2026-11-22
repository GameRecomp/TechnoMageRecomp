// SPDX-License-Identifier: MIT
//! Zip reader for archives of installed copies: stored and deflated entries, UTF-8 or
//! CP437 names, CRC-32 checked. Zip64, encryption and multi-disk archives are rejected.
//!
//! A single top-level directory shared by every entry (e.g. `Cultures Saga/`) is stripped,
//! so entry paths are relative to the install directory.

use crate::{codepage, create_output, format_err, le16, le32, Archive, Entry, Error, Result, Sink};
use flate2::read::DeflateDecoder;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

struct ZipFile {
    method: u16,
    crc: u32,
    compressed: u64,
    local_offset: u64,
}

pub struct Zip<R> {
    r: R,
    entries: Vec<Entry>,
    directories: Vec<String>,
    files: Vec<ZipFile>,
}

impl Zip<BufReader<File>> {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(BufReader::new(File::open(path)?))
    }
}

impl<R: Read + Seek> Zip<R> {
    pub fn from_reader(mut r: R) -> Result<Self> {
        let len = r.seek(SeekFrom::End(0))?;
        let tail_len = len.min(22 + 65_535);
        let mut tail = vec![0u8; tail_len as usize];
        r.seek(SeekFrom::Start(len - tail_len))?;
        r.read_exact(&mut tail)?;
        let eocd = (0..tail.len().saturating_sub(21))
            .rev()
            .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
            .ok_or_else(|| Error::Format("no end of central directory".into()))?;
        let e = &tail[eocd..];
        if le16(e, 4)? != 0 || le16(e, 6)? != 0 {
            return Err(Error::Unsupported("multi-disk zip".into()));
        }
        let count = usize::from(le16(e, 10)?);
        let cd_size = le32(e, 12)?;
        let cd_offset = le32(e, 16)?;
        if count == 0xffff || cd_offset == u32::MAX {
            return Err(Error::Unsupported("zip64".into()));
        }
        let mut cd = vec![0u8; cd_size as usize];
        r.seek(SeekFrom::Start(u64::from(cd_offset)))?;
        r.read_exact(&mut cd)?;

        let mut raw = Vec::with_capacity(count);
        let mut dir_names = Vec::new();
        let mut o = 0usize;
        for _ in 0..count {
            if le32(&cd, o)? != 0x0201_4b50 {
                return format_err("bad central directory entry");
            }
            let flags = le16(&cd, o + 8)?;
            let method = le16(&cd, o + 10)?;
            let crc = le32(&cd, o + 16)?;
            let compressed = le32(&cd, o + 20)?;
            let size = le32(&cd, o + 24)?;
            let name_len = usize::from(le16(&cd, o + 28)?);
            let extra_len = usize::from(le16(&cd, o + 30)?);
            let comment_len = usize::from(le16(&cd, o + 32)?);
            let local = le32(&cd, o + 42)?;
            let name_bytes = cd
                .get(o + 46..o + 46 + name_len)
                .ok_or_else(|| Error::Format("truncated name".into()))?;
            o += 46 + name_len + extra_len + comment_len;
            if flags & 1 != 0 {
                return Err(Error::Unsupported("encrypted zip entry".into()));
            }
            if compressed == u32::MAX || size == u32::MAX || local == u32::MAX {
                return Err(Error::Unsupported("zip64 entry".into()));
            }
            let name = if flags & 0x0800 != 0 {
                String::from_utf8_lossy(name_bytes).into_owned()
            } else {
                codepage::cp437(name_bytes)
            };
            if name.ends_with('/') {
                dir_names.push(name.trim_end_matches('/').to_string());
                continue;
            }
            if method != 0 && method != 8 {
                return Err(Error::Unsupported(format!(
                    "{name}: compression method {method}"
                )));
            }
            raw.push((
                name,
                u64::from(size),
                ZipFile {
                    method,
                    crc,
                    compressed: u64::from(compressed),
                    local_offset: u64::from(local),
                },
            ));
        }

        // Strip a common top-level directory.
        let prefix = raw
            .first()
            .and_then(|(n, _, _)| n.split_once('/'))
            .map(|(p, _)| p);
        let strip = prefix.filter(|p| {
            raw.iter()
                .all(|(n, _, _)| n.strip_prefix(p).is_some_and(|rest| rest.starts_with('/')))
                && dir_names
                    .iter()
                    .all(|d| d == p || d.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
        });
        let strip_len = strip.map_or(0, |p| p.len() + 1);
        let directories = dir_names
            .iter()
            .filter(|d| d.len() > strip_len)
            .map(|d| d[strip_len..].to_string())
            .collect();

        let mut entries = Vec::with_capacity(raw.len());
        let mut files = Vec::with_capacity(raw.len());
        for (name, size, f) in raw {
            entries.push(Entry {
                path: name[strip_len..].to_string(),
                size,
            });
            files.push(f);
        }
        Ok(Zip {
            r,
            entries,
            directories,
            files,
        })
    }

    /// Read a whole file (`k` indexes `entries`).
    pub fn read_entry(&mut self, k: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.entries[k].size as usize);
        self.write_file(k, &mut out)?;
        Ok(out)
    }

    fn write_file(&mut self, k: usize, out: &mut dyn Write) -> Result<()> {
        let f = &self.files[k];
        let path = &self.entries[k].path;
        let mut lh = [0u8; 30];
        self.r.seek(SeekFrom::Start(f.local_offset))?;
        self.r.read_exact(&mut lh)?;
        if le32(&lh, 0)? != 0x0403_4b50 {
            return format_err(format!("{path}: bad local header"));
        }
        let skip = u64::from(le16(&lh, 26)?) + u64::from(le16(&lh, 28)?);
        self.r.seek(SeekFrom::Start(f.local_offset + 30 + skip))?;
        let data = (&mut self.r).take(f.compressed);
        let mut sink = CrcWriter {
            inner: out,
            crc: crc32fast::Hasher::new(),
            n: 0,
        };
        match f.method {
            0 => {
                std::io::copy(&mut { data }, &mut sink)?;
            }
            _ => {
                std::io::copy(&mut DeflateDecoder::new(data), &mut sink)?;
            }
        }
        let size = self.entries[k].size;
        if sink.n != size || sink.crc.finalize() != f.crc {
            return Err(Error::Checksum(format!("{path}: size or CRC-32 mismatch")));
        }
        Ok(())
    }
}

struct CrcWriter<'a> {
    inner: &'a mut dyn Write,
    crc: crc32fast::Hasher,
    n: u64,
}

impl Write for CrcWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc.update(&buf[..n]);
        self.n += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<R: Read + Seek> Archive for Zip<R> {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn directories(&self) -> &[String] {
        &self.directories
    }

    fn extract(&mut self, dest: &Path, select: &dyn Fn(&Entry) -> bool) -> Result<usize> {
        crate::create_dirs(dest, &self.directories)?;
        let mut n = 0;
        for k in 0..self.entries.len() {
            if !select(&self.entries[k]) {
                continue;
            }
            let path = self.entries[k].path.clone();
            let mut out = std::io::BufWriter::new(create_output(dest, &path)?);
            self.write_file(k, &mut out)?;
            out.flush()?;
            n += 1;
        }
        Ok(n)
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
        let mut n = 0;
        for k in 0..self.entries.len() {
            if !select(&self.entries[k]) {
                continue;
            }
            let Entry { path, size } = self.entries[k].clone();
            crate::safe_relative(&path)?;
            sink.begin(&path, size)?;
            let mut w = SinkWriter { sink, error: None };
            let r = self.write_file(k, &mut w);
            if let Some(e) = w.error {
                return Err(e);
            }
            r?;
            sink.end()?;
            n += 1;
        }
        Ok(n)
    }
}

/// A sink as a writer (one file's bytes); the sink's own error is kept.
struct SinkWriter<'a> {
    sink: &'a mut dyn Sink,
    error: Option<Error>,
}

impl Write for SinkWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.sink.write(buf) {
            Ok(()) => Ok(buf.len()),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(e);
                Err(std::io::Error::other(msg))
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a zip with the given (name, data, deflate?) entries.
    fn build(files: &[(&[u8], &[u8], bool)], utf8: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data, deflate) in files {
            let payload = if *deflate {
                let mut e =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let crc = crc32fast::hash(data);
            let method: u16 = if *deflate { 8 } else { 0 };
            let flags: u16 = if utf8 { 0x0800 } else { 0 };
            let offset = out.len() as u32;
            let mut lh = vec![0u8; 30];
            lh[0..4].copy_from_slice(&0x0403_4b50u32.to_le_bytes());
            lh[6..8].copy_from_slice(&flags.to_le_bytes());
            lh[8..10].copy_from_slice(&method.to_le_bytes());
            lh[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend(lh);
            out.extend_from_slice(name);
            out.extend(&payload);
            let mut ce = vec![0u8; 46];
            ce[0..4].copy_from_slice(&0x0201_4b50u32.to_le_bytes());
            ce[8..10].copy_from_slice(&flags.to_le_bytes());
            ce[10..12].copy_from_slice(&method.to_le_bytes());
            ce[16..20].copy_from_slice(&crc.to_le_bytes());
            ce[20..24].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            ce[24..28].copy_from_slice(&(data.len() as u32).to_le_bytes());
            ce[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            ce[42..46].copy_from_slice(&offset.to_le_bytes());
            cd.extend(ce);
            cd.extend_from_slice(name);
        }
        let cd_offset = out.len() as u32;
        out.extend(&cd);
        let mut eocd = vec![0u8; 22];
        eocd[0..4].copy_from_slice(&0x0605_4b50u32.to_le_bytes());
        eocd[8..10].copy_from_slice(&(files.len() as u16).to_le_bytes());
        eocd[10..12].copy_from_slice(&(files.len() as u16).to_le_bytes());
        eocd[12..16].copy_from_slice(&(cd.len() as u32).to_le_bytes());
        eocd[16..20].copy_from_slice(&cd_offset.to_le_bytes());
        out.extend(eocd);
        out
    }

    #[test]
    fn strips_prefix_and_reads_both_methods() {
        let z = build(
            &[
                (b"Game/", b"", false),
                (b"Game/Game.exe", b"MZ stored", false),
                (
                    "Game/DataX/Ö.txt".as_bytes(),
                    b"deflated deflated deflated",
                    true,
                ),
            ],
            true,
        );
        let mut zip = Zip::from_reader(Cursor::new(z)).unwrap();
        let paths: Vec<_> = zip.entries().iter().map(|e| e.path.clone()).collect();
        assert_eq!(paths, ["Game.exe", "DataX/Ö.txt"]);
        assert!(zip.directories().is_empty()); // only the stripped top-level directory
        let mut out = Vec::new();
        zip.write_file(1, &mut out).unwrap();
        assert_eq!(out, b"deflated deflated deflated");
    }

    /// Records what a sink is given.
    #[derive(Default)]
    struct Log(Vec<String>);

    impl Sink for Log {
        fn dir(&mut self, p: &str) -> Result<()> {
            self.0.push(format!("dir {p}"));
            Ok(())
        }
        fn begin(&mut self, p: &str, n: u64) -> Result<()> {
            self.0.push(format!("begin {p} {n}"));
            Ok(())
        }
        fn write(&mut self, d: &[u8]) -> Result<()> {
            self.0.push(String::from_utf8_lossy(d).into_owned());
            Ok(())
        }
        fn end(&mut self) -> Result<()> {
            self.0.push("end".into());
            Ok(())
        }
    }

    #[test]
    fn extracts_to_a_sink() {
        let z = build(
            &[
                (b"Game/Maps/", b"", false),
                (b"Game/Game.exe", b"MZ", false),
                (b"Game/a.txt", b"aaaa", true),
            ],
            true,
        );
        let mut zip = Zip::from_reader(Cursor::new(z)).unwrap();
        let mut log = Log::default();
        let n = zip.extract_to(&mut log, &|e| e.path != "Game.exe").unwrap();
        assert_eq!(n, 1);
        assert_eq!(log.0.join("|"), "dir Maps|begin a.txt 4|aaaa|end");
    }

    #[test]
    fn cp437_names_and_crc_check() {
        let mut z = build(&[(b"K\x94nig.c2m", b"abc", false)], false);
        let mut zip = Zip::from_reader(Cursor::new(z.clone())).unwrap();
        assert_eq!(zip.entries()[0].path, "König.c2m");
        let mut out = Vec::new();
        zip.write_file(0, &mut out).unwrap();
        assert_eq!(out, b"abc");

        // Corrupt the stored data: the CRC check must catch it.
        let data_pos = z.windows(3).position(|w| w == b"abc").unwrap();
        z[data_pos] = b'x';
        let mut zip = Zip::from_reader(Cursor::new(z)).unwrap();
        assert!(matches!(
            zip.write_file(0, &mut Vec::new()),
            Err(Error::Checksum(_))
        ));
    }
}
