// SPDX-License-Identifier: MIT
//! TechnoMage's saved games (`Saves\TMN_NN.TSG`): every byte described, read, checked,
//! changed and written back.
//!
//! A saved game is the game's persistent state block as it is in memory (the game keeps it at
//! one pointer, packs the live state into it before saving and reads the file back whole):
//! 30,232 bytes without a checksum. [`schema`] describes all of them, each byte by exactly one
//! field (a test holds it to that); the editor reads and writes fields, never bytes the schema
//! does not name.

pub mod check;
pub mod json;
pub mod names;
pub mod picture;
pub mod schema;
pub mod worlds;

pub use names::Names;
pub use schema::{Field, Ty};

/// The size of every saved game.
pub const SIZE: usize = 0x7618;

/// A field's value: a number, or the bytes of a byte field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Bytes(b) => f.write_str(&hex(b)),
        }
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hex digits (spaces allowed) to bytes.
pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return Err("an odd number of hex digits".into());
    }
    digits
        .chunks(2)
        .map(|p| {
            u8::from_str_radix(std::str::from_utf8(p).unwrap_or("?"), 16)
                .map_err(|_| format!("not hex: {:?}", String::from_utf8_lossy(p)))
        })
        .collect()
}

/// One saved game, always [`SIZE`] bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    bytes: Vec<u8>,
}

impl Save {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Save, String> {
        if bytes.len() != SIZE {
            return Err(format!(
                "a saved game has {SIZE} bytes, this one {}",
                bytes.len()
            ));
        }
        Ok(Save { bytes })
    }

    /// All zeros: the base an import without one starts from.
    pub fn zeroed() -> Save {
        Save {
            bytes: vec![0; SIZE],
        }
    }

    pub fn read(path: &std::path::Path) -> Result<Save, String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Save::from_bytes(b).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn write(&self, path: &std::path::Path) -> Result<(), String> {
        std::fs::write(path, &self.bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn get(&self, f: &Field) -> Value {
        let b = &self.bytes[f.offset..f.offset + f.ty.size()];
        match f.ty {
            Ty::U8 => Value::Int(i64::from(b[0])),
            Ty::I8 => Value::Int(i64::from(b[0] as i8)),
            Ty::U16 => Value::Int(i64::from(u16::from_le_bytes([b[0], b[1]]))),
            Ty::I16 => Value::Int(i64::from(i16::from_le_bytes([b[0], b[1]]))),
            Ty::U32 => Value::Int(i64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            Ty::I32 => Value::Int(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            Ty::Bytes(_) => Value::Bytes(b.to_vec()),
            Ty::Bit(n) => Value::Int(i64::from(b[0] >> n & 1)),
        }
    }

    /// Set `f` to `v`; a number must fit the field's type, bytes its length.
    pub fn set(&mut self, f: &Field, v: &Value) -> Result<(), String> {
        let out = &mut self.bytes[f.offset..f.offset + f.ty.size()];
        let range = |lo: i64, hi: i64, n: i64| -> Result<i64, String> {
            if (lo..=hi).contains(&n) {
                Ok(n)
            } else {
                Err(format!("{}: {n} is outside {lo}..={hi}", f.path))
            }
        };
        match (f.ty, v) {
            (Ty::Bytes(n), Value::Bytes(b)) => {
                if b.len() != n {
                    return Err(format!("{}: {n} bytes wanted, {} given", f.path, b.len()));
                }
                out.copy_from_slice(b);
            }
            (Ty::Bytes(_), Value::Int(_)) => {
                return Err(format!("{}: bytes wanted (hex)", f.path));
            }
            (_, Value::Bytes(_)) => return Err(format!("{}: a number wanted", f.path)),
            (Ty::Bit(b), Value::Int(n)) => {
                let bit = range(0, 1, *n)? as u8;
                out[0] = out[0] & !(1 << b) | bit << b;
            }
            (Ty::U8, Value::Int(n)) => out[0] = range(0, 0xff, *n)? as u8,
            (Ty::I8, Value::Int(n)) => out[0] = range(-0x80, 0x7f, *n)? as i8 as u8,
            (Ty::U16, Value::Int(n)) => {
                out.copy_from_slice(&(range(0, 0xffff, *n)? as u16).to_le_bytes());
            }
            (Ty::I16, Value::Int(n)) => {
                out.copy_from_slice(&(range(-0x8000, 0x7fff, *n)? as i16).to_le_bytes());
            }
            (Ty::U32, Value::Int(n)) => {
                out.copy_from_slice(&(range(0, 0xffff_ffff, *n)? as u32).to_le_bytes());
            }
            (Ty::I32, Value::Int(n)) => {
                out.copy_from_slice(&(range(-0x8000_0000, 0x7fff_ffff, *n)? as i32).to_le_bytes())
            }
        }
        Ok(())
    }

    /// Parse `text` as a value for `f`: a number (decimal, `0x` hex, negative), a name the
    /// field knows (an item, a flag set as `NAME|NAME`, `true`/`false`), or hex for bytes.
    pub fn parse(&self, f: &Field, text: &str, names: Option<&Names>) -> Result<Value, String> {
        if let Ty::Bytes(n) = f.ty {
            if f.label == schema::Label::Text {
                // Text, zero-padded; hex only when given as 0x…
                if let Some(h) = text.strip_prefix("0x") {
                    return unhex(h).map(Value::Bytes);
                }
                if text.len() > n || !text.is_ascii() {
                    return Err(format!("{}: at most {n} ASCII characters", f.path));
                }
                let mut b = text.as_bytes().to_vec();
                b.resize(n, 0);
                return Ok(Value::Bytes(b));
            }
            return unhex(text).map(Value::Bytes);
        }
        if let Some(n) = parse_int(text) {
            return Ok(Value::Int(n));
        }
        f.label
            .parse(text, names)
            .map(Value::Int)
            .ok_or_else(|| format!("{}: {text:?} is not a number or a name it knows", f.path))
    }
}

pub fn parse_int(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let v = if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else {
        body.parse::<i64>().ok()?
    };
    Some(if neg { -v } else { v })
}

/// The fields whose values differ between `a` and `b`.
pub fn diff<'a>(fields: &'a [Field], a: &Save, b: &Save) -> Vec<(&'a Field, Value, Value)> {
    fields
        .iter()
        .filter_map(|f| {
            let (x, y) = (a.get(f), b.get(f));
            (x != y).then_some((f, x, y))
        })
        .collect()
}
