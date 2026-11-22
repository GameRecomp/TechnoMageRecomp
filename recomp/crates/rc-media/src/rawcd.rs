// SPDX-License-Identifier: MIT
//! Raw CD images (`.bin` with 2352-byte sectors, as a `.cue` describes them) read as the cooked
//! 2048-byte-sector image of their first data track, so the ISO 9660 reader can open them.
//!
//! A raw data sector starts with the 12-byte sync pattern `00 ff×10 00`, a 3-byte address and a
//! mode byte. Mode 1 sectors carry 2048 bytes of user data at offset 16; Mode 2 Form 1 sectors
//! (what a `MODE2/2352` track holds for data) carry them at offset 24, after the 8-byte subheader.
//! The mode is read from the first sector; the track is taken to start at the beginning of the
//! file (a single-track data CD, as the `.cue` files of the supported games say).

use crate::ReadSeek;
use std::io::{self, Read, Seek, SeekFrom};

pub const RAW_SECTOR: u64 = 2352;
pub const COOKED_SECTOR: u64 = 2048;
const SYNC: [u8; 12] = [
    0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,
];

/// Where the user data of each raw sector of `r` starts, if `r` is a raw CD image: 16 for Mode 1,
/// 24 for Mode 2 (Form 1). `None` for anything else (a cooked ISO, an exe, …).
pub fn detect(r: &mut dyn ReadSeek) -> Option<u64> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    if len < 17 * RAW_SECTOR || len % RAW_SECTOR != 0 {
        return None;
    }
    let mut head = [0u8; 16];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut head).ok()?;
    if head[..12] != SYNC {
        return None;
    }
    let offset = match head[15] {
        1 => 16,
        2 => 24,
        _ => return None,
    };
    // The ISO 9660 primary volume descriptor is in sector 16.
    let mut pvd = [0u8; 6];
    r.seek(SeekFrom::Start(16 * RAW_SECTOR + offset)).ok()?;
    r.read_exact(&mut pvd).ok()?;
    (pvd[1..6] == *b"CD001").then_some(offset)
}

/// The cooked image of a raw CD image: sector `n`'s 2048 user-data bytes at `n * 2048`.
pub struct RawCd<R> {
    inner: R,
    data_offset: u64,
    sectors: u64,
    pos: u64,
}

impl<R: Read + Seek> RawCd<R> {
    /// `data_offset` is what [`detect`] returned.
    pub fn new(mut inner: R, data_offset: u64) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        Ok(RawCd {
            inner,
            data_offset,
            sectors: len / RAW_SECTOR,
            pos: 0,
        })
    }

    fn len(&self) -> u64 {
        self.sectors * COOKED_SECTOR
    }
}

impl<R: Read + Seek> Read for RawCd<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() && self.pos < self.len() {
            let sector = self.pos / COOKED_SECTOR;
            let within = self.pos % COOKED_SECTOR;
            let n = ((COOKED_SECTOR - within) as usize).min(buf.len() - done);
            self.inner.seek(SeekFrom::Start(
                sector * RAW_SECTOR + self.data_offset + within,
            ))?;
            self.inner.read_exact(&mut buf[done..done + n])?;
            done += n;
            self.pos += n as u64;
        }
        Ok(done)
    }
}

impl<R: Read + Seek> Seek for RawCd<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        match target {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the CD image",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn raw_image(mode: u8, sectors: usize) -> Vec<u8> {
        let offset = if mode == 1 { 16 } else { 24 };
        let mut img = Vec::new();
        for s in 0..sectors {
            let mut sec = vec![0u8; RAW_SECTOR as usize];
            sec[..12].copy_from_slice(&SYNC);
            sec[15] = mode;
            for (i, b) in sec[offset..offset + 2048].iter_mut().enumerate() {
                *b = (s * 7 + i) as u8;
            }
            if s == 16 {
                sec[offset..offset + 6].copy_from_slice(b"\x01CD001");
            }
            img.extend(sec);
        }
        img
    }

    #[test]
    fn cooks_mode2_sectors() {
        let img = raw_image(2, 20);
        let mut c = Cursor::new(img);
        assert_eq!(detect(&mut c), Some(24));
        let mut cd = RawCd::new(c, 24).unwrap();
        let mut buf = vec![0u8; 3000];
        cd.seek(SeekFrom::Start(2047)).unwrap();
        cd.read_exact(&mut buf).unwrap();
        assert_eq!(buf[0], (2047u32 % 256) as u8);
        assert_eq!(buf[1], 7); // sector 1, byte 0
        assert_eq!(cd.seek(SeekFrom::End(0)).unwrap(), 20 * 2048);
        assert_eq!(cd.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn mode1_and_non_raw() {
        let mut c = Cursor::new(raw_image(1, 17));
        assert_eq!(detect(&mut c), Some(16));
        let mut iso = Cursor::new(vec![0u8; 40 * 2048]);
        assert_eq!(detect(&mut iso), None);
    }
}
