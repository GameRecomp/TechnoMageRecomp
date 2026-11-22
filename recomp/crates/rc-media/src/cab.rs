// SPDX-License-Identifier: MIT
//! Microsoft cabinets (`MSCF`) with stored or MSZIP folders, as Windows Installer packages ship
//! them (`data.cab` beside an `.msi`). LZX and Quantum folders and cabinet sets spanning several
//! files are refused with [`Error::Unsupported`].
//!
//! Layout (Microsoft's published cabinet format): a header (sizes, folder and file counts, flags,
//! optional reserved areas and neighbour cabinet names), the folders (offset of their first data
//! block, block count, compression), the files (size, offset in their folder's uncompressed
//! stream, folder index, DOS time, attributes, name) and the data blocks (checksum, compressed
//! and uncompressed size, at most 32 KiB uncompressed each). An MSZIP block is `CK` and a deflate
//! stream that may refer back into the previous block's output.

use crate::{format_err, Error, ReadSeek, Result};
use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;
use std::io::{Read, Seek, SeekFrom, Write};

const FLAG_PREV: u16 = 1;
const FLAG_NEXT: u16 = 2;
const FLAG_RESERVE: u16 = 4;
const COMPRESS_NONE: u16 = 0;
const COMPRESS_MSZIP: u16 = 1;
const BLOCK_MAX: usize = 32 * 1024;

/// What receives a file's bytes from [`Cabinet::extract`]: `(file, piece, last piece)`.
pub type PieceFn<'a> = dyn FnMut(usize, &[u8], bool) -> Result<()> + 'a;
const WINDOW: usize = 32 * 1024;

#[derive(Debug, Clone)]
struct Folder {
    first_block: u32,
    blocks: u16,
    compression: u16,
}

/// One file of the cabinet.
#[derive(Debug, Clone)]
pub struct CabFile {
    pub name: String,
    pub size: u32,
    offset: u32,
    folder: u16,
}

pub struct Cabinet {
    r: Box<dyn ReadSeek>,
    folders: Vec<Folder>,
    pub files: Vec<CabFile>,
    data_reserve: usize,
}

impl Cabinet {
    pub fn open(mut r: Box<dyn ReadSeek>) -> Result<Self> {
        let mut h = [0u8; 36];
        r.seek(SeekFrom::Start(0))?;
        r.read_exact(&mut h)?;
        if &h[..4] != b"MSCF" {
            return format_err("not a cabinet (no MSCF)");
        }
        let le16 = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
        let le32 = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().expect("4"));
        let files_at = u64::from(le32(16));
        let (folders, files, flags) = (le16(26), le16(28), le16(30));
        if flags & (FLAG_PREV | FLAG_NEXT) != 0 {
            return Err(Error::Unsupported(
                "cabinet sets spanning several files".into(),
            ));
        }
        let (mut folder_reserve, mut data_reserve) = (0usize, 0usize);
        if flags & FLAG_RESERVE != 0 {
            let mut res = [0u8; 4];
            r.read_exact(&mut res)?;
            let header_reserve = u16::from_le_bytes([res[0], res[1]]);
            folder_reserve = usize::from(res[2]);
            data_reserve = usize::from(res[3]);
            r.seek(SeekFrom::Current(i64::from(header_reserve)))?;
        }
        let mut fs = Vec::with_capacity(usize::from(folders));
        for _ in 0..folders {
            let mut f = [0u8; 8];
            r.read_exact(&mut f)?;
            r.seek(SeekFrom::Current(folder_reserve as i64))?;
            let compression = u16::from_le_bytes([f[6], f[7]]);
            if compression & 0x0f != COMPRESS_NONE && compression & 0x0f != COMPRESS_MSZIP {
                return Err(Error::Unsupported(format!(
                    "cabinet compression {compression:#x} (only stored and MSZIP)"
                )));
            }
            fs.push(Folder {
                first_block: u32::from_le_bytes(f[0..4].try_into().expect("4")),
                blocks: u16::from_le_bytes([f[4], f[5]]),
                compression: compression & 0x0f,
            });
        }
        r.seek(SeekFrom::Start(files_at))?;
        let mut list = Vec::with_capacity(usize::from(files));
        for _ in 0..files {
            let mut f = [0u8; 16];
            r.read_exact(&mut f)?;
            let folder = u16::from_le_bytes([f[8], f[9]]);
            if usize::from(folder) >= fs.len() {
                return Err(Error::Unsupported(
                    "cabinet file continued from or into another cabinet".into(),
                ));
            }
            let mut name = Vec::new();
            loop {
                let mut c = [0u8; 1];
                r.read_exact(&mut c)?;
                if c[0] == 0 {
                    break;
                }
                name.push(c[0]);
                if name.len() > 1024 {
                    return format_err("cabinet file name too long");
                }
            }
            list.push(CabFile {
                name: crate::codepage::cp1252(&name),
                size: u32::from_le_bytes(f[0..4].try_into().expect("4")),
                offset: u32::from_le_bytes(f[4..8].try_into().expect("4")),
                folder,
            });
        }
        Ok(Cabinet {
            r,
            folders: fs,
            files: list,
            data_reserve,
        })
    }

    /// Unpack the files `wanted` (indices into `files`), in cabinet order. `out` is called
    /// with each file index and gives the writer its bytes go to (`None`: skip the file).
    pub fn extract(&mut self, wanted: &[usize], out: &mut PieceFn<'_>) -> Result<()> {
        let mut by_folder: Vec<Vec<usize>> = vec![Vec::new(); self.folders.len()];
        for &i in wanted {
            by_folder[usize::from(self.files[i].folder)].push(i);
        }
        for (f, mut list) in by_folder.into_iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            list.sort_by_key(|&i| self.files[i].offset);
            self.extract_folder(f, &list, out)?;
        }
        Ok(())
    }

    /// Stream folder `f` and hand the bytes of the files in `list` (sorted by offset) to `out`:
    /// `out(file, bytes, last)` for each piece, `last` on the final one of a file.
    fn extract_folder(&mut self, f: usize, list: &[usize], out: &mut PieceFn<'_>) -> Result<()> {
        let folder = self.folders[f].clone();
        self.r
            .seek(SeekFrom::Start(u64::from(folder.first_block)))?;
        // [window | block]: the last 32 KiB of output, then the block being inflated.
        let mut buf = vec![0u8; WINDOW + BLOCK_MAX];
        let mut window = 0usize;
        let mut pos = 0u64; // uncompressed position of the folder stream
        let mut next = 0usize; // index into list
        let mut raw = Vec::new();
        for _ in 0..folder.blocks {
            if next == list.len() {
                break;
            }
            let mut h = [0u8; 8];
            self.r.read_exact(&mut h)?;
            self.r.seek(SeekFrom::Current(self.data_reserve as i64))?;
            let packed = usize::from(u16::from_le_bytes([h[4], h[5]]));
            let size = usize::from(u16::from_le_bytes([h[6], h[7]]));
            if size > BLOCK_MAX {
                return format_err("cabinet data block larger than 32 KiB");
            }
            raw.resize(packed, 0);
            self.r.read_exact(&mut raw)?;
            let block: &[u8] = if folder.compression == COMPRESS_MSZIP {
                if raw.len() < 2 || &raw[..2] != b"CK" {
                    return format_err("MSZIP block without its CK signature");
                }
                let mut d = DecompressorOxide::new();
                let (status, used, made) = decompress(
                    &mut d,
                    &raw[2..],
                    &mut buf[..window + size],
                    window,
                    inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
                );
                if status != TINFLStatus::Done || made != size || used + 2 != raw.len() {
                    return Err(Error::Checksum(format!(
                        "MSZIP block of folder {f}: {status:?}, {made} of {size} bytes"
                    )));
                }
                &buf[window..window + size]
            } else {
                if packed != size {
                    return format_err("stored cabinet block with two sizes");
                }
                buf[window..window + size].copy_from_slice(&raw);
                &buf[window..window + size]
            };
            // Hand out the parts of the files this block covers.
            let (start, end) = (pos, pos + size as u64);
            while next < list.len() {
                let file = &self.files[list[next]];
                let (fs, fe) = (
                    u64::from(file.offset),
                    u64::from(file.offset) + u64::from(file.size),
                );
                if fs >= end && file.size > 0 {
                    break;
                }
                let a = fs.max(start);
                let b = fe.min(end);
                let piece = if a < b {
                    &block[(a - start) as usize..(b - start) as usize]
                } else {
                    &[][..]
                };
                let last = fe <= end;
                out(list[next], piece, last)?;
                if last {
                    next += 1;
                } else {
                    break;
                }
            }
            pos = end;
            // Keep the last 32 KiB as the next block's history.
            let total = window + size;
            let keep = total.min(WINDOW);
            buf.copy_within(total - keep..total, 0);
            window = keep;
        }
        if next != list.len() {
            return format_err(format!("cabinet folder {f} ends before its files"));
        }
        Ok(())
    }
}

/// Collects one file's pieces for [`Cabinet::extract`] into a writer.
pub fn write_piece(w: &mut dyn Write, piece: &[u8]) -> Result<()> {
    w.write_all(piece)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A cabinet with one MSZIP folder of two blocks (the second refers back into the first)
    /// holding two files.
    fn cabinet() -> (Vec<u8>, Vec<u8>) {
        let text: Vec<u8> = b"Limbic ".iter().cycle().take(40_000).copied().collect();
        let blocks = [&text[..32768], &text[32768..]];
        let mut data = Vec::new();
        let mut history: Vec<u8> = Vec::new();
        for b in blocks {
            // A stored deflate block is valid MSZIP (it needs no history).
            let mut z = vec![b'C', b'K'];
            z.push(1); // final, stored
            z.extend((b.len() as u16).to_le_bytes());
            z.extend((!(b.len() as u16)).to_le_bytes());
            z.extend_from_slice(b);
            data.extend(0u32.to_le_bytes());
            data.extend((z.len() as u16).to_le_bytes());
            data.extend((b.len() as u16).to_le_bytes());
            data.extend(z);
            history.extend_from_slice(b);
        }
        let names = [b"a.txt\0".to_vec(), b"b.bin\0".to_vec()];
        let header_len = 36 + 8;
        let files_len: usize = names.iter().map(|n| 16 + n.len()).sum();
        let data_at = header_len + files_len;
        let mut c = Vec::new();
        c.extend(b"MSCF");
        c.extend(0u32.to_le_bytes());
        c.extend(((data_at + data.len()) as u32).to_le_bytes());
        c.extend(0u32.to_le_bytes());
        c.extend((header_len as u32).to_le_bytes());
        c.extend(0u32.to_le_bytes());
        c.extend([3, 1]);
        c.extend(1u16.to_le_bytes());
        c.extend(2u16.to_le_bytes());
        c.extend(0u16.to_le_bytes());
        c.extend(0u16.to_le_bytes());
        c.extend(0u16.to_le_bytes());
        c.extend((data_at as u32).to_le_bytes());
        c.extend(2u16.to_le_bytes());
        c.extend(COMPRESS_MSZIP.to_le_bytes());
        for (i, n) in names.iter().enumerate() {
            let (size, off) = if i == 0 {
                (30_000u32, 0u32)
            } else {
                (10_000, 30_000)
            };
            c.extend(size.to_le_bytes());
            c.extend(off.to_le_bytes());
            c.extend(0u16.to_le_bytes());
            c.extend([0u8; 6]);
            c.extend(n);
        }
        c.extend(data);
        (c, history)
    }

    #[test]
    fn unpacks_files_across_blocks() {
        let (c, text) = cabinet();
        let mut cab = Cabinet::open(Box::new(Cursor::new(c))).unwrap();
        assert_eq!(cab.files.len(), 2);
        assert_eq!(cab.files[1].name, "b.bin");
        let mut got = [Vec::new(), Vec::new()];
        let mut ends = 0;
        cab.extract(&[1, 0], &mut |i, piece, last| {
            got[i].extend_from_slice(piece);
            ends += usize::from(last);
            Ok(())
        })
        .unwrap();
        assert_eq!(ends, 2);
        assert_eq!(got[0], text[..30_000]);
        assert_eq!(got[1], text[30_000..]);
    }
}
