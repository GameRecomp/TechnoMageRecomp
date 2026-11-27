// SPDX-License-Identifier: MIT
//! A CD image packed for archiving: a file split into parts (`.001`, `.002`, …), a tar archive,
//! a 7z archive, or one inside the other, as archive.org holds some releases. The image is taken
//! out into memory (it must be read randomly, and a 7z stream can only be decoded from its
//! start); the CD readers then read it as any other image.
//!
//! The 7z reader is written from the format's documentation (`7zFormat.txt` of the LZMA SDK) and
//! covers what archivers write for such an archive: one coder per folder (LZMA2, LZMA or
//! copy), the header itself LZMA-compressed or not. Several folders, filters (BCJ) and
//! encryption are refused.

use crate::{format_err, lzma, ReadSeek, Result, SubReader};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

/// Parts of one file, joined.
pub struct Joined {
    parts: Vec<(Box<dyn ReadSeek>, u64)>,
    len: u64,
    pos: u64,
}

impl Joined {
    pub fn new(parts: Vec<Box<dyn ReadSeek>>) -> io::Result<Joined> {
        let mut out = Vec::new();
        let mut len = 0;
        for mut p in parts {
            let n = p.seek(SeekFrom::End(0))?;
            len += n;
            out.push((p, n));
        }
        Ok(Joined {
            parts: out,
            len,
            pos: 0,
        })
    }
}

impl Read for Joined {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut start = 0;
        for (p, n) in &mut self.parts {
            if self.pos < start + *n {
                let at = self.pos - start;
                let want = buf
                    .len()
                    .min(usize::try_from(*n - at).unwrap_or(usize::MAX));
                p.seek(SeekFrom::Start(at))?;
                let got = p.read(&mut buf[..want])?;
                self.pos += got as u64;
                return Ok(got);
            }
            start += *n;
        }
        Ok(0)
    }
}

impl Seek for Joined {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        let p =
            new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        self.pos = p;
        Ok(p)
    }
}

/// Bytes in memory, shared: each [`Image::reader`] reads them from the start.
#[derive(Clone)]
pub struct Image(Arc<Vec<u8>>);

impl AsRef<[u8]> for Image {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Image {
    pub fn reader(&self) -> io::Cursor<Image> {
        io::Cursor::new(self.clone())
    }

    pub fn len(&self) -> u64 {
        self.0.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Whether the medium is packed (a tar or 7z archive), by its first bytes.
pub fn is_packed(r: &mut dyn ReadSeek) -> bool {
    let mut head = [0u8; 512];
    let ok = r.seek(SeekFrom::Start(0)).is_ok() && r.read_exact(&mut head).is_ok();
    let _ = r.seek(SeekFrom::Start(0));
    ok && (is_tar(&head) || head.starts_with(SEVENZIP))
}

fn is_tar(head: &[u8; 512]) -> bool {
    &head[257..262] == b"ustar"
}

const SEVENZIP: &[u8] = b"7z\xbc\xaf\x27\x1c";

/// The CD image (`.iso`, `.bin`, `.img`) inside a packed medium, its parts in order. `None` if
/// the medium is not packed.
pub fn unpack(parts: Vec<Box<dyn ReadSeek>>) -> Result<Option<Image>> {
    let mut r: Box<dyn ReadSeek> = if parts.len() == 1 {
        parts.into_iter().next().expect("one part")
    } else {
        Box::new(Joined::new(parts)?)
    };
    if !is_packed(&mut *r) {
        return Ok(None);
    }
    // A tar holds the 7z (or the image): its first file of either.
    let mut head = [0u8; 512];
    r.read_exact(&mut head)?;
    r.seek(SeekFrom::Start(0))?;
    if is_tar(&head) {
        let (name, offset, size) = tar_member(&mut *r)?;
        let mut sub = SubReader::new(r, offset, size);
        let mut magic = [0u8; 6];
        sub.read_exact(&mut magic)?;
        sub.seek(SeekFrom::Start(0))?;
        if magic != SEVENZIP {
            if !is_image_name(&name) {
                return format_err(format!("tar: {name} is no CD image or 7z archive"));
            }
            let mut data = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
            sub.read_to_end(&mut data)?;
            return Ok(Some(Image(Arc::new(data))));
        }
        return sevenzip_image(&mut sub).map(Some);
    }
    sevenzip_image(&mut *r).map(Some)
}

fn is_image_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [".iso", ".bin", ".img", ".mdf"]
        .iter()
        .any(|e| n.ends_with(e))
}

/// The first regular file of a tar archive that is a 7z archive or a CD image: its name, where its
/// data starts and its size.
fn tar_member(r: &mut dyn ReadSeek) -> Result<(String, u64, u64)> {
    let mut at = 0u64;
    let mut head = [0u8; 512];
    loop {
        r.seek(SeekFrom::Start(at))?;
        r.read_exact(&mut head)?;
        if head.iter().all(|&b| b == 0) {
            return format_err("tar: no 7z archive or CD image in it");
        }
        let name_end = head[..100].iter().position(|&b| b == 0).unwrap_or(100);
        let mut name = String::from_utf8_lossy(&head[..name_end]).into_owned();
        let prefix_end = head[345..500].iter().position(|&b| b == 0).unwrap_or(155);
        if prefix_end > 0 {
            name = format!(
                "{}/{name}",
                String::from_utf8_lossy(&head[345..345 + prefix_end])
            );
        }
        let size = octal(&head[124..136])?;
        let regular = matches!(head[156], 0 | b'0');
        if regular && (name.to_ascii_lowercase().ends_with(".7z") || is_image_name(&name)) {
            return Ok((name, at + 512, size));
        }
        at += 512 + size.div_ceil(512) * 512;
    }
}

fn octal(b: &[u8]) -> Result<u64> {
    let s = String::from_utf8_lossy(b);
    let s = s.trim_matches(|c: char| c == '\0' || c == ' ');
    u64::from_str_radix(s, 8).or_else(|_| format_err(format!("tar: bad size {s:?}")))
}

// ---- 7z -------------------------------------------------------------------------------------

const K_END: u8 = 0x00;
const K_HEADER: u8 = 0x01;
const K_ARCHIVE_PROPERTIES: u8 = 0x02;
const K_MAIN_STREAMS_INFO: u8 = 0x04;
const K_FILES_INFO: u8 = 0x05;
const K_PACK_INFO: u8 = 0x06;
const K_UNPACK_INFO: u8 = 0x07;
const K_SUBSTREAMS_INFO: u8 = 0x08;
const K_SIZE: u8 = 0x09;
const K_CRC: u8 = 0x0a;
const K_FOLDER: u8 = 0x0b;
const K_CODERS_UNPACK_SIZE: u8 = 0x0c;
const K_NUM_UNPACK_STREAM: u8 = 0x0d;
const K_EMPTY_STREAM: u8 = 0x0e;
const K_NAME: u8 = 0x11;
const K_ENCODED_HEADER: u8 = 0x17;

/// The bytes of a 7z header, read in its own number format.
struct Bytes<'a> {
    b: &'a [u8],
    i: usize,
}

impl Bytes<'_> {
    fn byte(&mut self) -> Result<u8> {
        let v = *self
            .b
            .get(self.i)
            .ok_or_else(|| crate::Error::Format("7z: header too short".into()))?;
        self.i += 1;
        Ok(v)
    }

    fn take(&mut self, n: u64) -> Result<&[u8]> {
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        let end = self.i.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else {
            return format_err("7z: header too short");
        };
        let s = &self.b[self.i..end];
        self.i = end;
        Ok(s)
    }

    /// 7z's variable-length number: the high bits of the first byte say how many follow.
    fn num(&mut self) -> Result<u64> {
        let first = self.byte()?;
        let mut mask = 0x80u8;
        let mut v = 0u64;
        for i in 0..8 {
            if first & mask == 0 {
                let high = u64::from(first & mask.wrapping_sub(1));
                return Ok(v | high << (8 * i));
            }
            v |= u64::from(self.byte()?) << (8 * i);
            mask >>= 1;
        }
        Ok(v)
    }

    /// A vector of `n` bits, after an "all defined" byte when `all_byte`.
    fn bits(&mut self, n: usize, all_byte: bool) -> Result<Vec<bool>> {
        if all_byte && self.byte()? != 0 {
            return Ok(vec![true; n]);
        }
        let bytes = self.take(n.div_ceil(8) as u64)?;
        Ok((0..n)
            .map(|i| bytes[i / 8] & (0x80 >> (i % 8)) != 0)
            .collect())
    }
}

#[derive(Default)]
struct Folder {
    method: Vec<u8>,
    props: Vec<u8>,
    unpack_size: u64,
}

#[derive(Default)]
struct Streams {
    pack_pos: u64,
    pack_sizes: Vec<u64>,
    folders: Vec<Folder>,
    /// Streams per folder, and their sizes in order.
    per_folder: Vec<u64>,
    sizes: Vec<u64>,
}

fn streams_info(h: &mut Bytes) -> Result<Streams> {
    let mut s = Streams::default();
    loop {
        match h.byte()? {
            K_END => break,
            K_PACK_INFO => {
                s.pack_pos = h.num()?;
                let n = h.num()? as usize;
                loop {
                    match h.byte()? {
                        K_END => break,
                        K_SIZE => s.pack_sizes = (0..n).map(|_| h.num()).collect::<Result<_>>()?,
                        K_CRC => {
                            let defined = h.bits(n, true)?;
                            h.take(4 * defined.iter().filter(|&&d| d).count() as u64)?;
                        }
                        _ => return format_err("7z: unknown pack info"),
                    }
                }
            }
            K_UNPACK_INFO => {
                if h.byte()? != K_FOLDER {
                    return format_err("7z: no folders");
                }
                let n = h.num()? as usize;
                if h.byte()? != 0 {
                    return format_err("7z: external folders");
                }
                for _ in 0..n {
                    if h.num()? != 1 {
                        return format_err("7z: a folder of several coders (a filter)");
                    }
                    let flags = h.byte()?;
                    let method = h.take(u64::from(flags & 0x0f))?.to_vec();
                    if flags & 0x10 != 0 {
                        return format_err("7z: a coder of several streams");
                    }
                    let props = if flags & 0x20 != 0 {
                        let n = h.num()?;
                        h.take(n)?.to_vec()
                    } else {
                        Vec::new()
                    };
                    s.folders.push(Folder {
                        method,
                        props,
                        unpack_size: 0,
                    });
                }
                if h.byte()? != K_CODERS_UNPACK_SIZE {
                    return format_err("7z: no unpacked sizes");
                }
                for f in &mut s.folders {
                    f.unpack_size = h.num()?;
                }
                loop {
                    match h.byte()? {
                        K_END => break,
                        K_CRC => {
                            let defined = h.bits(n, true)?;
                            h.take(4 * defined.iter().filter(|&&d| d).count() as u64)?;
                        }
                        _ => return format_err("7z: unknown unpack info"),
                    }
                }
            }
            K_SUBSTREAMS_INFO => {
                s.per_folder = vec![1; s.folders.len()];
                loop {
                    match h.byte()? {
                        K_END => break,
                        K_NUM_UNPACK_STREAM => {
                            for n in &mut s.per_folder {
                                *n = h.num()?;
                            }
                        }
                        K_SIZE => {
                            for (f, &n) in s.folders.iter().zip(&s.per_folder) {
                                if n == 0 {
                                    continue;
                                }
                                let mut sum = 0;
                                for _ in 1..n {
                                    let x = h.num()?;
                                    sum += x;
                                    s.sizes.push(x);
                                }
                                s.sizes.push(f.unpack_size.saturating_sub(sum));
                            }
                        }
                        K_CRC => {
                            let n = s.per_folder.iter().sum::<u64>() as usize;
                            let defined = h.bits(n, true)?;
                            h.take(4 * defined.iter().filter(|&&d| d).count() as u64)?;
                        }
                        _ => return format_err("7z: unknown substreams info"),
                    }
                }
            }
            _ => return format_err("7z: unknown streams info"),
        }
    }
    if s.per_folder.is_empty() {
        s.per_folder = vec![1; s.folders.len()];
    }
    if s.sizes.is_empty() {
        s.sizes = s.folders.iter().map(|f| f.unpack_size).collect();
    }
    if s.pack_sizes.len() != s.folders.len() {
        return format_err("7z: packed streams and folders differ in number");
    }
    Ok(s)
}

/// Where the archive's packed streams start (after the 32-byte signature header).
const DATA: u64 = 32;

/// Decode folder `k` from its start, keeping `[skip, skip + want)` of its output.
fn decode(r: &mut dyn ReadSeek, s: &Streams, k: usize, skip: u64, want: u64) -> Result<Vec<u8>> {
    let f = &s.folders[k];
    let at = DATA + s.pack_pos + s.pack_sizes[..k].iter().sum::<u64>();
    r.seek(SeekFrom::Start(at))?;
    let mut packed = (&mut *r).take(s.pack_sizes[k]);
    let mut out = Keep {
        skip,
        want,
        pos: 0,
        data: Vec::with_capacity(usize::try_from(want).unwrap_or(0)),
    };
    let size = Some(skip + want);
    match f.method.as_slice() {
        [0x21] => {
            let mut input = (&f.props[..1.min(f.props.len())]).chain(&mut packed);
            lzma::decode_lzma2(&mut input, &mut out, size)?;
        }
        [0x03, 0x01, 0x01] => {
            let mut input = f.props.as_slice().chain(&mut packed);
            lzma::decode_lzma1(&mut input, &mut out, size)?;
        }
        [0x00] => {
            io::copy(&mut packed.take(skip + want), &mut out)?;
        }
        m => return format_err(format!("7z: unsupported method {m:02x?}")),
    }
    if (out.data.len() as u64) < want {
        return format_err("7z: the stream ends early");
    }
    Ok(out.data)
}

/// Output that keeps only a window of what is written.
struct Keep {
    skip: u64,
    want: u64,
    pos: u64,
    data: Vec<u8>,
}

impl Write for Keep {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let start = self.pos;
        let end = start + buf.len() as u64;
        let (a, b) = (self.skip.max(start), (self.skip + self.want).min(end));
        if b > a {
            self.data
                .extend_from_slice(&buf[(a - start) as usize..(b - start) as usize]);
        }
        self.pos = end;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The largest CD image in a 7z archive.
fn sevenzip_image(r: &mut dyn ReadSeek) -> Result<Image> {
    let mut sig = [0u8; 32];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut sig)?;
    if &sig[..6] != SEVENZIP {
        return format_err("not a 7z archive");
    }
    let next_offset = u64::from_le_bytes(sig[12..20].try_into().expect("8 bytes"));
    let next_size = u64::from_le_bytes(sig[20..28].try_into().expect("8 bytes"));
    if next_size > 1 << 28 {
        return format_err("7z: header too large");
    }
    r.seek(SeekFrom::Start(DATA + next_offset))?;
    let mut header = vec![0u8; next_size as usize];
    r.read_exact(&mut header)?;
    // A compressed header: its streams, then the header they hold.
    while header.first() == Some(&K_ENCODED_HEADER) {
        let mut h = Bytes { b: &header, i: 1 };
        let s = streams_info(&mut h)?;
        let size = s.folders.first().map_or(0, |f| f.unpack_size);
        header = decode(r, &s, 0, 0, size)?;
    }
    let mut h = Bytes { b: &header, i: 0 };
    if h.byte()? != K_HEADER {
        return format_err("7z: no header");
    }
    let mut id = h.byte()?;
    if id == K_ARCHIVE_PROPERTIES {
        while h.byte()? != K_END {
            let n = h.num()?;
            h.take(n)?;
        }
        id = h.byte()?;
    }
    if id != K_MAIN_STREAMS_INFO {
        return format_err("7z: no streams");
    }
    let s = streams_info(&mut h)?;
    if h.byte()? != K_FILES_INFO {
        return format_err("7z: no file list");
    }
    let files = h.num()? as usize;
    let mut names: Vec<String> = Vec::new();
    let mut empty = vec![false; files];
    loop {
        let id = h.byte()?;
        if id == K_END {
            break;
        }
        let size = h.num()?;
        let body = h.take(size)?;
        match id {
            K_EMPTY_STREAM => {
                empty = Bytes { b: body, i: 0 }.bits(files, false)?;
            }
            K_NAME => {
                if body.first() != Some(&0) {
                    return format_err("7z: external names");
                }
                let units: Vec<u16> = body[1..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| u16::from_le_bytes(c))
                    .collect();
                names = units
                    .split(|&u| u == 0)
                    .take(files)
                    .map(String::from_utf16_lossy)
                    .collect();
            }
            _ => {}
        }
    }
    // Each file with data is the next stream; the streams fill the folders in order.
    let mut stream = 0usize;
    let mut best: Option<(u64, usize, u64)> = None; // size, folder, offset in it
    let mut folder = 0usize;
    let mut in_folder = 0u64;
    let mut offset = 0u64;
    for (i, &e) in empty.iter().enumerate() {
        if e {
            continue;
        }
        while folder < s.per_folder.len() && in_folder >= s.per_folder[folder] {
            folder += 1;
            in_folder = 0;
            offset = 0;
        }
        let Some(&size) = s.sizes.get(stream) else {
            return format_err("7z: more files than streams");
        };
        let name = names.get(i).map_or("", String::as_str);
        if is_image_name(name) && best.is_none_or(|b| size > b.0) {
            best = Some((size, folder, offset));
        }
        offset += size;
        in_folder += 1;
        stream += 1;
    }
    let Some((size, folder, offset)) = best else {
        return format_err("7z: no CD image in it");
    };
    Ok(Image(Arc::new(decode(r, &s, folder, offset, size)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tar_of(name: &str, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512 + 1024, 0);
        out
    }

    /// 7z's number format, up to 0x3fff.
    fn num(n: usize) -> Vec<u8> {
        if n < 0x80 {
            vec![n as u8]
        } else {
            vec![0x80 | (n >> 8) as u8, n as u8]
        }
    }

    /// A stored 7z of one file (method copy, header not compressed).
    fn sevenzip_of(name: &str, data: &[u8]) -> Vec<u8> {
        let mut hdr = vec![K_HEADER, K_MAIN_STREAMS_INFO];
        hdr.extend([K_PACK_INFO, 0, 1, K_SIZE]);
        hdr.extend(num(data.len()));
        hdr.extend([K_END]);
        hdr.extend([K_UNPACK_INFO, K_FOLDER, 1, 0, 1, 0x01, 0x00]);
        hdr.push(K_CODERS_UNPACK_SIZE);
        hdr.extend(num(data.len()));
        hdr.extend([K_END, K_END]);
        let units: Vec<u8> = name
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        hdr.extend([K_FILES_INFO, 1, K_NAME, units.len() as u8 + 1, 0]);
        hdr.extend(units);
        hdr.extend([K_END, K_END]);
        let mut out = SEVENZIP.to_vec();
        out.extend([0, 4]);
        out.extend([0; 4]);
        out.extend((data.len() as u64).to_le_bytes());
        out.extend((hdr.len() as u64).to_le_bytes());
        out.extend([0; 4]);
        out.extend_from_slice(data);
        out.extend(hdr);
        out
    }

    #[test]
    fn joined_parts_read_as_one() {
        let a: Box<dyn ReadSeek> = Box::new(Cursor::new(vec![1u8, 2, 3]));
        let b: Box<dyn ReadSeek> = Box::new(Cursor::new(vec![4u8, 5]));
        let mut j = Joined::new(vec![a, b]).unwrap();
        let mut all = Vec::new();
        j.read_to_end(&mut all).unwrap();
        assert_eq!(all, [1, 2, 3, 4, 5]);
        j.seek(SeekFrom::Start(2)).unwrap();
        let mut two = [0u8; 2];
        j.read_exact(&mut two).unwrap();
        assert_eq!(two, [3, 4]);
    }

    #[test]
    fn image_in_a_7z_in_a_split_tar() {
        let image: Vec<u8> = (0..200u8).collect();
        let tar = tar_of("cd.7z", &sevenzip_of("x/cd.iso", &image));
        let (a, b) = tar.split_at(700);
        let parts: Vec<Box<dyn ReadSeek>> = vec![
            Box::new(Cursor::new(a.to_vec())),
            Box::new(Cursor::new(b.to_vec())),
        ];
        let got = unpack(parts).unwrap().unwrap();
        assert_eq!(got.as_ref(), &image[..]);
    }

    #[test]
    fn plain_media_are_not_packed() {
        let iso: Box<dyn ReadSeek> = Box::new(Cursor::new(vec![0u8; 4096]));
        assert!(unpack(vec![iso]).unwrap().is_none());
    }
}
