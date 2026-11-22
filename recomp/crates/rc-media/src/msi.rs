// SPDX-License-Identifier: MIT
//! Windows Installer packages: an `.msi` database (a [compound file](crate::cfb)) whose files are
//! in an external cabinet ([`crate::cab`]) beside it, as on the Russian Crazy School CD (made with
//! InstallShield Developer 7). Packages with cabinets stored inside the `.msi`, several media or
//! uncompressed source trees are refused with [`Error::Unsupported`].
//!
//! The database (Microsoft's published table format, our own implementation):
//!
//! - Stream names are packed: two name characters from `0-9A-Za-z._` per code unit in
//!   `0x3800..0x4800`, one in `0x4800..0x4840`; tables start with `0x4840`.
//! - `_StringPool` holds the code page (with bit 31 for 3-byte string references) and a
//!   (length, reference count) pair per string; `_StringData` the strings, back to back.
//! - `_Columns` (table, number, name, type) describes every table. A table's stream holds its
//!   rows column by column: strings as references, integers biased by `0x8000` / `0x80000000`
//!   (0 is NULL).
//! - `File` maps each cabinet member (by its key) to a component, a name (`short|long`) and a
//!   size; `Component` places it in a `Directory`, whose `DefaultDir` (`target:source`, each
//!   `short|long`, `.` for "the parent") builds the path. Paths are given relative to the deepest
//!   directory all files share: the install directory.

use crate::cab::Cabinet;
use crate::cfb::Compound;
use crate::inno::{RegistryEntry, RegistryValue};
use crate::{format_err, Archive, DirSink, Entry, Error, ReadSeek, Result, Sink};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Null,
    Int(i32),
    Str(String),
}

impl Value {
    fn str(&self) -> &str {
        match self {
            Value::Str(s) => s,
            _ => "",
        }
    }
    fn int(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

struct Column {
    name: String,
    string: bool,
    /// 2 or 4 for integers.
    width: usize,
}

struct Database {
    cfb: Compound,
    strings: Vec<String>,
    long_refs: bool,
    columns: BTreeMap<String, Vec<Column>>,
}

/// The packed stream name of `name` (a table's with the `0x4840` mark).
fn encode_name(name: &str, table: bool) -> Vec<u16> {
    fn code(c: u8) -> Option<u16> {
        Some(match c {
            b'0'..=b'9' => u16::from(c - b'0'),
            b'A'..=b'Z' => u16::from(c - b'A') + 10,
            b'a'..=b'z' => u16::from(c - b'a') + 36,
            b'.' => 62,
            b'_' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::new();
    if table {
        out.push(0x4840);
    }
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match (code(b[i]), b.get(i + 1).and_then(|&c| code(c))) {
            (Some(x), Some(y)) => {
                out.push(0x3800 + x + (y << 6));
                i += 2;
            }
            (Some(x), None) => {
                out.push(0x4800 + x);
                i += 1;
            }
            (None, _) => {
                out.push(u16::from(b[i]));
                i += 1;
            }
        }
    }
    out
}

/// Windows-1251 (Cyrillic) bytes 0x80..=0xFF.
fn cp1251(bytes: &[u8]) -> String {
    const HIGH: &str = "ЂЃ‚ѓ„…†‡€‰Љ‹ЊЌЋЏђ‘’“”•–—\u{fffd}™љ›њќћџ\u{a0}ЎўЈ¤Ґ¦§Ё©Є«¬\u{ad}®Ї°±Ііґµ¶·ё№є»јЅѕїАБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдежзийклмнопрстуфхцчшщъыьэюя";
    let high: Vec<char> = HIGH.chars().collect();
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                high[usize::from(b - 0x80)]
            }
        })
        .collect()
}

impl Database {
    fn parse(data: &[u8]) -> Result<Self> {
        let cfb = Compound::parse(data)?;
        let pool = cfb
            .stream(&encode_name("_StringPool", true))
            .ok_or_else(|| Error::Format("MSI without _StringPool".into()))?;
        let text = cfb
            .stream(&encode_name("_StringData", true))
            .ok_or_else(|| Error::Format("MSI without _StringData".into()))?;
        if pool.len() < 4 {
            return format_err("MSI string pool too short");
        }
        let cp = u32::from_le_bytes(pool[..4].try_into().expect("4"));
        let long_refs = cp & 0x8000_0000 != 0;
        let decode = |b: &[u8]| -> String {
            match cp & 0xffff {
                1251 => cp1251(b),
                _ => crate::codepage::cp1252(b),
            }
        };
        let mut strings = vec![String::new()]; // id 0 is NULL
        let mut at = 0usize;
        let mut i = 4;
        while i + 4 <= pool.len() {
            let len = usize::from(u16::from_le_bytes([pool[i], pool[i + 1]]));
            let refs = u16::from_le_bytes([pool[i + 2], pool[i + 3]]);
            i += 4;
            if len == 0 && refs != 0 {
                return Err(Error::Unsupported("MSI strings longer than 64 KiB".into()));
            }
            let s = text
                .get(at..at + len)
                .ok_or_else(|| Error::Format("MSI string data shorter than its pool".into()))?;
            strings.push(decode(s));
            at += len;
        }
        let mut db = Database {
            cfb,
            strings,
            long_refs,
            columns: BTreeMap::new(),
        };
        // _Columns: Table (string), Number (i2), Name (string), Type (i2).
        let fixed = vec![
            Column {
                name: "Table".into(),
                string: true,
                width: 0,
            },
            Column {
                name: "Number".into(),
                string: false,
                width: 2,
            },
            Column {
                name: "Name".into(),
                string: true,
                width: 0,
            },
            Column {
                name: "Type".into(),
                string: false,
                width: 2,
            },
        ];
        let rows = db.rows_with("_Columns", &fixed)?;
        let mut cols: BTreeMap<String, Vec<(i32, Column)>> = BTreeMap::new();
        for r in rows {
            let ty = r[3].int().unwrap_or(0) as u32;
            let string = ty & 0x0800 != 0;
            let width = if (ty & 0xff) == 4 { 4 } else { 2 };
            cols.entry(r[0].str().to_string()).or_default().push((
                r[1].int().unwrap_or(0),
                Column {
                    name: r[2].str().to_string(),
                    string,
                    width,
                },
            ));
        }
        for (t, mut c) in cols {
            c.sort_by_key(|(n, _)| *n);
            db.columns
                .insert(t, c.into_iter().map(|(_, c)| c).collect());
        }
        Ok(db)
    }

    /// The rows of `table` as maps from column name to value; none when the table is absent.
    fn table(&self, table: &str) -> Result<Vec<BTreeMap<String, Value>>> {
        let Some(cols) = self.columns.get(table) else {
            return Ok(Vec::new());
        };
        let rows = self.rows_with(table, cols)?;
        Ok(rows
            .into_iter()
            .map(|r| cols.iter().map(|c| c.name.clone()).zip(r).collect())
            .collect())
    }

    fn rows_with(&self, table: &str, cols: &[Column]) -> Result<Vec<Vec<Value>>> {
        let Some(data) = self.cfb.stream(&encode_name(table, true)) else {
            return Ok(Vec::new());
        };
        let sref = if self.long_refs { 3 } else { 2 };
        let row_size: usize = cols
            .iter()
            .map(|c| if c.string { sref } else { c.width })
            .sum();
        if row_size == 0 || data.len() % row_size != 0 {
            return format_err(format!(
                "MSI table {table}: {} bytes, rows of {row_size}",
                data.len()
            ));
        }
        let n = data.len() / row_size;
        let mut rows = vec![Vec::with_capacity(cols.len()); n];
        let mut at = 0;
        for c in cols {
            let w = if c.string { sref } else { c.width };
            for row in rows.iter_mut() {
                let b = &data[at..at + w];
                let raw = b.iter().rev().fold(0u32, |v, &x| v << 8 | u32::from(x));
                at += w;
                row.push(if c.string {
                    if raw == 0 {
                        Value::Null
                    } else {
                        Value::Str(
                            self.strings
                                .get(raw as usize)
                                .ok_or_else(|| Error::Format(format!("MSI string {raw} missing")))?
                                .clone(),
                        )
                    }
                } else if raw == 0 {
                    Value::Null
                } else if w == 2 {
                    Value::Int(i32::from((raw ^ 0x8000) as u16 as i16))
                } else {
                    Value::Int((raw ^ 0x8000_0000) as i32)
                });
            }
        }
        Ok(rows)
    }
}

/// The path of directory `key` from the root, as long names.
fn dir_path(dirs: &BTreeMap<String, (String, String)>, key: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut key = key;
    for _ in 0..64 {
        let (parent, name) = dirs
            .get(key)
            .ok_or_else(|| Error::Format(format!("MSI directory {key} missing")))?;
        if parent.is_empty() || parent == key {
            parts.reverse();
            return Ok(parts);
        }
        if name != "." && !name.is_empty() {
            parts.push(name.clone());
        }
        key = parent;
    }
    format_err("MSI directory tree loops")
}

/// The long name of a `short|long` pair.
fn long_name(s: &str) -> &str {
    s.split_once('|').map_or(s, |(_, l)| l)
}

pub struct Package {
    cab: Cabinet,
    entries: Vec<Entry>,
    /// The cabinet member of each entry.
    members: Vec<usize>,
    directories: Vec<String>,
    registry: Vec<RegistryEntry>,
}

impl Package {
    /// The package from its database bytes and its cabinet (`Media.Cabinet`, opened by the
    /// caller from beside the `.msi`).
    pub fn open(msi: &[u8], cab: Box<dyn ReadSeek>) -> Result<Self> {
        let db = Database::parse(msi)?;
        let media = db.table("Media")?;
        if media.len() != 1 {
            return Err(Error::Unsupported(format!(
                "MSI with {} media",
                media.len()
            )));
        }
        if media[0]
            .get("Cabinet")
            .map(Value::str)
            .unwrap_or("")
            .starts_with('#')
        {
            return Err(Error::Unsupported("MSI with its cabinet inside".into()));
        }
        // Directory paths from the root (TARGETDIR), by long target names.
        let dirs: BTreeMap<String, (String, String)> = db
            .table("Directory")?
            .into_iter()
            .map(|r| {
                let key = r["Directory"].str().to_string();
                let parent = r["Directory_Parent"].str().to_string();
                let target = r["DefaultDir"]
                    .str()
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_string();
                (key, (parent, long_name(&target).to_string()))
            })
            .collect();
        let path_of = |key: &str| dir_path(&dirs, key);
        let components: BTreeMap<String, String> = db
            .table("Component")?
            .into_iter()
            .map(|r| {
                (
                    r["Component"].str().to_string(),
                    r["Directory_"].str().to_string(),
                )
            })
            .collect();
        let mut files = Vec::new();
        for r in db.table("File")? {
            let comp = r["Component_"].str();
            let dir = components
                .get(comp)
                .ok_or_else(|| Error::Format(format!("MSI component {comp} missing")))?;
            let mut path = path_of(dir)?;
            path.push(long_name(r["FileName"].str()).to_string());
            let size = r["FileSize"].int().unwrap_or(0).max(0) as u64;
            files.push((r["File"].str().to_string(), path, size));
        }
        if files.is_empty() {
            return format_err("MSI without files");
        }
        // The install directory: the deepest directory every file is in.
        let mut common: Vec<String> = files[0].1[..files[0].1.len() - 1].to_vec();
        for (_, p, _) in &files {
            let dir = &p[..p.len() - 1];
            let n = common.iter().zip(dir).take_while(|(a, b)| a == b).count();
            common.truncate(n);
        }
        let cab = Cabinet::open(cab)?;
        let by_name: BTreeMap<&str, usize> = cab
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.as_str(), i))
            .collect();
        let (mut entries, mut members, mut directories) = (Vec::new(), Vec::new(), BTreeSet::new());
        for (key, path, size) in &files {
            let &m = by_name
                .get(key.as_str())
                .ok_or_else(|| Error::Format(format!("{key}: not in the cabinet")))?;
            if u64::from(cab.files[m].size) != *size {
                return Err(Error::Checksum(format!(
                    "{key}: cabinet and database sizes differ"
                )));
            }
            let rel = path[common.len()..].join("/");
            crate::safe_relative(&rel)?;
            for k in 1..path.len() - common.len() {
                directories.insert(path[common.len()..common.len() + k].join("/"));
            }
            entries.push(Entry {
                path: rel,
                size: *size,
            });
            members.push(m);
        }
        // Registry values that name the install directory get Inno's {app}.
        let install_keys: Vec<String> = dirs
            .keys()
            .filter(|k| path_of(k).map(|p| p == common).unwrap_or(false))
            .map(|k| format!("[{k}]"))
            .collect();
        let mut registry = Vec::new();
        for r in db.table("Registry")? {
            let root = match r["Root"].int() {
                Some(0) => "HKCR",
                Some(1) => "HKCU",
                Some(2) | Some(-1) => "HKLM",
                Some(3) => "HKU",
                _ => continue,
            };
            let mut value = r.get("Value").map(Value::str).unwrap_or("").to_string();
            for k in &install_keys {
                value = value.replace(k.as_str(), "{app}");
            }
            let name = r.get("Name").map(Value::str).unwrap_or("").to_string();
            if name.starts_with(['+', '-', '*']) || value.starts_with('#') {
                continue; // key actions and typed values the game does not need
            }
            registry.push(RegistryEntry {
                root,
                key: r["Key"].str().to_string(),
                name,
                kind: RegistryValue::String,
                value: value.trim_end_matches('\\').to_string(),
                has_check: false,
                deletes: false,
            });
        }
        Ok(Package {
            cab,
            entries,
            members,
            directories: directories.into_iter().collect(),
            registry,
        })
    }
}

impl Archive for Package {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn directories(&self) -> &[String] {
        &self.directories
    }

    fn registry(&self) -> &[RegistryEntry] {
        &self.registry
    }

    fn extract(&mut self, dest: &Path, select: &dyn Fn(&Entry) -> bool) -> Result<usize> {
        self.extract_to(&mut DirSink::new(dest), select)
    }

    fn extract_to(
        &mut self,
        sink: &mut dyn Sink,
        select: &dyn Fn(&Entry) -> bool,
    ) -> Result<usize> {
        for d in &self.directories {
            sink.dir(d)?;
        }
        // Several entries may name one member; the cabinet is read in its own order.
        let mut wanted: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            if select(e) {
                wanted.entry(self.members[i]).or_default().push(i);
            }
        }
        let members: Vec<usize> = wanted.keys().copied().collect();
        let entries = &self.entries;
        let mut n = 0;
        let mut open: Option<usize> = None;
        self.cab.extract(&members, &mut |m, piece, last| {
            let list = &wanted[&m];
            // A member named by several entries is written for the first; copies are rare and
            // not needed by the supported package.
            if list.len() > 1 {
                return Err(Error::Unsupported(
                    "an MSI cabinet member installed twice".into(),
                ));
            }
            let e = &entries[list[0]];
            if open != Some(m) {
                sink.begin(&e.path, e.size)?;
                open = Some(m);
            }
            sink.write(piece)?;
            if last {
                sink.end()?;
                open = None;
                n += 1;
            }
            Ok(())
        })?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_names() {
        // "File" as a table: the mark, "Fi", "le".
        let f = encode_name("File", true);
        assert_eq!(f[0], 0x4840);
        assert_eq!(f[1], 0x3800 + 15 + (44 << 6));
        assert_eq!(f[2], 0x3800 + 47 + (40 << 6));
        assert_eq!(encode_name("_Tables", true).len(), 5);
        assert_eq!(encode_name("a", false), [0x4800 + 36]);
    }

    #[test]
    fn cyrillic() {
        assert_eq!(cp1251(b"\xce\xf2\xee\xf0\xe2\xe0"), "Оторва");
        assert_eq!(cp1251(b"abc"), "abc");
    }
}
