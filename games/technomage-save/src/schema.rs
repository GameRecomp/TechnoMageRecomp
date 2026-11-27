// SPDX-License-Identifier: MIT
//! Every byte of a saved game, as fields: offset, type, path and meaning. Worked out from the
//! game's code (docs/RESEARCH.md, "Saved games"): the pack function that copies the live state
//! into the block, the code that restores it, and the code that uses each part.
//!
//! [`fields`] covers the 30,232 bytes exactly once for a given saved game (the world state's
//! layout depends on the world it is in); [`views`] adds single bits of the levels' bitfields
//! by their meaning (doors, creatures, …), which overlap those bytes and are not exported.

use crate::{names::Names, worlds, Save, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    Bytes(usize),
    /// One bit (0 = lowest) of the byte at the field's offset.
    Bit(u8),
}

impl Ty {
    pub fn size(self) -> usize {
        match self {
            Ty::U8 | Ty::I8 | Ty::Bit(_) => 1,
            Ty::U16 | Ty::I16 => 2,
            Ty::U32 | Ty::I32 => 4,
            Ty::Bytes(n) => n,
        }
    }

    pub fn name(self) -> String {
        match self {
            Ty::U8 => "u8".into(),
            Ty::I8 => "i8".into(),
            Ty::U16 => "u16".into(),
            Ty::I16 => "i16".into(),
            Ty::U32 => "u32".into(),
            Ty::I32 => "i32".into(),
            Ty::Bytes(n) => format!("[{n}]"),
            Ty::Bit(b) => format!("bit{b}"),
        }
    }
}

/// How a field's number reads (and which names `set` takes for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    None,
    /// An item table index (−1 or any negative = none).
    Item,
    /// The 1-based position in an equipment ring whose first item is this item index (0 =
    /// none), with this many entries.
    Ring(usize, usize),
    /// Named bits, lowest first.
    Flags(&'static [&'static str]),
    /// Named values.
    Enum(&'static [(i64, &'static str)]),
    /// A world number (1..=8).
    World,
    /// A stage of the saved game's world (1-based).
    Stage,
    /// A level slot of the saved game's world.
    Slot,
    /// A 12-bit angle (4096 = 360°).
    Angle,
    /// Play time, 1/3840 s.
    PlayTime,
    /// Zero-padded text (a byte field).
    Text,
    /// A torch timer: bit 31 lit, the rest 1/480 s.
    Torch,
}

const CRYSTALS: &[&str] = &["GREEN", "YELLOW", "BLUE", "RED", "WHITE"];
const VIDEOS: &[(i64, &str)] = &[
    (0, "LOGO"),
    (1, "INTRO"),
    (2, "CUT1"),
    (3, "CUT2"),
    (4, "VISION1"),
    (5, "CUT3"),
    (6, "VISION2"),
    (7, "CUT4"),
    (8, "VISION3"),
    (9, "CUT5"),
    (10, "VISION4"),
    (11, "CUT6"),
    (12, "VISION5"),
    (13, "CUT7"),
    (14, "OUTRO"),
    (15, "END"),
];
const DONE: &[&str] = &["done"];
const CHEATS: &[&str] = &[
    "b0",
    "b1",
    "b2",
    "b3",
    "b4",
    "b5",
    "b6",
    "b7",
    "b8",
    "b9",
    "b10",
    "b11",
    "b12",
    "b13",
    "b14",
    "b15",
    "b16",
    "b17",
    "b18",
    "b19",
    "b20",
    "b21",
    "b22",
    "b23",
    "b24",
    "b25",
    "b26",
    "cooper",
    "cult",
    "bughouse",
    "apocalyptica",
    "b31",
];

/// World names by number (the level tables' own names).
pub const WORLDS: [&str; 9] = [
    "", "DREAMER", "STEAMER", "THEHIVE", "FOREST", "CANYON", "TOWER", "VULCANO", "RUINS",
];

impl Label {
    /// The name of `n`, if the label gives it one.
    pub fn show(&self, n: i64, names: Option<&Names>, s: Option<&Save>) -> Option<String> {
        let world = s.map(|s| s.bytes()[LOCATION_WORLD] as i8);
        match *self {
            Label::None => None,
            Label::Item => {
                if n < 0 {
                    Some("empty".into())
                } else {
                    names
                        .and_then(|nm| nm.item(n as usize))
                        .map(str::to_owned)
                        .or_else(|| Some(format!("item {n}")))
                }
            }
            Label::Ring(first, len) => match n {
                0 => Some("none".into()),
                1.. if (n as usize) <= len => Some(
                    names
                        .and_then(|nm| nm.item(first + n as usize - 1))
                        .map_or_else(|| format!("item {}", first + n as usize - 1), str::to_owned),
                ),
                _ => Some("invalid".into()),
            },
            Label::Flags(fl) => {
                let set: Vec<String> = (0..64)
                    .filter(|b| n >> b & 1 == 1)
                    .map(|b| {
                        fl.get(b)
                            .map_or_else(|| format!("bit{b}"), |s| (*s).to_owned())
                    })
                    .collect();
                Some(if set.is_empty() {
                    "none".into()
                } else {
                    set.join("|")
                })
            }
            Label::Enum(e) => e
                .iter()
                .find(|(v, _)| *v == n)
                .map(|(_, s)| (*s).to_owned()),
            Label::World => WORLDS
                .get(n as usize)
                .filter(|w| n > 0 && !w.is_empty())
                .map(|w| (*w).to_owned()),
            Label::Stage => {
                let w = world? as usize;
                names?.stage(w, n as usize).map(str::to_owned)
            }
            Label::Slot => {
                let w = world? as usize;
                names?.slot_name(w, n as usize).map(str::to_owned)
            }
            Label::Angle => Some(format!("{:.1}°", (n & 0xfff) as f64 * 360.0 / 4096.0)),
            Label::PlayTime => {
                let secs = n as f64 / 3840.0;
                Some(format!(
                    "{}:{:02}:{:04.1}",
                    (secs / 3600.0) as u64,
                    (secs / 60.0) as u64 % 60,
                    secs % 60.0
                ))
            }
            Label::Text => None,
            Label::Torch => Some(format!(
                "{}, {:.0} s left",
                if n >> 31 & 1 == 1 { "lit" } else { "out" },
                (n & 0x7fff_ffff) as f64 / 480.0
            )),
        }
    }

    /// The number a name stands for in this field.
    pub fn parse(&self, text: &str, names: Option<&Names>) -> Option<i64> {
        match *self {
            Label::Item => {
                if text.eq_ignore_ascii_case("empty") || text.eq_ignore_ascii_case("none") {
                    return Some(-1);
                }
                names?.item_index(text).map(|i| i as i64)
            }
            Label::Ring(first, len) => {
                if text.eq_ignore_ascii_case("none") {
                    return Some(0);
                }
                let i = names?.item_index(text)?;
                (first..first + len)
                    .contains(&i)
                    .then(|| (i - first + 1) as i64)
            }
            Label::Flags(fl) => {
                if text.eq_ignore_ascii_case("none") {
                    return Some(0);
                }
                let mut v = 0i64;
                for part in text.split('|') {
                    let part = part.trim();
                    let b = fl
                        .iter()
                        .position(|f| f.eq_ignore_ascii_case(part))
                        .or_else(|| part.strip_prefix("bit").and_then(|b| b.parse().ok()))?;
                    v |= 1 << b;
                }
                Some(v)
            }
            Label::Enum(e) => e
                .iter()
                .find(|(_, s)| s.eq_ignore_ascii_case(text))
                .map(|(v, _)| *v),
            Label::World => WORLDS
                .iter()
                .position(|w| !w.is_empty() && w.eq_ignore_ascii_case(text))
                .map(|w| w as i64),
            _ => match text {
                "true" => Some(1),
                "false" => Some(0),
                _ => None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub path: String,
    pub offset: usize,
    pub ty: Ty,
    pub label: Label,
    pub doc: String,
    /// A view onto bits of another field: not exported, not part of the coverage.
    pub view: bool,
}

// Offsets the checks and labels use.
pub const ITEMS: usize = 0x4b14;
pub const BAG: usize = 0x4f14;
pub const BAG_SLOTS: usize = 27;
pub const SLOT_MASK: usize = 0x4f70;
pub const SLOTS: usize = 0x4f74;
pub const SLOT_SIZE: usize = 0x18c;
pub const SLOT_COUNT: usize = 22;
pub const ENEMIES: usize = 0x717c;
pub const NPCS: usize = 0x71fc;
pub const CHARACTER: usize = 0x738c;
pub const EQUIP: usize = 0x73b4;
pub const LOCATION_WORLD: usize = 0x73ba;
pub const WORLD_STATE: usize = 0x73c4;
pub const QUESTS: usize = 0x74c4;
pub const QUEST_COUNT: usize = 0x7604;
/// The names of the level bitfield's groups, in their order (their sizes are the counts).
pub const GROUPS: [(&str, &str); 9] = [
    ("trigger", "trigger box already fired"),
    ("door", "door open"),
    ("creature", "creature alive"),
    (
        "pickup",
        "placed item: bit 2i still there, bit 2i+1 its hidden flag (unsure)",
    ),
    (
        "lever",
        "lever or switch in its start position (polarity unsure)",
    ),
    (
        "magtorch",
        "magic torch: 1 = state 0/1 (probably unlit; unsure)",
    ),
    ("star", "BLINKINGSTAR still there"),
    ("trap", "trap still there"),
    ("web", "spider web intact"),
];

struct B(Vec<Field>);

impl B {
    fn f(&mut self, offset: usize, ty: Ty, path: impl Into<String>, label: Label, doc: &str) {
        self.0.push(Field {
            path: path.into(),
            offset,
            ty,
            label,
            doc: doc.to_owned(),
            view: false,
        });
    }
}

/// The fields of `s`: the static ones and its world's state layout (an unknown world's state
/// as raw bytes). Without a saved game, the layout of world 1.
pub fn fields_for(s: Option<&Save>) -> Vec<Field> {
    let world = s.map_or(1, |s| s.bytes()[LOCATION_WORLD]);
    let mut v = static_fields();
    v.extend(world_fields(world));
    v.sort_by_key(|f| f.offset);
    v
}

/// The fields every saved game has, with the world state of world 1 (for listings).
pub fn fields() -> Vec<Field> {
    fields_for(None)
}

/// The world state's fields for `world` (raw bytes for a world without a layout).
pub fn world_fields(world: u8) -> Vec<Field> {
    let layout = worlds::layout(world);
    if layout.is_empty() {
        return vec![Field {
            path: "world.raw".into(),
            offset: WORLD_STATE,
            ty: Ty::Bytes(0x100),
            label: Label::None,
            doc: "the world's script state (no layout: the world number is not 1..8)".into(),
            view: false,
        }];
    }
    layout
        .iter()
        .map(|&(off, ty, path, doc)| Field {
            path: path.to_owned(),
            offset: WORLD_STATE + off,
            ty,
            label: Label::None,
            doc: format!("{}: {doc}", WORLDS[world as usize]),
            view: false,
        })
        .collect()
}

fn static_fields() -> Vec<Field> {
    let mut b = B(Vec::new());
    use Label as L;
    use Ty::*;
    // The header: what the load/save book shows; copies of fields further on.
    b.f(0x00, U8, "header.picture_width", L::None, "picture width: the screen width / 4 when saved (the book shows the picture only if it is the current screen's / 4: keep 160)");
    b.f(
        0x01,
        U8,
        "header.picture_height",
        L::None,
        "picture height: the screen height / 4 (keep 120; the picture is always 160 x 120)",
    );
    b.f(
        0x02,
        I8,
        "header.world",
        L::World,
        "world shown in the book (copy of location.world; 9 makes the slot look empty)",
    );
    b.f(
        0x03,
        I8,
        "header.stage",
        L::Stage,
        "stage shown in the book (copy of location.stage)",
    );
    b.f(
        0x04,
        I16,
        "header.hp",
        L::None,
        "hit points shown in the book (copy of character.hp)",
    );
    b.f(
        0x06,
        I16,
        "header.hp_max",
        L::None,
        "maximum hit points shown (copy of character.hp_max)",
    );
    b.f(
        0x08,
        I16,
        "header.mana",
        L::None,
        "mana shown (copy of character.mana)",
    );
    b.f(
        0x0a,
        I16,
        "header.mana_max",
        L::None,
        "maximum mana shown (copy of character.mana_max)",
    );
    b.f(
        0x0c,
        U8,
        "header.crystals",
        L::Flags(CRYSTALS),
        "crystal icons shown: bit i set when item CRYSTAL_* i (52 + i) is owned",
    );
    b.f(
        0x0d,
        Bytes(3),
        "header.pad",
        L::None,
        "padding, never written (0)",
    );
    b.f(
        0x10,
        U32,
        "header.play_time",
        L::PlayTime,
        "play time shown (copy of play_time)",
    );
    b.f(
        0x14,
        Bytes(crate::picture::LEN),
        "picture",
        L::None,
        "the picture, 160 x 120, RGB 3-3-2 (tmsave picture)",
    );
    // Items: a count per item table entry.
    for i in 0..256 {
        b.f(ITEMS + 4 * i, U32, format!("items[{i}]"), L::None, "count (0..99; GOLDCOIN: gold, no upper limit; weapons, spells, defence, tools: owned when not 0; 255 never changes; entries past the table stay 0)");
    }
    for i in 0..BAG_SLOTS {
        b.f(
            BAG + 2 * i,
            I16,
            format!("bag[{i}]"),
            L::Item,
            "inventory bag slot (column i % 3, row i / 3): an item, -1 = empty",
        );
    }
    b.f(
        0x4f4a,
        I16,
        "player.heading",
        L::Angle,
        "the player's heading (12-bit angle)",
    );
    b.f(
        0x4f4c,
        I16,
        "player.camera",
        L::Angle,
        "the camera's yaw (12-bit angle; keep 0..4095)",
    );
    b.f(0x4f4e, Bytes(2), "player.pad", L::None, "padding (0)");
    b.f(
        0x4f50,
        I32,
        "player.x",
        L::None,
        "the player's position: x (2048 units a map tile)",
    );
    b.f(
        0x4f54,
        I32,
        "player.y",
        L::None,
        "the player's position: y (height)",
    );
    b.f(0x4f58, I32, "player.z", L::None, "the player's position: z");
    b.f(
        0x4f5c,
        I32,
        "player.spawn_x",
        L::None,
        "the activated SPAWN point: x (-1 = none)",
    );
    b.f(
        0x4f60,
        I32,
        "player.spawn_y",
        L::None,
        "the activated SPAWN point: y (-1 = none)",
    );
    b.f(
        0x4f64,
        I32,
        "player.spawn_z",
        L::None,
        "the activated SPAWN point: z (-1 = none)",
    );
    b.f(0x4f68, U32, "player.torch", L::Torch, "torch: bit 31 lit, bits 0..30 burn time left in 1/480 s (432000 a TORCH_OFF; at most 42768000)");
    b.f(
        0x4f6c,
        I32,
        "player.shield",
        L::None,
        "magic shield (SP_OSHIELD) strength, 0 = none (100 + 10 INT when cast; keep <= 32767)",
    );
    b.f(
        SLOT_MASK,
        U32,
        "levels.mask",
        L::Flags(&[]),
        "bit s: level slot s of the current world holds saved state (bits 22..31 unused)",
    );
    for s in 0..SLOT_COUNT {
        let r = SLOTS + s * SLOT_SIZE;
        let p = format!("slots[{s}]");
        for i in 0..16 {
            let q = r + 8 * i;
            b.f(q, U32, format!("{p}.platform[{i}].pos"), L::None, "moving platform / elevator / raft: packed position (10 bits each of x, y (signed), z, in 512 units)");
            for k in 0..4 {
                b.f(
                    q + 4 + k,
                    U8,
                    format!("{p}.platform[{i}].state[{k}]"),
                    L::None,
                    "moving platform state byte (meaning unsure)",
                );
            }
        }
        b.f(
            r + 0x80,
            U32,
            format!("{p}.map_lo"),
            L::Flags(&[]),
            "automap sections revealed, bits 0..31",
        );
        b.f(
            r + 0x84,
            U32,
            format!("{p}.map_hi"),
            L::Flags(&[]),
            "automap sections revealed, bits 32..63",
        );
        b.f(
            r + 0x88,
            U32,
            format!("{p}.markers"),
            L::Flags(&[]),
            "automap markers shown",
        );
        for (g, (name, _)) in GROUPS.iter().enumerate() {
            b.f(r + 0x8c + g, U8, format!("{p}.count.{name}"), L::None, "size of this group in the bitfield: a constant of the level (keep it; all groups <= 256 bits)");
        }
        b.f(r + 0x95, Bytes(32), format!("{p}.bits"), L::None, "the level's object bitfield (groups in order: triggers, doors, creatures, pickups x2, levers, magic torches, stars, traps, webs; see the views slots[s].<group>[i])");
        for i in 0..64 {
            b.f(
                r + 0xb5 + 2 * i,
                U8,
                format!("{p}.crate[{i}].a"),
                L::None,
                "crate / chest: drag crate: tile x; container: 1 = still there, 0 = gone",
            );
            b.f(
                r + 0xb6 + 2 * i,
                U8,
                format!("{p}.crate[{i}].b"),
                L::None,
                "crate / chest: drag crate: tile z",
            );
        }
        b.f(
            r + 0x135,
            Bytes(3),
            format!("{p}.pad"),
            L::None,
            "padding (0)",
        );
        b.f(r + 0x138, I32, format!("{p}.return_pos"), L::None, "player return position (x >> 8 in bits 0..10, z >> 8 in 11..21, angle >> 6 in 22..31; -1 = none)");
        for i in 0..20 {
            b.f(r + 0x13c + 4 * i, I32, format!("{p}.chalk[{i}]"), L::None, "chalk mark (x >> 6 in bits 0..11, z >> 6 in 12..23, angle >> 4 in 24..31; -1 = empty)");
        }
    }
    for i in 0..16 {
        let r = ENEMIES + 8 * i;
        b.f(
            r,
            U32,
            format!("enemies[{i}].pos"),
            L::None,
            "a creature of the current level: packed position (as return_pos)",
        );
        b.f(
            r + 4,
            U16,
            format!("enemies[{i}].hp"),
            L::None,
            "its hit points (0 = free record)",
        );
        b.f(
            r + 6,
            I16,
            format!("enemies[{i}].creature"),
            L::None,
            "its index in the creature group (-2 = free)",
        );
    }
    for i in 0..40 {
        let r = NPCS + 10 * i;
        b.f(
            r,
            U16,
            format!("npcs[{i}].x"),
            L::None,
            "a person of the current level: x >> 8",
        );
        b.f(r + 2, U16, format!("npcs[{i}].z"), L::None, "z >> 8");
        b.f(
            r + 4,
            U16,
            format!("npcs[{i}].heading"),
            L::Angle,
            "heading",
        );
        b.f(
            r + 6,
            I16,
            format!("npcs[{i}].node"),
            L::None,
            "path node index (-1 = none)",
        );
        b.f(
            r + 8,
            U8,
            format!("npcs[{i}].a"),
            L::None,
            "state byte (unsure)",
        );
        b.f(
            r + 9,
            U8,
            format!("npcs[{i}].b"),
            L::None,
            "state byte (unsure)",
        );
    }
    b.f(
        0x738c,
        U32,
        "character.xp",
        L::None,
        "experience (level L needs XP >= table[L]: 0, 0, 10000, 22000, ..., 1000000 for 20)",
    );
    b.f(
        0x7390,
        U32,
        "character.xp_pending",
        L::None,
        "experience from kills, added when xp_timer runs out",
    );
    b.f(
        0x7394,
        U32,
        "character.xp_reward",
        L::None,
        "experience from quest rewards, shown with particles, then moved to xp_pending",
    );
    b.f(
        0x7398,
        I16,
        "character.xp_timer",
        L::None,
        "experience flush timer, 1/480 s",
    );
    b.f(0x739a, I16, "character.hp", L::None, "hit points");
    b.f(
        0x739c,
        I16,
        "character.hp_max",
        L::None,
        "maximum hit points = 5 CON + 6 level + 4 (recomputed on level-up; keep >= 2)",
    );
    b.f(
        0x739e,
        I16,
        "character.poison",
        L::None,
        "poison: hit points still to drain",
    );
    b.f(0x73a0, I16, "character.mana", L::None, "mana");
    b.f(
        0x73a2,
        I16,
        "character.mana_max",
        L::None,
        "maximum mana = 10 INT + 5",
    );
    b.f(
        0x73a4,
        I16,
        "character.poison_timer",
        L::None,
        "poison tick timer, 1/480 s",
    );
    b.f(
        0x73a6,
        I16,
        "character.poison_effect",
        L::None,
        "poison screen effect strength, 0..4096",
    );
    b.f(
        0x73a8,
        I16,
        "character.hp_regen_timer",
        L::None,
        "hit point regeneration timer, 1/480 s (only below half)",
    );
    b.f(
        0x73aa,
        I16,
        "character.mana_regen_timer",
        L::None,
        "mana regeneration timer, 1/480 s",
    );
    b.f(0x73ac, I8, "character.level", L::None, "level, 1..20");
    b.f(0x73ad, I8, "character.str", L::None, "strength, 0..40");
    b.f(
        0x73ae,
        I8,
        "character.int",
        L::None,
        "intelligence, 0..40 (maximum mana, shield)",
    );
    b.f(
        0x73af,
        I8,
        "character.con",
        L::None,
        "constitution, 0..40 (maximum hit points)",
    );
    b.f(
        0x73b0,
        I8,
        "character.mys",
        L::None,
        "mysticism, 0..40 (mana regeneration from 7)",
    );
    b.f(0x73b1, Bytes(3), "character.pad", L::None, "padding (0)");
    b.f(
        EQUIP,
        U8,
        "equip.weapon",
        L::Ring(0, 8),
        "selected weapon: 1..8 = WP_DAGGER..WP_BOW_WATER, 0 = none (must be owned)",
    );
    b.f(
        EQUIP + 1,
        U8,
        "equip.spell",
        L::Ring(8, 7),
        "selected spell: 1..7 = SP_FIREBALL..SP_EARTHQUAKE, 0 = none (must be owned)",
    );
    b.f(
        EQUIP + 2,
        U8,
        "equip.tool",
        L::Ring(23, 4),
        "selected tool: 1..4 = TL_BOMB..TL_CLAW, 0 = none (must be owned)",
    );
    b.f(EQUIP + 3, U8, "equip.defense", L::Ring(15, 8), "selected defence item: 1..8 = DEF_AMULET_ANKH..DEF_GLOVES_DEXTERITY, 0 = none (must be owned)");
    b.f(
        0x73b8,
        I8,
        "bag_cursor.x",
        L::None,
        "inventory cursor column, 0..2",
    );
    b.f(
        0x73b9,
        I8,
        "bag_cursor.y",
        L::None,
        "inventory cursor row, 0..8",
    );
    b.f(
        LOCATION_WORLD,
        I8,
        "location.world",
        L::World,
        "world, 1..8 (selects the world state's layout)",
    );
    b.f(
        0x73bb,
        I8,
        "location.stage",
        L::Stage,
        "stage of the world, 1-based (a level of the world's table)",
    );
    b.f(
        0x73bc,
        I8,
        "location.entry",
        L::None,
        "entry point the level was entered by, 0..23",
    );
    b.f(
        0x73bd,
        I8,
        "location.map_variant",
        L::None,
        "alternative map flag (0 or 1)",
    );
    b.f(
        0x73be,
        I8,
        "videos_seen",
        L::Enum(VIDEOS),
        "the furthest story video seen (the video menu offers those up to it)",
    );
    b.f(0x73bf, U8, "pad", L::None, "padding (0)");
    b.f(
        0x73c0,
        U32,
        "play_time",
        L::PlayTime,
        "play time in 1/3840 s (counts while playing, not in menus)",
    );
    for i in 0..16 {
        let r = QUESTS + 20 * i;
        b.f(
            r,
            Bytes(15),
            format!("quests[{i}].key"),
            L::Text,
            "quest log entry: the quest's key without QUESTLOG_ (zero-padded)",
        );
        b.f(
            r + 15,
            U8,
            format!("quests[{i}].flags"),
            L::Flags(DONE),
            "bit 0: done",
        );
        b.f(
            r + 16,
            U32,
            format!("quests[{i}].text"),
            L::None,
            "a pointer the game sets on entering a level (stale in the file; write 0)",
        );
    }
    b.f(
        QUEST_COUNT,
        U32,
        "quest_count",
        L::None,
        "quest log entries in use, 0..16",
    );
    for i in 0..4 {
        b.f(
            0x7608 + 4 * i,
            U32,
            format!("cheats[{i}]"),
            L::Flags(CHEATS),
            "cheat markers: a bit per cheat used (never read by the game)",
        );
    }
    b.0
}

/// The single bits of the levels' bitfields by meaning: `slots[s].door[i]` and so on, sized by
/// the slot's group counts.
pub fn views(s: &Save) -> Vec<Field> {
    let mut out = Vec::new();
    let b = s.bytes();
    for slot in 0..SLOT_COUNT {
        let r = SLOTS + slot * SLOT_SIZE;
        let mut bit = 0usize;
        for (g, (name, doc)) in GROUPS.iter().enumerate() {
            let n = b[r + 0x8c + g] as usize;
            for i in 0..n {
                let at = bit + i;
                if at >= 256 {
                    break;
                }
                out.push(Field {
                    path: format!("slots[{slot}].{name}[{i}]"),
                    offset: r + 0x95 + at / 8,
                    ty: Ty::Bit((at % 8) as u8),
                    label: Label::None,
                    doc: (*doc).to_owned(),
                    view: true,
                });
            }
            bit += n;
        }
    }
    out
}

/// Whether `v` is the value `f` has when unused (0, or the empty marker of bag slots, chalk
/// marks, positions and records): `show` leaves those out.
pub fn is_unused(f: &Field, v: &Value) -> bool {
    match v {
        Value::Bytes(b) => b.iter().all(|&x| x == 0),
        Value::Int(n) => {
            *n == 0
                || (*n == -1
                    && (f.label == Label::Item
                        || f.path.ends_with(".return_pos")
                        || f.path.contains(".chalk[")
                        || f.path.starts_with("player.spawn_")
                        || f.path.ends_with(".node")))
                || (*n == -2 && f.path.ends_with(".creature"))
                // An empty world timer slot (0xdeadbeef in all three words).
                || (f.path.starts_with("world.timer[") && (*n == 0xdead_beef || *n == -0x2152_4111))
        }
    }
}

/// A path as typed to the schema's: `items.NAME` to `items[i]`.
pub fn resolve(path: &str, names: Option<&Names>) -> String {
    if let Some(rest) = path.strip_prefix("items.") {
        if let Some(i) = names.and_then(|n| n.item_index(rest)) {
            return format!("items[{i}]");
        }
    }
    path.to_owned()
}

/// A field's path as shown: items by name where the names are known, slots with their level.
pub fn display_path(f: &Field, names: Option<&Names>, s: Option<&Save>) -> String {
    if let Some(i) = f
        .path
        .strip_prefix("items[")
        .and_then(|r| r.strip_suffix(']'))
        .and_then(|i| i.parse::<usize>().ok())
    {
        if let Some(n) = names.and_then(|n| n.item(i)) {
            return format!("items.{n}");
        }
    }
    if let (Some(rest), Some(nm), Some(s)) = (f.path.strip_prefix("slots["), names, s) {
        if let Some((n, tail)) = rest.split_once(']') {
            if let Some(level) = n
                .parse::<usize>()
                .ok()
                .and_then(|n| nm.slot_name(s.bytes()[LOCATION_WORLD] as usize, n))
            {
                return format!("slots[{n}:{level}]{tail}");
            }
        }
    }
    f.path.clone()
}

/// Read a bit view (or any field) as a number or bytes.
pub fn value(s: &Save, f: &Field) -> Value {
    s.get(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte belongs to exactly one field, in every world's layout.
    #[test]
    fn the_fields_cover_every_byte_once() {
        for world in 0..=9u8 {
            let mut b = vec![0u8; crate::SIZE];
            b[LOCATION_WORLD] = world;
            let s = Save::from_bytes(b).unwrap();
            let fields = fields_for(Some(&s));
            let mut at = 0;
            for f in &fields {
                assert!(!f.view);
                assert_eq!(
                    f.offset, at,
                    "world {world}: {} starts at {:#x}",
                    f.path, f.offset
                );
                at += f.ty.size();
            }
            assert_eq!(at, crate::SIZE, "world {world}");
            let mut paths: Vec<&str> = fields.iter().map(|f| f.path.as_str()).collect();
            paths.sort_unstable();
            let n = paths.len();
            paths.dedup();
            assert_eq!(paths.len(), n, "world {world}: a path twice");
        }
    }

    #[test]
    fn views_follow_the_group_counts() {
        let mut b = vec![0u8; crate::SIZE];
        let r = SLOTS + 4 * SLOT_SIZE;
        b[r + 0x8c] = 19; // triggers
        b[r + 0x8d] = 2; // doors
        let s = Save::from_bytes(b).unwrap();
        let v = views(&s);
        let door1 = v.iter().find(|f| f.path == "slots[4].door[1]").unwrap();
        assert_eq!(
            (door1.offset, door1.ty),
            (r + 0x95 + 20 / 8, Ty::Bit(20 % 8))
        );
        assert_eq!(v.len(), 21);
    }
}
