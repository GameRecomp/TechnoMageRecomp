// SPDX-License-Identifier: MIT
//! InstallShield 5 and 6 cabinet readers (`DATA1.HDR` + `DATA1.CAB`, `DATA2.CAB`, …), as used on
//! the German Cultures Gold CD (5) and the Russian Sacked CD (6). Files may span volumes. Other
//! versions are reported as unsupported.
//!
//! Compressed file data is a sequence of chunks, each a little-endian u16 length followed
//! by that many bytes of raw DEFLATE. Obfuscated files are de-obfuscated first. Version 6 keeps
//! 64-bit sizes and an MD5 per file (checked here) and may link a file to another one's data.

use crate::md5::Md5;
use crate::{
    codepage, cstr, format_err, le16, le32, Archive, Entry, Error, ReadSeek, Result, Sink,
};
use flate2::{Decompress, FlushDecompress, Status};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const SIGNATURE: u32 = 0x2863_5349; // "ISc("
const FLAG_OBFUSCATED: u16 = 0x0002;
const FLAG_COMPRESSED: u16 = 0x0004;
const FLAG_INVALID: u16 = 0x0008;
const LINK_PREV: u8 = 0x01;
const V6_DESCRIPTOR: usize = 0x57;

#[derive(Debug, Clone)]
struct CabFile {
    index: u32,
    flags: u16,
    expanded: u64,
    compressed: u64,
    data_offset: u64,
    /// 1-based volume (`DATA<n>.CAB`) the file starts in.
    volume: u16,
    md5: Option<[u8; 16]>,
}

/// What a volume's header says about the files in it.
#[derive(Debug, Clone, Copy, Default)]
struct VolumeHeader {
    first_index: u32,
    last_index: u32,
    first_offset: u64,
    first_compressed: u64,
    last_offset: u64,
    last_compressed: u64,
}

pub struct Cabinet {
    /// `DATA1.CAB`, `DATA2.CAB`, … (`None`: not available).
    volumes: Vec<Option<(Box<dyn ReadSeek>, VolumeHeader)>>,
    entries: Vec<Entry>,
    files: Vec<CabFile>,
}

/// Major version from the common header's version field.
fn major_version(v: u32) -> u32 {
    match v >> 24 {
        1 => (v >> 12) & 0xf,
        2 | 4 => (v & 0xffff) / 100,
        _ => 0,
    }
}

fn le64(b: &[u8], o: usize) -> Result<u64> {
    Ok(u64::from(le32(b, o)?) | u64::from(le32(b, o + 4)?) << 32)
}

impl Cabinet {
    /// `hdr` is the whole `DATA1.HDR`; `volume` reads `DATA1.CAB` (the only volume).
    pub fn open(hdr: &[u8], volume: Box<dyn ReadSeek>) -> Result<Self> {
        Self::open_volumes(hdr, vec![Some(volume)])
    }

    /// `hdr` is the whole `DATA1.HDR`; `volumes` read `DATA1.CAB`, `DATA2.CAB`, … in order.
    pub fn open_volumes(hdr: &[u8], volumes: Vec<Option<Box<dyn ReadSeek>>>) -> Result<Self> {
        if le32(hdr, 0)? != SIGNATURE {
            return format_err("not an InstallShield header (signature)");
        }
        let major = major_version(le32(hdr, 4)?);
        if major != 5 && major != 6 {
            return Err(Error::Unsupported(format!(
                "InstallShield cabinet version {major} (only 5 and 6)"
            )));
        }
        let b = le32(hdr, 0x0c)? as usize; // cab descriptor
        let t = b + le32(hdr, b + 0x0c)? as usize; // file table
        let dir_count = le32(hdr, b + 0x1c)? as usize;
        let file_count = le32(hdr, b + 0x28)? as usize;
        let t2 = t + le32(hdr, b + 0x2c)? as usize; // v6 file descriptors
        if dir_count > 100_000 || file_count > 1_000_000 {
            return format_err("implausible directory/file count");
        }
        let table = |i: usize| le32(hdr, t + 4 * i).map(|v| v as usize);

        let mut dirs = Vec::with_capacity(dir_count);
        for i in 0..dir_count {
            let name = codepage::cp1252(cstr(hdr, t + table(i)?)?);
            dirs.push(name.replace('\\', "/"));
        }

        let mut entries = Vec::new();
        let mut files = Vec::new();
        let mut all = vec![None; file_count];
        for i in 0..file_count {
            let (name_off, dir_index, mut file, link) = if major == 5 {
                let d = t + table(dir_count + i)?;
                let f = CabFile {
                    index: i as u32,
                    flags: le16(hdr, d + 0x08)?,
                    expanded: u64::from(le32(hdr, d + 0x0a)?),
                    compressed: u64::from(le32(hdr, d + 0x0e)?),
                    data_offset: u64::from(le32(hdr, d + 0x26)?),
                    volume: 1,
                    md5: None,
                };
                (
                    le32(hdr, d)? as usize,
                    usize::from(le16(hdr, d + 0x04)?),
                    f,
                    None,
                )
            } else {
                let d = t2 + i * V6_DESCRIPTOR;
                let md5: [u8; 16] = hdr
                    .get(d + 0x1a..d + 0x2a)
                    .ok_or_else(|| Error::Format("file descriptor truncated".into()))?
                    .try_into()
                    .expect("16");
                let f = CabFile {
                    index: i as u32,
                    flags: le16(hdr, d)?,
                    expanded: le64(hdr, d + 0x02)?,
                    compressed: le64(hdr, d + 0x0a)?,
                    data_offset: le64(hdr, d + 0x12)?,
                    volume: le16(hdr, d + 0x55)?,
                    md5: Some(md5),
                };
                let link_flags = hdr.get(d + 0x54).copied().unwrap_or(0);
                let link = (link_flags & LINK_PREV != 0)
                    .then(|| le32(hdr, d + 0x4c))
                    .transpose()?;
                (
                    le32(hdr, d + 0x3a)? as usize,
                    usize::from(le16(hdr, d + 0x3e)?),
                    f,
                    link,
                )
            };
            all[i] = Some(file.clone());
            if file.flags & FLAG_INVALID != 0 || name_off == 0 {
                continue;
            }
            if let Some(prev) = link {
                // The data is another file's: read that one's.
                let Some(Some(p)) = all.get(prev as usize).cloned() else {
                    return format_err(format!(
                        "file {i} links to file {prev}, which is not before it"
                    ));
                };
                file = CabFile {
                    index: file.index,
                    ..p
                };
            } else if file.data_offset == 0 {
                continue;
            }
            let name = codepage::cp1252(cstr(hdr, t + name_off)?);
            let dir = dirs
                .get(dir_index)
                .ok_or_else(|| Error::Format(format!("file {name}: bad directory index")))?;
            let path = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            entries.push(Entry {
                path,
                size: file.expanded,
            });
            files.push(file);
        }

        let mut vols = Vec::new();
        for v in volumes {
            let Some(mut r) = v else {
                vols.push(None);
                continue;
            };
            let mut vh = [0u8; 0x54];
            r.seek(SeekFrom::Start(0))?;
            r.read_exact(&mut vh)?;
            if le32(&vh, 0)? != SIGNATURE {
                return format_err("cabinet volume has no InstallShield signature");
            }
            let h = if major == 5 {
                VolumeHeader {
                    first_index: le32(&vh, 0x1c)?,
                    last_index: le32(&vh, 0x20)?,
                    first_offset: u64::from(le32(&vh, 0x24)?),
                    first_compressed: u64::from(le32(&vh, 0x2c)?),
                    last_offset: u64::from(le32(&vh, 0x30)?),
                    last_compressed: u64::from(le32(&vh, 0x38)?),
                }
            } else {
                VolumeHeader {
                    first_index: le32(&vh, 0x1c)?,
                    last_index: le32(&vh, 0x20)?,
                    first_offset: le64(&vh, 0x24)?,
                    first_compressed: le64(&vh, 0x34)?,
                    last_offset: le64(&vh, 0x3c)?,
                    last_compressed: le64(&vh, 0x4c)?,
                }
            };
            vols.push(Some((r, h)));
        }
        Ok(Cabinet {
            volumes: vols,
            entries,
            files,
        })
    }

    /// The stored bytes of file `f`, gathered from the volumes it spans.
    fn stored_bytes(&mut self, f: &CabFile, path: &str) -> Result<Vec<u8>> {
        let total = if f.flags & FLAG_COMPRESSED != 0 {
            f.compressed
        } else {
            f.expanded
        };
        let mut raw = Vec::with_capacity(total as usize);
        let mut vol = usize::from(f.volume.max(1));
        let mut first = true;
        while (raw.len() as u64) < total {
            let Some(Some((r, h))) = self.volumes.get_mut(vol - 1) else {
                return Err(Error::Unsupported(format!(
                    "{path} is stored in cabinet volume {vol}, which is missing"
                )));
            };
            let h = *h;
            // As the last file of a volume a file may go on in the next one; as the first file
            // of a volume it may come from the previous one; else it is where its descriptor says.
            // (A volume whose files do not span leaves these fields 0.)
            let (offset, size) =
                if f.index == h.last_index && h.last_offset != 0x7fff_ffff && h.last_offset != 0 {
                    (h.last_offset, h.last_compressed)
                } else if f.index == h.first_index && h.first_offset != 0 {
                    (h.first_offset, h.first_compressed)
                } else if first {
                    (f.data_offset, total)
                } else {
                    return format_err(format!("{path}: not continued in cabinet volume {vol}"));
                };
            if f.index < h.first_index || f.index > h.last_index {
                return format_err(format!("{path}: not in cabinet volume {vol}"));
            }
            let want = size.min(total - raw.len() as u64);
            if want == 0 {
                return format_err(format!("{path}: no bytes in cabinet volume {vol}"));
            }
            r.seek(SeekFrom::Start(offset))?;
            let start = raw.len();
            raw.resize(start + want as usize, 0);
            r.read_exact(&mut raw[start..])?;
            vol += 1;
            first = false;
        }
        Ok(raw)
    }

    fn write_file(&mut self, k: usize, out: &mut dyn Write) -> Result<()> {
        let f = self.files[k].clone();
        let path = self.entries[k].path.clone();
        let mut raw = self.stored_bytes(&f, &path)?;
        if f.flags & FLAG_OBFUSCATED != 0 {
            deobfuscate(&mut raw);
        }
        let mut sum = Md5Writer(Md5::new(), out);
        let written = if f.flags & FLAG_COMPRESSED == 0 {
            sum.write_all(&raw)?;
            raw.len() as u64
        } else {
            inflate_chunks(&raw, &mut sum)?
        };
        if written != f.expanded {
            return Err(Error::Checksum(format!(
                "{path}: {written} bytes, expected {}",
                f.expanded
            )));
        }
        if let Some(want) = f.md5 {
            if sum.0.finish() != want {
                return Err(Error::Checksum(format!("{path}: MD5")));
            }
        }
        Ok(())
    }
}

/// Feeds a [`Sink`] as a [`Write`].
struct SinkWriter<'s>(&'s mut dyn Sink);

impl Write for SinkWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .write(buf)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Hashes what passes through to the writer.
struct Md5Writer<'w>(Md5, &'w mut dyn Write);

impl Write for Md5Writer<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        self.1.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.1.flush()
    }
}

/// Inverse of InstallShield's byte obfuscation (counter starts at 0 per file).
fn deobfuscate(buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (*b ^ 0xd5).rotate_right(2).wrapping_sub((i % 0x47) as u8);
    }
}

/// Decode `[u16 len][raw deflate]...` into `out`. Returns the number of bytes written.
fn inflate_chunks(mut data: &[u8], out: &mut dyn Write) -> Result<u64> {
    let mut total = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    while !data.is_empty() {
        let len = usize::from(le16(data, 0)?);
        let chunk = data
            .get(2..2 + len)
            .ok_or_else(|| Error::Format("truncated compressed chunk".into()))?;
        data = &data[2 + len..];
        let mut z = Decompress::new(false);
        let mut input = chunk;
        loop {
            let before_in = z.total_in();
            let before_out = z.total_out();
            let status = z
                .decompress(input, &mut buf, FlushDecompress::Finish)
                .map_err(|e| Error::Format(format!("deflate: {e}")))?;
            let used = (z.total_in() - before_in) as usize;
            let produced = (z.total_out() - before_out) as usize;
            input = &input[used..];
            out.write_all(&buf[..produced])?;
            total += produced as u64;
            match status {
                Status::StreamEnd => break,
                _ if used == 0 && produced == 0 => {
                    if input.is_empty() {
                        break; // chunk ended without a final block marker
                    }
                    return format_err("deflate made no progress");
                }
                _ => {}
            }
        }
    }
    Ok(total)
}

impl Archive for Cabinet {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn extract_to(
        &mut self,
        sink: &mut dyn Sink,
        select: &dyn Fn(&Entry) -> bool,
    ) -> Result<usize> {
        let mut dirs = std::collections::BTreeSet::new();
        for e in &self.entries {
            if let Some((d, _)) = e.path.rsplit_once('/') {
                dirs.insert(d.to_string());
            }
        }
        for d in dirs {
            sink.dir(&d)?;
        }
        let mut n = 0;
        for k in 0..self.entries.len() {
            if !select(&self.entries[k]) {
                continue;
            }
            let (path, size) = (self.entries[k].path.clone(), self.entries[k].size);
            sink.begin(&path, size)?;
            self.write_file(k, &mut SinkWriter(sink))?;
            sink.end()?;
            n += 1;
        }
        Ok(n)
    }

    fn extract(&mut self, dest: &Path, select: &dyn Fn(&Entry) -> bool) -> Result<usize> {
        let mut n = 0;
        for k in 0..self.entries.len() {
            if !select(&self.entries[k]) {
                continue;
            }
            let path = self.entries[k].path.clone();
            let mut out = std::io::BufWriter::new(crate::create_output(dest, &path)?);
            self.write_file(k, &mut out)?;
            out.flush()?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    #[test]
    fn version_decoding() {
        assert_eq!(major_version(0x0100_5201), 5); // Cultures Gold
        assert_eq!(major_version(0x0200_0258), 6);
        assert_eq!(major_version(0x0700_0000), 0);
    }

    #[test]
    fn obfuscation_round_trip() {
        let plain: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        // Forward transform: (b + i%0x47) rotl 2, xor 0xd5.
        let mut enc: Vec<u8> = plain
            .iter()
            .enumerate()
            .map(|(i, &b)| b.wrapping_add((i % 0x47) as u8).rotate_left(2) ^ 0xd5)
            .collect();
        deobfuscate(&mut enc);
        assert_eq!(enc, plain);
    }

    #[test]
    fn chunked_raw_deflate() {
        let parts: [&[u8]; 2] = [b"first chunk, first chunk, first chunk", b"second"];
        let mut stream = Vec::new();
        for p in parts {
            let mut c = Compress::new(Compression::default(), false);
            let mut buf = vec![0u8; 256];
            c.compress(p, &mut buf, FlushCompress::Finish).unwrap();
            let n = c.total_out() as usize;
            stream.extend_from_slice(&(n as u16).to_le_bytes());
            stream.extend_from_slice(&buf[..n]);
        }
        let mut out = Vec::new();
        assert_eq!(inflate_chunks(&stream, &mut out).unwrap(), 43);
        assert_eq!(out, b"first chunk, first chunk, first chunksecond");
    }
}
