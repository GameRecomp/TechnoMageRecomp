// SPDX-License-Identifier: MIT
//! ISO 9660 reader. Uses the Joliet supplementary volume (long Unicode names) when present,
//! otherwise the primary volume. Rock Ridge is not interpreted. Multi-extent files are rejected.

use crate::{
    copy_exact, create_output, format_err, le32, Archive, Entry, Error, ReadSeek, Result, SubReader,
};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const SECTOR: u64 = 2048;

pub struct Iso {
    r: Box<dyn ReadSeek>,
    entries: Vec<Entry>,
    /// (byte offset, length) of each entry's single extent.
    extents: Vec<(u64, u64)>,
    joliet: bool,
}

impl Iso {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(Box::new(BufReader::new(File::open(path)?)))
    }

    pub fn from_reader(mut r: Box<dyn ReadSeek>) -> Result<Self> {
        let mut primary = None;
        let mut joliet = None;
        for sector in 16u64..64 {
            let mut vd = [0u8; 2048];
            r.seek(SeekFrom::Start(sector * SECTOR))?;
            r.read_exact(&mut vd)?;
            if &vd[1..6] != b"CD001" {
                return format_err("no ISO 9660 volume descriptor");
            }
            match vd[0] {
                1 if primary.is_none() => primary = Some(vd),
                2 if matches!(&vd[88..91], b"%/@" | b"%/C" | b"%/E") => joliet = Some(vd),
                255 => break,
                _ => {}
            }
        }
        let (vd, is_joliet) = match (joliet, primary) {
            (Some(j), _) => (j, true),
            (None, Some(p)) => (p, false),
            (None, None) => return format_err("no primary volume descriptor"),
        };
        let root = &vd[156..156 + 34];
        let mut iso = Iso {
            r,
            entries: Vec::new(),
            extents: Vec::new(),
            joliet: is_joliet,
        };
        let (lba, len) = (le32(root, 2)?, le32(root, 10)?);
        iso.walk(u64::from(lba) * SECTOR, u64::from(len), "", 0)?;
        Ok(iso)
    }

    fn walk(&mut self, offset: u64, len: u64, prefix: &str, depth: u32) -> Result<()> {
        if depth > 32 {
            return format_err("directory nesting too deep");
        }
        let mut dir = vec![0u8; usize::try_from(len).map_err(|_| Error::Format("dir".into()))?];
        self.r.seek(SeekFrom::Start(offset))?;
        self.r.read_exact(&mut dir)?;
        let mut o = 0usize;
        while o < dir.len() {
            let rec_len = usize::from(dir[o]);
            if rec_len == 0 {
                // Records never span sectors; skip the padding to the next sector.
                o = (o / 2048 + 1) * 2048;
                continue;
            }
            let rec = dir
                .get(o..o + rec_len)
                .ok_or_else(|| Error::Format("truncated directory record".into()))?;
            o += rec_len;
            let name_len = usize::from(*rec.get(32).unwrap_or(&0));
            let raw = rec
                .get(33..33 + name_len)
                .ok_or_else(|| Error::Format("truncated file name".into()))?;
            if raw == [0] || raw == [1] {
                continue; // "." and ".."
            }
            let flags = rec[25];
            if flags & 0x80 != 0 {
                return Err(Error::Unsupported("multi-extent ISO files".into()));
            }
            let name = self.decode_name(raw);
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let (lba, size) = (u64::from(le32(rec, 2)?), u64::from(le32(rec, 10)?));
            if flags & 0x02 != 0 {
                self.walk(lba * SECTOR, size, &path, depth + 1)?;
            } else {
                self.entries.push(Entry { path, size });
                self.extents.push((lba * SECTOR, size));
            }
        }
        Ok(())
    }

    fn decode_name(&self, raw: &[u8]) -> String {
        let mut name = if self.joliet {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            crate::codepage::cp437(raw)
        };
        if let Some(i) = name.rfind(';') {
            name.truncate(i); // version suffix ";1"
        }
        if name.ends_with('.') {
            name.pop(); // "README." for names without extension
        }
        name
    }

    pub fn is_joliet(&self) -> bool {
        self.joliet
    }

    /// Index of the entry whose path matches ignoring ASCII case.
    pub fn find(&self, path: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.path.eq_ignore_ascii_case(path))
    }

    /// Random access to one file of the image.
    pub fn open_entry(&mut self, index: usize) -> SubReader<&mut dyn ReadSeek> {
        let (off, len) = self.extents[index];
        SubReader::new(self.r.as_mut(), off, len)
    }

    /// Read a whole (small) file.
    pub fn read_entry(&mut self, index: usize) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.open_entry(index).read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Consume the image and give its reader plus the extent of one file, so a dependent
    /// reader (the InstallShield cabinet) can own it.
    pub fn into_entry_reader(self, index: usize) -> SubReader<Box<dyn ReadSeek>> {
        let (off, len) = self.extents[index];
        SubReader::new(self.r, off, len)
    }
}

impl Iso {
    /// Consume the image and give one reader per file of `indices`, all sharing the image.
    pub fn into_entry_readers(self, indices: &[usize]) -> Vec<SubReader<crate::SharedReader>> {
        let shared = crate::SharedReader::new(self.r);
        indices
            .iter()
            .map(|&i| {
                let (off, len) = self.extents[i];
                SubReader::new(shared.clone(), off, len)
            })
            .collect()
    }
}

impl Archive for Iso {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn extract(&mut self, dest: &Path, select: &dyn Fn(&Entry) -> bool) -> Result<usize> {
        let mut n = 0;
        for i in 0..self.entries.len() {
            if !select(&self.entries[i]) {
                continue;
            }
            let (path, size) = (self.entries[i].path.clone(), self.entries[i].size);
            let mut out = create_output(dest, &path)?;
            copy_exact(&mut self.open_entry(i), &mut out, size)?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn dir_record(name: &[u8], lba: u32, size: u32, flags: u8) -> Vec<u8> {
        let len = 33 + name.len() + (name.len() + 1) % 2;
        let mut r = vec![0u8; len];
        r[0] = len as u8;
        r[2..6].copy_from_slice(&lba.to_le_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[25] = flags;
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        r
    }

    /// Image: PVD at 16, terminator at 17, root dir at 18, subdir at 19, file data at 20.
    fn image() -> Vec<u8> {
        let mut img = vec![0u8; 21 * 2048];
        let pvd = 16 * 2048;
        img[pvd] = 1;
        img[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        let root = dir_record(&[0], 18, 2048, 2);
        img[pvd + 156..pvd + 156 + 34].copy_from_slice(&root[..34]);
        img[17 * 2048] = 255;
        img[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"CD001");
        let mut root_dir = Vec::new();
        root_dir.extend(dir_record(&[0], 18, 2048, 2));
        root_dir.extend(dir_record(&[1], 18, 2048, 2));
        root_dir.extend(dir_record(b"DATA1.HDR;1", 20, 5, 0));
        root_dir.extend(dir_record(b"EXTRAS", 19, 2048, 2));
        img[18 * 2048..18 * 2048 + root_dir.len()].copy_from_slice(&root_dir);
        let sub = dir_record(b"README.;1", 20, 3, 0);
        img[19 * 2048..19 * 2048 + sub.len()].copy_from_slice(&sub);
        img[20 * 2048..20 * 2048 + 5].copy_from_slice(b"hello");
        img
    }

    #[test]
    fn lists_and_reads() {
        let mut iso = Iso::from_reader(Box::new(Cursor::new(image()))).unwrap();
        assert!(!iso.is_joliet());
        let paths: Vec<_> = iso.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["DATA1.HDR", "EXTRAS/README"]);
        let i = iso.find("data1.hdr").unwrap();
        assert_eq!(iso.read_entry(i).unwrap(), b"hello");
        assert_eq!(iso.read_entry(1).unwrap(), b"hel");
    }

    #[test]
    fn rejects_non_iso() {
        assert!(Iso::from_reader(Box::new(Cursor::new(vec![0u8; 40 * 2048]))).is_err());
    }
}
