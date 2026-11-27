// SPDX-License-Identifier: MIT
//! Names the game's executable gives to what a saved game counts: its item table's names (the
//! scripts' identifiers, such as `HEAL_POTION`). They are read from the player's own
//! `TechnoMage.exe` (packed or not), never shipped with the editor.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// SHA-256 of the restored (UPX-unpacked) German 1.04 executable, the one every variant runs:
/// the addresses below are this build's.
const UNPACKED_SHA256: &str = "a1201ab89542093b3ca312840fc44c216d62cdb6146e3ca3b29925ceac68ba1a";

/// The item table: 12-byte entries {name pointer, class, count}; it ends at a null name.
const ITEM_TABLE: u32 = 0x0055_8f30;
pub const ITEM_SLOTS: usize = 256;

/// The level tables: a pointer per world (1..=8) to its stages, 0x30 bytes each: the level's
/// slot (i8 at +5, -1 = keeps no state) and its LEV name (pointer at +0x14, null = no level).
const LEVEL_TABLES: u32 = 0x0056_2134;
const STAGES: [usize; 9] = [0, 13, 23, 10, 29, 20, 20, 8, 11];

#[derive(Debug, Clone, Default)]
pub struct Names {
    /// Item names by index (the item table's order); fewer than [`ITEM_SLOTS`].
    pub items: Vec<String>,
    /// Per world (index 1..=8): its stages' (LEV name, slot), stage 1 first.
    pub stages: Vec<Vec<(Option<String>, i8)>>,
}

impl Names {
    /// The index of the item called `name` (case ignored); the first if two share it.
    pub fn item_index(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|n| n.eq_ignore_ascii_case(name))
    }

    pub fn item(&self, i: usize) -> Option<&str> {
        self.items.get(i).map(String::as_str)
    }

    /// The level of `stage` (1-based) of `world`.
    pub fn stage(&self, world: usize, stage: usize) -> Option<&str> {
        self.stages
            .get(world)?
            .get(stage.checked_sub(1)?)?
            .0
            .as_deref()
    }

    /// How many stages `world` has (0 for no world).
    pub fn stage_count(&self, world: usize) -> usize {
        self.stages.get(world).map_or(0, Vec::len)
    }

    /// The level slot `stage` of `world` keeps its state in (None: it keeps none).
    pub fn stage_slot(&self, world: usize, stage: usize) -> Option<usize> {
        let s = self.stages.get(world)?.get(stage.checked_sub(1)?)?.1;
        usize::try_from(s).ok()
    }

    /// The first level of `world` that keeps its state in `slot`.
    pub fn slot_name(&self, world: usize, slot: usize) -> Option<&str> {
        self.stages
            .get(world)?
            .iter()
            .find(|(n, s)| n.is_some() && usize::try_from(*s).ok() == Some(slot))
            .and_then(|(n, _)| n.as_deref())
    }
}

/// Where to look for the executable: `explicit`, else `$RC_GAME_DIR`, else the German variant
/// in the repository's `orig/`.
pub fn find_exe(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("RC_GAME_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("orig/technomage/cd-de"));
    dirs.into_iter().find_map(|d| {
        std::fs::read_dir(&d)
            .ok()?
            .filter_map(Result::ok)
            .find_map(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case("TechnoMage.exe"))
                    .then(|| e.path())
            })
    })
}

/// The names in the executable at `path`.
pub fn load(path: &Path) -> Result<Names, String> {
    let file = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let image = if rc_unpack::detect(&file).is_some() {
        rc_unpack::unpack(&file).map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        file
    };
    let hash: String = Sha256::digest(&image)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hash != UNPACKED_SHA256 {
        return Err(format!(
            "{}: not the executable the editor knows (German 1.04, which every language runs)",
            path.display()
        ));
    }
    let pe = rc_pe::parse(&image).map_err(|e| format!("{}: {e:?}", path.display()))?;
    let at = |va: u32| -> Option<usize> {
        let rva = va.checked_sub(pe.image_base)?;
        let s = pe.section_for_rva(rva)?;
        let off = rva - s.virtual_address;
        (off < s.raw_size).then(|| (s.raw_offset + off) as usize)
    };
    let u32_at = |va: u32| -> Option<u32> {
        let o = at(va)?;
        Some(u32::from_le_bytes(image.get(o..o + 4)?.try_into().ok()?))
    };
    let cstr = |va: u32| -> Option<String> {
        let o = at(va)?;
        let end = image[o..].iter().position(|&b| b == 0)?;
        Some(image[o..o + end].iter().map(|&b| char::from(b)).collect())
    };
    let mut items = Vec::new();
    for i in 0..ITEM_SLOTS as u32 {
        let name = u32_at(ITEM_TABLE + 12 * i).ok_or("the item table is out of the image")?;
        if name == 0 {
            break;
        }
        items.push(cstr(name).ok_or("an item name is out of the image")?);
    }
    let mut stages = vec![Vec::new()];
    for (w, &count) in STAGES.iter().enumerate().skip(1) {
        let table =
            u32_at(LEVEL_TABLES + 4 * w as u32).ok_or("the level tables are out of the image")?;
        let mut list = Vec::new();
        for i in 0..count as u32 {
            let e = table + 0x30 * i;
            let slot = at(e + 5)
                .and_then(|o| image.get(o))
                .copied()
                .unwrap_or(0xff) as i8;
            let name = match u32_at(e + 0x14) {
                Some(0) | None => None,
                Some(p) => cstr(p),
            };
            list.push((name, slot));
        }
        stages.push(list);
    }
    Ok(Names { items, stages })
}
