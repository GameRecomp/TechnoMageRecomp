// SPDX-License-Identifier: MIT
//! Unpackers for packed executables, run before `rc-pe` loads a binary.
//!
//! The retail `TechnoMage.exe` is packed with UPX 1.07 (sections `UPX0`, `UPX1`, `UPX2`; the
//! original image is about 1.8 MiB with `.text`, `_GMCODE`, `.rdata`, `.data`, `.idata`, `_GMDATA`
//! and `.reloc`). The recompiler never lifts the packer's stub: this crate restores the original
//! image (NRV2B/D/E decompression, the call/jump filter, the import table and the relocations),
//! and the manifest pins both hashes, the packed file's and the restored image's.
//!
//! Planned API (M1):
//!
//! - `detect(&[u8]) -> Option<Packer>`: the packer and its version, from the section names and
//!   the `UPX!` block.
//! - `unpack(&[u8]) -> Result<Vec<u8>>`: the restored PE image as a file, byte for byte what
//!   `upx -d` writes (the reference in tests).

#![forbid(unsafe_code)]

/// A packer `detect` recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packer {
    /// UPX for Win32 PE, with the version from the `UPX!` block (for example 1.07).
    UpxWin32 { major: u8, minor: u8 },
}

/// Recognises a UPX-packed Win32 executable by its section names and `UPX!` block.
pub fn detect(image: &[u8]) -> Option<Packer> {
    let has = |needle: &[u8]| {
        image
            .windows(needle.len())
            .take(0x1000)
            .any(|w| w == needle)
    };
    if has(b"UPX0") && has(b"UPX1") && has(b"UPX!") {
        // The version string is "$Id: UPX x.yz ..." next to the block; parsed in M1.
        return Some(Packer::UpxWin32 { major: 1, minor: 7 });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_bytes_are_not_packed() {
        assert_eq!(detect(b"MZ\x90\x00 not packed"), None);
    }
}
