// SPDX-License-Identifier: MIT
//! Every field of a saved game as JSON, and back: `{"format": "technomage-save", "fields":
//! {path: value, …}}`, numbers as numbers, byte fields as hex. Export then import gives the
//! same file, byte for byte.

use crate::{schema, unhex, Save, Value};

pub const FORMAT: &str = "technomage-save";

/// All fields of `s`, in the schema's order (one a line, so a diff of two exports reads well).
pub fn export(s: &Save) -> String {
    let mut out = format!("{{\n  \"format\": \"{FORMAT}\",\n  \"fields\": {{\n");
    let fields = schema::fields_for(Some(s));
    for (i, f) in fields.iter().enumerate() {
        let v = match s.get(f) {
            Value::Int(n) => n.to_string(),
            Value::Bytes(b) => format!("\"{}\"", crate::hex(&b)),
        };
        let comma = if i + 1 < fields.len() { "," } else { "" };
        out.push_str(&format!("    \"{}\": {v}{comma}\n", f.path));
    }
    out.push_str("  }\n}\n");
    out
}

/// Fields from `text` over `base`. Without a base every field must be given; with one, those
/// given replace its values. A path the schema does not have is an error.
pub fn import(text: &str, base: Option<Save>) -> Result<Save, String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if doc.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
        return Err(format!(
            "not a {FORMAT} export (\"format\" missing or different)"
        ));
    }
    let given = doc
        .get("fields")
        .and_then(|f| f.as_object())
        .ok_or("no \"fields\" object")?;
    let complete = base.is_none();
    let mut s = base.unwrap_or_else(Save::zeroed);
    // The world state's layout follows the world: the other fields first, then those.
    let statics: Vec<_> = schema::fields_for(None)
        .into_iter()
        .filter(|f| !f.path.starts_with("world."))
        .collect();
    let mut used = 0;
    for f in &statics {
        if let Some(v) = given.get(&f.path) {
            s.set(f, &json_value(&f.path, v)?)?;
            used += 1;
        }
    }
    let world = schema::world_fields(s.bytes()[schema::LOCATION_WORLD]);
    for f in &world {
        if let Some(v) = given.get(&f.path) {
            s.set(f, &json_value(&f.path, v)?)?;
            used += 1;
        }
    }
    if used != given.len() {
        let unknown = given
            .keys()
            .find(|k| !statics.iter().chain(&world).any(|f| f.path == **k))
            .cloned()
            .unwrap_or_default();
        return Err(format!(
            "no field {unknown} (in the layout of world {})",
            s.bytes()[schema::LOCATION_WORLD]
        ));
    }
    if complete {
        if let Some(f) = statics
            .iter()
            .chain(&world)
            .find(|f| !given.contains_key(&f.path))
        {
            return Err(format!(
                "{} is missing (an import without a base file needs every field)",
                f.path
            ));
        }
    }
    Ok(s)
}

fn json_value(path: &str, v: &serde_json::Value) -> Result<Value, String> {
    match v {
        serde_json::Value::Number(n) => Ok(Value::Int(
            n.as_i64()
                .ok_or_else(|| format!("{path}: not an integer"))?,
        )),
        serde_json::Value::String(h) => {
            Ok(Value::Bytes(unhex(h).map_err(|e| format!("{path}: {e}"))?))
        }
        _ => Err(format!("{path}: a number or a hex string wanted")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_then_import_is_the_same_file() {
        for world in [1u8, 4, 8, 0, 200] {
            let mut b: Vec<u8> = (0..crate::SIZE).map(|i| (i * 31 % 251) as u8).collect();
            b[schema::LOCATION_WORLD] = world;
            let s = Save::from_bytes(b).unwrap();
            assert_eq!(import(&export(&s), None).unwrap(), s, "world {world}");
        }
    }

    #[test]
    fn a_partial_import_needs_a_base() {
        let text = format!("{{\"format\": \"{FORMAT}\", \"fields\": {{\"character.level\": 7}}}}");
        assert!(import(&text, None).is_err());
        let s = import(&text, Some(Save::zeroed())).unwrap();
        assert_eq!(s.bytes()[0x73ac], 7);
    }
}
