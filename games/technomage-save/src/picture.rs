// SPDX-License-Identifier: MIT
//! A saved game's picture: 160 × 120 pixels, a byte each, red in the top three bits, green in
//! the next three, blue in the low two (what the game takes from the screen, every fourth
//! pixel). Written and read as binary PPM (P6).

pub const WIDTH: usize = 160;
pub const HEIGHT: usize = 120;
pub const LEN: usize = WIDTH * HEIGHT;

/// The picture's bytes as RGB, three bytes a pixel (each channel spread over 0..=255).
pub fn to_rgb(px: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 3);
    for &p in px {
        let (r, g, b) = (u32::from(p >> 5), u32::from(p >> 2 & 7), u32::from(p & 3));
        out.extend_from_slice(&[
            (r * 255 / 7) as u8,
            (g * 255 / 7) as u8,
            (b * 255 / 3) as u8,
        ]);
    }
    out
}

/// RGB, three bytes a pixel, to the picture's bytes (each channel rounded to the nearest level).
pub fn from_rgb(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            let level = |v: u8, max: u32| ((u32::from(v) * max + 127) / 255) as u8;
            level(c[0], 7) << 5 | level(c[1], 7) << 2 | level(c[2], 3)
        })
        .collect()
}

pub fn to_ppm(px: &[u8]) -> Vec<u8> {
    let mut out = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    out.extend(to_rgb(px));
    out
}

/// A binary PPM of exactly 160 × 120 pixels (maximum value 255) to the picture's bytes.
pub fn from_ppm(data: &[u8]) -> Result<Vec<u8>, String> {
    // The header: P6, width, height, maximum, each separated by white space (comments allowed),
    // then one white space character before the pixels.
    let mut pos = 0;
    let mut tokens = Vec::new();
    while tokens.len() < 4 {
        while pos < data.len() && (data[pos].is_ascii_whitespace() || data[pos] == b'#') {
            if data[pos] == b'#' {
                while pos < data.len() && data[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if start == pos {
            return Err("not a complete PPM header".into());
        }
        tokens.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    pos += 1;
    if tokens[0] != "P6" {
        return Err("not a binary PPM (P6)".into());
    }
    let num = |s: &str| {
        s.parse::<usize>()
            .map_err(|_| format!("bad PPM number {s:?}"))
    };
    let (w, h, max) = (num(&tokens[1])?, num(&tokens[2])?, num(&tokens[3])?);
    if (w, h) != (WIDTH, HEIGHT) {
        return Err(format!(
            "the picture must be {WIDTH} × {HEIGHT}, not {w} × {h}"
        ));
    }
    if max != 255 {
        return Err(format!("the PPM's maximum must be 255, not {max}"));
    }
    let rgb = data
        .get(pos..pos + LEN * 3)
        .ok_or("the PPM ends before its last pixel")?;
    Ok(from_rgb(rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_survives_rgb_and_back() {
        let px: Vec<u8> = (0..=255).collect();
        assert_eq!(from_rgb(&to_rgb(&px)), px);
    }

    #[test]
    fn ppm_round_trip() {
        let px: Vec<u8> = (0..LEN).map(|i| (i * 7) as u8).collect();
        assert_eq!(from_ppm(&to_ppm(&px)).unwrap(), px);
        assert!(from_ppm(b"P6\n10 10\n255\n").is_err());
    }
}
