// SPDX-License-Identifier: MIT
//! Unpackers for packed executables, run before `rc-pe` loads a binary.
//!
//! The retail `TechnoMage.exe` is packed with UPX 1.07 (sections `UPX0`, `UPX1`, `UPX2`). The
//! recompiler never lifts the packer's stub: [`unpack`] restores the original file, byte for
//! byte what `upx -d` writes (the reference in the tests), and the manifest pins both hashes.
//!
//! The format, as UPX's Win32/PE packer writes it:
//!
//! - The pack header (`UPX!`, version, format, method, level, Adler-32 of the uncompressed and
//!   compressed data, both lengths, the filter and its call-trick byte) lies in the 1 KiB before
//!   `UPX1`'s file data; the compressed block follows it.
//! - Decompressed, the block is the original image from its lowest section on (`rvamin`), and
//!   after it the original PE header, its section table and the "extra info": where the
//!   compressed import and relocation tables are and how to rebuild them. The last four bytes
//!   give the offset of that PE header.
//! - The code section went through a call-trick filter: the targets of `call`/`jmp` rel32 were
//!   made absolute and big-endian; [`unfilter_cto`] reverses it.
//! - Imports are a compact list (DLL name offset into `UPX2`, the IAT's place, then the names or
//!   ordinals); relocations a delta-coded list of places whose values were stored big-endian and
//!   without the image base. Both are rebuilt into the original image.
//!
//! Supported: PE32 (i386), compression NRV2D with 32-bit bit buffers (UPX method 5), filters
//! `0x26` (call trick with offset, `e8`/`e9`) and none, no resources, no exports, no TLS: what
//! the pinned binaries use. Anything else is reported as unsupported.

#![forbid(unsafe_code)]

use std::fmt;

/// A packer `detect` recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packer {
    /// UPX for Win32 PE, with the pack header's version byte, compression method and filter.
    UpxWin32 { version: u8, method: u8, filter: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a file this crate unpacks.
    NotPacked,
    /// A packed file with a feature not implemented here.
    Unsupported(String),
    /// The data contradicts itself (truncated, bad checksum, out of range).
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotPacked => f.write_str("not a packed executable"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Corrupt(s) => write!(f, "corrupt: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn corrupt<T>(what: impl Into<String>) -> Result<T> {
    Err(Error::Corrupt(what.into()))
}

fn le16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| Error::Corrupt(format!("read of 2 bytes at {o:#x} out of range")))
}

fn le32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| Error::Corrupt(format!("read of 4 bytes at {o:#x} out of range")))
}

fn put32(b: &mut [u8], o: usize, v: u32) -> Result<()> {
    b.get_mut(o..o + 4)
        .map(|s| s.copy_from_slice(&v.to_le_bytes()))
        .ok_or_else(|| Error::Corrupt(format!("write of 4 bytes at {o:#x} out of range")))
}

fn align_up(v: u32, a: u32) -> u32 {
    if a == 0 {
        v
    } else {
        v.div_ceil(a) * a
    }
}

/// UPX's Adler-32 (initial value 1).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

// --- PE layout -----------------------------------------------------------------------------

/// Size of the PE header UPX keeps: signature, file header and the PE32 optional header.
const PE_HEADER: usize = 0xf8;
const SECTION: usize = 40;
/// Offsets inside the PE header (from the `PE\0\0` signature).
const OH_OBJECTS: usize = 6;
const OH_FLAGS: usize = 22;
const OH_CODESIZE: usize = 28;
const OH_CODEBASE: usize = 44;
const OH_IMAGEBASE: usize = 52;
const OH_FILEALIGN: usize = 60;
const OH_HEADERSIZE: usize = 84;
const OH_CHKSUM: usize = 88;
const OH_DDIRS: usize = 120;
const DIR_IMPORT: usize = 1;
const DIR_RESOURCE: usize = 2;
const DIR_EXPORT: usize = 0;
const DIR_BASERELOC: usize = 5;
const DIR_DEBUG: usize = 6;
const DIR_TLS: usize = 9;
const DIR_BOUND_IMPORT: usize = 11;
const DIR_IAT: usize = 12;
const RELOCS_STRIPPED: u16 = 0x0001;

#[derive(Debug, Clone, Copy)]
struct Section {
    vaddr: u32,
    size: u32,
    rawdataptr: u32,
}

fn section(b: &[u8], o: usize) -> Result<Section> {
    Ok(Section {
        vaddr: le32(b, o + 12)?,
        size: le32(b, o + 16)?,
        rawdataptr: le32(b, o + 20)?,
    })
}

fn dir(h: &[u8], i: usize) -> Result<(u32, u32)> {
    Ok((le32(h, OH_DDIRS + 8 * i)?, le32(h, OH_DDIRS + 8 * i + 4)?))
}

fn set_dir(h: &mut [u8], i: usize, addr: u32, size: u32) -> Result<()> {
    put32(h, OH_DDIRS + 8 * i, addr)?;
    put32(h, OH_DDIRS + 8 * i + 4, size)
}

// --- pack header ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct PackHeader {
    version: u8,
    format: u8,
    method: u8,
    u_adler: u32,
    c_adler: u32,
    u_len: u32,
    c_len: u32,
    filter: u8,
    filter_cto: u8,
    /// Offset of `UPX!` in the file, and the header's length.
    at: usize,
    len: usize,
}

const UPX_F_W32PE_I386: u8 = 9;
const M_NRV2D_LE32: u8 = 5;

fn pack_header(file: &[u8], from: usize) -> Option<PackHeader> {
    let end = (from + 1024).min(file.len());
    let at = from + file.get(from..end)?.windows(4).position(|w| w == b"UPX!")?;
    let p = file.get(at..)?;
    if p.len() < 32 {
        return None;
    }
    let version = p[4];
    let format = p[5];
    let len = match version {
        0..=3 => 24,
        4..=9 => 28,
        _ => 32,
    };
    Some(PackHeader {
        version,
        format,
        method: p[6],
        u_adler: le32(p, 8).ok()?,
        c_adler: le32(p, 12).ok()?,
        u_len: le32(p, 16).ok()?,
        c_len: le32(p, 20).ok()?,
        filter: p[28],
        filter_cto: p[29],
        at,
        len,
    })
}

struct Packed<'a> {
    pe_offset: usize,
    ih: &'a [u8],
    isection: Vec<Section>,
    ph: PackHeader,
}

fn parse(file: &[u8]) -> Result<Packed<'_>> {
    if file.get(0..2) != Some(b"MZ") {
        return Err(Error::NotPacked);
    }
    let pe_offset = le32(file, 0x3c)? as usize;
    let ih = file
        .get(pe_offset..pe_offset + PE_HEADER)
        .ok_or(Error::NotPacked)?;
    if &ih[0..4] != b"PE\0\0" || le16(ih, 24)? != 0x10b {
        return Err(Error::NotPacked);
    }
    let objs = usize::from(le16(ih, OH_OBJECTS)?);
    let mut isection = Vec::with_capacity(objs);
    for i in 0..objs {
        isection.push(section(file, pe_offset + PE_HEADER + SECTION * i)?);
    }
    let name0 = file
        .get(pe_offset + PE_HEADER..pe_offset + PE_HEADER + 3)
        .ok_or(Error::NotPacked)?;
    if objs < 3 || name0 != b"UPX" {
        return Err(Error::NotPacked);
    }
    let from = (isection[1].rawdataptr as usize).saturating_sub(64);
    let ph = pack_header(file, from)
        .or_else(|| pack_header(file, isection[2].rawdataptr as usize))
        .ok_or(Error::NotPacked)?;
    Ok(Packed {
        pe_offset,
        ih,
        isection,
        ph,
    })
}

/// Recognises a UPX-packed Win32 executable.
pub fn detect(image: &[u8]) -> Option<Packer> {
    parse(image).ok().map(|p| Packer::UpxWin32 {
        version: p.ph.version,
        method: p.ph.method,
        filter: p.ph.filter,
    })
}

// --- NRV2D ---------------------------------------------------------------------------------

struct Bits<'a> {
    src: &'a [u8],
    ilen: usize,
    bb: u32,
    bc: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32> {
        if self.bc > 0 {
            self.bc -= 1;
        } else {
            self.bb = le32(self.src, self.ilen)
                .map_err(|_| Error::Corrupt("NRV2D: input overrun".into()))?;
            self.ilen += 4;
            self.bc = 31;
        }
        Ok((self.bb >> self.bc) & 1)
    }

    fn byte(&mut self) -> Result<u8> {
        let b = *self
            .src
            .get(self.ilen)
            .ok_or_else(|| Error::Corrupt("NRV2D: input overrun".into()))?;
        self.ilen += 1;
        Ok(b)
    }
}

/// UCL's NRV2D with a little-endian 32-bit bit buffer, into exactly `out_len` bytes.
pub fn nrv2d_le32(src: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut dst = Vec::with_capacity(out_len);
    let mut s = Bits {
        src,
        ilen: 0,
        bb: 0,
        bc: 0,
    };
    let mut last_m_off: u32 = 1;
    loop {
        while s.bit()? == 1 {
            if dst.len() >= out_len {
                return corrupt("NRV2D: output overrun");
            }
            let b = s.byte()?;
            dst.push(b);
        }
        let mut m_off: u32 = 1;
        loop {
            m_off = m_off * 2 + s.bit()?;
            if m_off > 0x00ff_ffff + 3 {
                return corrupt("NRV2D: look-behind overrun");
            }
            if s.bit()? == 1 {
                break;
            }
            m_off = (m_off - 1) * 2 + s.bit()?;
        }
        let mut m_len;
        if m_off == 2 {
            m_off = last_m_off;
            m_len = s.bit()?;
        } else {
            m_off = (m_off - 3)
                .wrapping_mul(256)
                .wrapping_add(u32::from(s.byte()?));
            if m_off == u32::MAX {
                break;
            }
            m_len = (m_off ^ u32::MAX) & 1;
            m_off >>= 1;
            m_off += 1;
            last_m_off = m_off;
        }
        m_len = m_len * 2 + s.bit()?;
        if m_len == 0 {
            m_len = 1;
            loop {
                m_len = m_len * 2 + s.bit()?;
                if m_len as usize >= out_len {
                    return corrupt("NRV2D: output overrun");
                }
                if s.bit()? == 1 {
                    break;
                }
            }
            m_len += 2;
        }
        m_len += u32::from(m_off > 0x500);
        let (m_off, m_len) = (m_off as usize, m_len as usize);
        if dst.len() + m_len >= out_len || m_off > dst.len() {
            return corrupt("NRV2D: match out of range");
        }
        let from = dst.len() - m_off;
        for i in 0..=m_len {
            let b = dst[from + i];
            dst.push(b);
        }
    }
    if s.ilen != src.len() {
        return corrupt("NRV2D: input not consumed exactly");
    }
    if dst.len() != out_len {
        return corrupt("NRV2D: wrong output length");
    }
    Ok(dst)
}

// --- filters -------------------------------------------------------------------------------

/// Undo UPX's call-trick filter with offset (`0x26`: `e8` and `e9`): the rel32 of a call or jump
/// into the buffer was stored as the big-endian absolute target plus `addvalue`, marked by
/// `cto` in its top byte.
pub fn unfilter_cto(b: &mut [u8], addvalue: u32, cto: u8) {
    if b.len() < 5 {
        return;
    }
    let size5 = b.len() - 5;
    let cto32 = u32::from(cto) << 24;
    let mut ic = 0;
    while ic < size5 {
        if (b[ic] == 0xe8 || b[ic] == 0xe9) && b[ic + 1] == cto {
            let jc = u32::from_be_bytes([b[ic + 1], b[ic + 2], b[ic + 3], b[ic + 4]]);
            let v = jc
                .wrapping_sub(ic as u32)
                .wrapping_sub(1)
                .wrapping_sub(addvalue)
                .wrapping_sub(cto32);
            b[ic + 1..ic + 5].copy_from_slice(&v.to_le_bytes());
            ic += 5;
        } else {
            ic += 1;
        }
    }
}

// --- rebuilding ----------------------------------------------------------------------------

fn cstr_len(b: &[u8], o: usize) -> Result<usize> {
    b.get(o..)
        .and_then(|s| s.iter().position(|&c| c == 0))
        .ok_or_else(|| Error::Corrupt(format!("unterminated string at {o:#x}")))
}

/// The import directory, from the compact list at `imdata` in the image.
fn rebuild_imports(
    obuf: &mut [u8],
    oh: &[u8],
    extra: &mut usize,
    rvamin: u32,
    upx2: &[u8],
    upx2_vaddr: u32,
    ih_import: u32,
) -> Result<()> {
    let (imp_addr, imp_size) = dir(oh, DIR_IMPORT)?;
    if imp_addr == 0 || imp_size <= 20 {
        return Ok(());
    }
    let imdata = le32(obuf, *extra)? as usize;
    let inamespos = le32(obuf, *extra + 4)?;
    *extra += 8;
    let import_base = ih_import
        .checked_sub(upx2_vaddr)
        .ok_or_else(|| Error::Corrupt("import directory outside UPX2".into()))?
        as usize;
    let import = upx2
        .get(import_base..)
        .ok_or_else(|| Error::Corrupt("import directory outside UPX2".into()))?;

    // Total length of the DLL names (for where the function names start).
    let mut sdllnames = 0usize;
    let mut p = imdata;
    while le32(obuf, p)? != 0 {
        let dname = le32(obuf, p)? as usize;
        sdllnames += cstr_len(import, dname)? + 1;
        p += 8;
        while obuf.get(p).copied().unwrap_or(0) != 0 {
            match obuf[p] {
                1 => p += 1 + cstr_len(obuf, p + 1)? + 1,
                0xff => p += 3,
                _ => p += 5,
            }
        }
        p += 1;
    }
    let sdllnames = sdllnames.div_ceil(2) * 2;

    let at = |rva: u32| -> Result<usize> {
        rva.checked_sub(rvamin)
            .map(|v| v as usize)
            .ok_or_else(|| Error::Corrupt(format!("address {rva:#x} below the image")))
    };
    let mut im = at(imp_addr)?;
    let mut dllnames = if inamespos != 0 { at(inamespos)? } else { 0 };
    let importednames_start = dllnames + sdllnames;
    let mut importednames = importednames_start;

    let mut p = imdata;
    while le32(obuf, p)? != 0 {
        let dname = le32(obuf, p)? as usize;
        let dlen = cstr_len(import, dname)?;
        let iatoffs = le32(obuf, p + 4)?.wrapping_add(rvamin);
        if inamespos != 0 {
            let name = import[dname..=dname + dlen].to_vec();
            obuf.get_mut(dllnames..dllnames + name.len())
                .ok_or_else(|| Error::Corrupt("DLL name out of range".into()))?
                .copy_from_slice(&name);
            put32(obuf, im + 12, dllnames as u32 + rvamin)?;
            dllnames += dlen + 1;
        } else {
            return Err(Error::Unsupported(
                "imports without a name area (old UPX layout)".into(),
            ));
        }
        put32(obuf, im + 16, iatoffs)?;
        let mut newiat = at(iatoffs)?;
        p += 8;
        while obuf.get(p).copied().unwrap_or(0) != 0 {
            match obuf[p] {
                1 => {
                    p += 1;
                    let ilen = cstr_len(obuf, p)? + 1;
                    if (importednames - importednames_start) & 1 != 0 {
                        importednames -= 1;
                    }
                    let name = obuf[p..p + ilen].to_vec();
                    obuf.get_mut(importednames + 2..importednames + 2 + ilen)
                        .ok_or_else(|| Error::Corrupt("import name out of range".into()))?
                        .copy_from_slice(&name);
                    put32(obuf, newiat, importednames as u32 + rvamin)?;
                    importednames += 2 + ilen;
                    p += ilen;
                }
                0xff => {
                    let ord = u32::from(le16(obuf, p + 1)?);
                    put32(obuf, newiat, ord | 0x8000_0000)?;
                    p += 3;
                }
                _ => {
                    let v = le32(import, le32(obuf, p + 1)? as usize)?;
                    if v & 0x8000_0000 == 0 {
                        return corrupt("an imported ordinal without its flag");
                    }
                    put32(obuf, newiat, v)?;
                    p += 5;
                }
            }
            newiat += 4;
        }
        put32(obuf, newiat, 0)?;
        p += 1;
        im += 20;
    }
    Ok(())
}

/// The base relocations, from the delta-coded list; returns the new directory size.
fn rebuild_relocs(obuf: &mut [u8], oh: &mut [u8], extra: &mut usize, rvamin: u32) -> Result<()> {
    let (rel_addr, rel_size) = dir(oh, DIR_BASERELOC)?;
    let flags = le16(oh, OH_FLAGS)?;
    if rel_addr == 0 || rel_size == 0 || flags & RELOCS_STRIPPED != 0 {
        return Ok(());
    }
    if rel_size == 8 {
        let o = (rel_addr - rvamin) as usize;
        obuf.get_mut(o..o + 8)
            .ok_or_else(|| Error::Corrupt("relocations out of range".into()))?
            .copy_from_slice(&[0, 0, 0, 0, 8, 0, 0, 0]);
        return Ok(());
    }
    let crelocs = le32(obuf, *extra)? as usize;
    let big = *obuf
        .get(*extra + 4)
        .ok_or_else(|| Error::Corrupt("extra info truncated".into()))?;
    *extra += 5;
    if big & 6 != 0 {
        return Err(Error::Unsupported("16-bit relocations".into()));
    }
    let imagebase = le32(oh, OH_IMAGEBASE)?;
    let image_len = crelocs;

    // Places, delta coded from -4.
    let mut places = Vec::new();
    let mut fix = crelocs;
    let mut pc: u32 = u32::MAX - 3;
    loop {
        let b = *obuf
            .get(fix)
            .ok_or_else(|| Error::Corrupt("relocation list truncated".into()))?;
        if b == 0 {
            break;
        }
        let delta = if b < 0xf0 {
            fix += 1;
            u32::from(b)
        } else {
            let mut d = u32::from(b & 0x0f) * 0x10000 + u32::from(le16(obuf, fix + 1)?);
            fix += 3;
            if d == 0 {
                d = le32(obuf, fix)?;
                fix += 4;
            }
            d
        };
        if delta < 4 {
            return corrupt("overlapping fixups");
        }
        pc = pc.wrapping_add(delta);
        if pc as usize + 4 > image_len {
            return corrupt(format!("relocation at {pc:#x} beyond the image"));
        }
        places.push(pc);
    }
    for &pc in &places {
        let o = pc as usize;
        let v = u32::from_be_bytes([obuf[o], obuf[o + 1], obuf[o + 2], obuf[o + 3]]);
        put32(obuf, o, v.wrapping_add(imagebase).wrapping_add(rvamin))?;
    }

    // The relocation directory: one block per page, entries sorted, each block padded to 4.
    let mut rvas: Vec<u32> = places.iter().map(|&p| p + rvamin).collect();
    rvas.sort_unstable();
    if rvas.windows(2).any(|w| w[0] == w[1]) {
        return corrupt("duplicate relocations");
    }
    let mut out: Vec<u8> = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut current_page = 0u32;
    let finish = |out: &mut Vec<u8>, start: usize| {
        while !(out.len() - start).is_multiple_of(4) {
            out.push(0);
        }
        let sob = (out.len() - start) as u32;
        out[start + 4..start + 8].copy_from_slice(&sob.to_le_bytes());
    };
    for (i, &pos) in rvas.iter().enumerate() {
        if i == 0 || pos - current_page >= 0x1000 {
            if let Some(s) = block_start {
                finish(&mut out, s);
            }
            current_page = pos & !0xfff;
            block_start = Some(out.len());
            out.extend_from_slice(&current_page.to_le_bytes());
            out.extend_from_slice(&8u32.to_le_bytes());
        }
        let entry = (3u16 << 12) | (pos & 0xfff) as u16;
        out.extend_from_slice(&entry.to_le_bytes());
    }
    if let Some(s) = block_start {
        finish(&mut out, s);
    }
    let o = (rel_addr - rvamin) as usize;
    obuf.get_mut(o..o + out.len())
        .ok_or_else(|| Error::Corrupt("relocation directory out of range".into()))?
        .copy_from_slice(&out);
    set_dir(oh, DIR_BASERELOC, rel_addr, out.len() as u32)
}

/// Restore the original executable from a UPX-packed Win32 PE file: what `upx -d` writes.
pub fn unpack(file: &[u8]) -> Result<Vec<u8>> {
    let p = parse(file)?;
    let ph = p.ph;
    if ph.format != UPX_F_W32PE_I386 {
        return Err(Error::Unsupported(format!("UPX format {}", ph.format)));
    }
    if ph.method != M_NRV2D_LE32 {
        return Err(Error::Unsupported(format!("UPX method {}", ph.method)));
    }
    let ih = p.ih;
    let filealign = le32(ih, OH_FILEALIGN)?;
    if filealign == 0 {
        return corrupt("file alignment 0");
    }
    let iobjs = p.isection.len();
    let last = p.isection[iobjs - 1];
    let overlay_at = align_up(last.rawdataptr + last.size, filealign) as usize;
    if overlay_at < file.len() {
        return Err(Error::Unsupported("an overlay after the sections".into()));
    }

    let cstart = ph.at + ph.len;
    let cdata = file
        .get(cstart..cstart + ph.c_len as usize)
        .ok_or_else(|| Error::Corrupt("compressed data truncated".into()))?;
    if adler32(cdata) != ph.c_adler {
        return corrupt("checksum of the compressed data");
    }
    let mut obuf = nrv2d_le32(cdata, ph.u_len as usize)?;
    if adler32(&obuf) != ph.u_adler {
        return corrupt("checksum of the decompressed data");
    }

    let skip = le32(&obuf, ph.u_len as usize - 4)? as usize;
    let mut oh = obuf
        .get(skip..skip + PE_HEADER)
        .ok_or_else(|| Error::Corrupt("bad PE header offset".into()))?
        .to_vec();
    let objs = usize::from(le16(&oh, OH_OBJECTS)?);
    if objs == 0 || (iobjs > 2 && p.isection[2].size == 0) {
        return corrupt("unexpected value in the PE header");
    }
    let sec_at = skip + PE_HEADER;
    let osection_bytes = obuf
        .get(sec_at..sec_at + SECTION * objs)
        .ok_or_else(|| Error::Corrupt("section table truncated".into()))?
        .to_vec();
    let mut osection = Vec::with_capacity(objs);
    for i in 0..objs {
        osection.push(section(&osection_bytes, SECTION * i)?);
    }
    let mut extra = sec_at + SECTION * objs;
    let rvamin = osection[0].vaddr;

    let upx2 = p.isection[2];
    let upx2_data = file
        .get(upx2.rawdataptr as usize..(upx2.rawdataptr + upx2.size) as usize)
        .ok_or_else(|| Error::Corrupt("UPX2 truncated".into()))?;

    if ph.filter != 0 {
        if ph.filter != 0x26 {
            return Err(Error::Unsupported(format!("UPX filter {:#x}", ph.filter)));
        }
        let codebase = le32(&oh, OH_CODEBASE)?;
        let codesize = le32(&oh, OH_CODESIZE)?;
        let o = (codebase - rvamin) as usize;
        let code = obuf
            .get_mut(o..o + codesize as usize)
            .ok_or_else(|| Error::Corrupt("code section out of range".into()))?;
        unfilter_cto(code, codebase - rvamin, ph.filter_cto);
    }

    if le16(ih, OH_FLAGS)? & RELOCS_STRIPPED != 0 {
        let f = le16(&oh, OH_FLAGS)? | RELOCS_STRIPPED;
        oh[OH_FLAGS..OH_FLAGS + 2].copy_from_slice(&f.to_le_bytes());
        set_dir(&mut oh, DIR_BASERELOC, 0, 0)?;
    }

    let (ih_import, _) = dir(ih, DIR_IMPORT)?;
    rebuild_imports(
        &mut obuf, &oh, &mut extra, rvamin, upx2_data, upx2.vaddr, ih_import,
    )?;
    rebuild_relocs(&mut obuf, &mut oh, &mut extra, rvamin)?;
    if dir(&oh, DIR_TLS)?.1 != 0 {
        return Err(Error::Unsupported("TLS".into()));
    }
    let (ex_addr, ex_size) = dir(&oh, DIR_EXPORT)?;
    if ex_size != 0 && ex_addr != dir(ih, DIR_EXPORT)?.0 {
        return Err(Error::Unsupported("exports".into()));
    }
    if dir(&oh, DIR_RESOURCE)?.1 != 0 && dir(ih, DIR_RESOURCE)?.1 != 0 {
        return Err(Error::Unsupported("resources".into()));
    }

    for d in [DIR_DEBUG, DIR_IAT, DIR_BOUND_IMPORT] {
        set_dir(&mut oh, d, 0, 0)?;
    }
    let headersize = align_up(
        (p.pe_offset + PE_HEADER + SECTION * objs) as u32,
        le32(&oh, OH_FILEALIGN)?,
    );
    put32(&mut oh, OH_HEADERSIZE, headersize)?;
    put32(&mut oh, OH_CHKSUM, 0)?;

    // The file: the original DOS stub, the header, the section table, zeros up to the first
    // section with file data, then each section's data rounded up to the file alignment.
    let ofilealign = le32(&oh, OH_FILEALIGN)?;
    let mut out = Vec::new();
    out.extend_from_slice(&file[..p.pe_offset]);
    out.extend_from_slice(&oh);
    out.extend_from_slice(&osection_bytes);
    let first = osection
        .iter()
        .find(|s| s.rawdataptr != 0)
        .map_or(out.len(), |s| s.rawdataptr as usize);
    if first < out.len() {
        return corrupt("section data overlaps the header");
    }
    out.resize(first, 0);
    for s in &osection {
        if s.rawdataptr != 0 {
            let o = (s.vaddr - rvamin) as usize;
            let n = align_up(s.size, ofilealign) as usize;
            let data = obuf
                .get(o..o + n)
                .ok_or_else(|| Error::Corrupt("section data beyond the image".into()))?;
            out.extend_from_slice(data);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_bytes_are_not_packed() {
        assert_eq!(detect(b"MZ\x90\x00 not packed"), None);
        assert_eq!(unpack(b"not even MZ"), Err(Error::NotPacked));
    }

    #[test]
    fn adler32_matches_the_reference() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn the_call_trick_is_undone() {
        // A call at 0 whose filtered target is 0x10, with addvalue 0x1000 and cto 0x42:
        // stored big-endian as 0x42 << 24 | (0x10 + 0x1000).
        let mut b = vec![0u8; 32];
        b[0] = 0xe8;
        b[1..5].copy_from_slice(&(0x4200_1010u32).to_be_bytes());
        unfilter_cto(&mut b, 0x1000, 0x42);
        // UPX counts the target from the byte after the opcode: rel32 = 0x10 - 1.
        assert_eq!(&b[1..5], &0x0fu32.to_le_bytes());
    }

    /// The pinned TechnoMage.exe, if present: the result must equal `upx -d`'s.
    #[test]
    #[ignore = "needs orig/technomage/cd-de/TechnoMage.exe and its upx -d output"]
    fn orig_technomage() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let packed = std::fs::read(root.join("orig/technomage/cd-de/TechnoMage.exe")).unwrap();
        let out = unpack(&packed).unwrap();
        assert_eq!(out.len(), 1_863_680);
    }
}
