// SPDX-License-Identifier: MIT
//! Compound files (OLE structured storage, "CFB"), the container of Windows Installer databases.
//! Only what an `.msi` needs: the streams of the root storage, read whole into memory.
//!
//! Layout (Microsoft's published format): a 512-byte header (signature, sector size, the first
//! sectors of the directory, the mini FAT and the DIFAT, the first 109 FAT sector numbers), then
//! sectors. The FAT chains sectors; streams below the mini stream cutoff (4096 bytes) live in the
//! root entry's mini stream, in 64-byte mini sectors chained by the mini FAT. Directory entries
//! are 128 bytes: a UTF-16 name, the type (1 storage, 2 stream, 5 root), tree links, the first
//! sector and the size.

use crate::{format_err, Error, Result};
use std::collections::BTreeMap;

const SIGNATURE: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const END_OF_CHAIN: u32 = 0xffff_fffe;
const FREE: u32 = 0xffff_ffff;
const MAX_REGULAR: u32 = 0xffff_fffa;

pub struct Compound {
    /// Every stream of the file by its name as UTF-16 code units (MSI names are not text).
    streams: BTreeMap<Vec<u16>, Vec<u8>>,
}

impl Compound {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 512 || data[..8] != SIGNATURE {
            return format_err("not a compound file");
        }
        let le16 = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
        let le32 = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().expect("4"));
        let shift = le16(0x1e);
        let mini_shift = le16(0x20);
        if !(shift == 9 || shift == 12) || mini_shift != 6 {
            return format_err(format!("compound file sector shifts {shift}/{mini_shift}"));
        }
        let ss = 1usize << shift;
        let sector = |n: u32| -> Result<&[u8]> {
            let start = (n as usize + 1) * ss;
            data.get(start..start + ss)
                .ok_or_else(|| Error::Format(format!("compound file sector {n} out of range")))
        };
        // The FAT sector numbers: 109 in the header, the rest in the DIFAT chain.
        let mut fat_sectors: Vec<u32> = (0..109).map(|i| le32(0x4c + i * 4)).collect();
        let (mut difat, difat_count) = (le32(0x44), le32(0x48));
        for _ in 0..difat_count {
            if difat > MAX_REGULAR {
                break;
            }
            let s = sector(difat)?;
            for i in 0..ss / 4 - 1 {
                fat_sectors.push(u32::from_le_bytes(
                    s[i * 4..i * 4 + 4].try_into().expect("4"),
                ));
            }
            difat = u32::from_le_bytes(s[ss - 4..].try_into().expect("4"));
        }
        let mut fat = Vec::new();
        for &f in fat_sectors.iter().take(le32(0x2c) as usize) {
            if f > MAX_REGULAR {
                continue;
            }
            for c in sector(f)?.as_chunks::<4>().0 {
                fat.push(u32::from_le_bytes(*c));
            }
        }
        let chain = |start: u32, table: &[u32]| -> Result<Vec<u32>> {
            let mut out = Vec::new();
            let mut n = start;
            while n != END_OF_CHAIN && n != FREE {
                if out.len() > table.len() {
                    return format_err("compound file chain loops");
                }
                out.push(n);
                n = *table.get(n as usize).ok_or_else(|| {
                    Error::Format(format!("compound file chain leaves the FAT at {n}"))
                })?;
            }
            Ok(out)
        };
        let read_chain = |start: u32| -> Result<Vec<u8>> {
            let mut out = Vec::new();
            for n in chain(start, &fat)? {
                out.extend_from_slice(sector(n)?);
            }
            Ok(out)
        };
        let dir = read_chain(le32(0x30))?;
        let mini_fat_bytes = if le32(0x3c) <= MAX_REGULAR {
            read_chain(le32(0x3c))?
        } else {
            Vec::new()
        };
        let mini_fat: Vec<u32> = mini_fat_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        let cutoff = u64::from(le32(0x38));
        let mut mini_stream = Vec::new();
        let mut streams = BTreeMap::new();
        for (i, e) in dir.as_chunks::<128>().0.iter().enumerate() {
            let kind = e[0x42];
            let name_len = usize::from(u16::from_le_bytes([e[0x40], e[0x41]])).min(64);
            let name: Vec<u16> = e[..name_len.saturating_sub(2)]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            let start = u32::from_le_bytes(e[0x74..0x78].try_into().expect("4"));
            let size = u64::from(u32::from_le_bytes(e[0x78..0x7c].try_into().expect("4")));
            match kind {
                5 if i == 0 => {
                    mini_stream = read_chain(start)?;
                }
                2 => {
                    let mut bytes = if size < cutoff {
                        let mut out = Vec::new();
                        for n in chain(start, &mini_fat)? {
                            let at = n as usize * 64;
                            out.extend_from_slice(mini_stream.get(at..at + 64).ok_or_else(
                                || {
                                    Error::Format(format!(
                                        "compound file mini sector {n} out of range"
                                    ))
                                },
                            )?);
                        }
                        out
                    } else {
                        read_chain(start)?
                    };
                    if (bytes.len() as u64) < size {
                        return format_err("compound file stream shorter than its size");
                    }
                    bytes.truncate(size as usize);
                    streams.insert(name, bytes);
                }
                _ => {}
            }
        }
        Ok(Compound { streams })
    }

    pub fn stream(&self, name: &[u16]) -> Option<&[u8]> {
        self.streams.get(name).map(Vec::as_slice)
    }

    pub fn names(&self) -> impl Iterator<Item = &[u16]> {
        self.streams.keys().map(Vec::as_slice)
    }
}
