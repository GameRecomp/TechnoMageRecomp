// SPDX-License-Identifier: MIT
//! The rules a saved game must keep for the game to load and run it as saved: errors (the game
//! crashes, divides by zero or reads past a table) and warnings (the game repairs or
//! overwrites the value, or something shows wrong). Worked out with the schema (schema.rs).

use crate::names::Names;
use crate::schema::{self, Field, LOCATION_WORLD};
use crate::{worlds, Save, Value};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub error: bool,
    pub text: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.error { "error" } else { "warning" };
        write!(f, "{kind}: {}", self.text)
    }
}

/// Experience needed for each level (index: level), up to level 20.
pub const XP: [i64; 21] = [
    0, 0, 10000, 22000, 35000, 51000, 69000, 90000, 115000, 143000, 175000, 215000, 258000, 310000,
    370000, 440000, 520000, 610000, 720000, 850000, 1_000_000,
];
/// Items the item table names (the counts past it stay 0).
const ITEMS_NAMED: usize = 163;
const GOLDCOIN: usize = 51;
const CRYSTAL_GREEN: usize = 52;
/// The second Q_HONEY: unreachable by name, so a count there lands on the first on loading.
const Q_HONEY_TWIN: usize = 89;

struct Ctx<'a> {
    s: &'a Save,
    fields: Vec<Field>,
    out: Vec<Problem>,
}

impl Ctx<'_> {
    fn int(&self, path: &str) -> i64 {
        let f = self
            .fields
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no field {path}"));
        match self.s.get(f) {
            Value::Int(n) => n,
            Value::Bytes(_) => panic!("{path} is not a number"),
        }
    }

    fn bytes(&self, path: &str) -> Vec<u8> {
        let f = self.fields.iter().find(|f| f.path == path).expect(path);
        match self.s.get(f) {
            Value::Bytes(b) => b,
            Value::Int(_) => panic!("{path} is bytes"),
        }
    }

    fn error(&mut self, text: String) {
        self.out.push(Problem { error: true, text });
    }

    fn warn(&mut self, text: String) {
        self.out.push(Problem { error: false, text });
    }

    fn range(&mut self, path: &str, lo: i64, hi: i64, error: bool, why: &str) {
        let v = self.int(path);
        if !(lo..=hi).contains(&v) {
            let t = format!("{path} = {v}, outside {lo}..={hi} ({why})");
            if error {
                self.error(t);
            } else {
                self.warn(t);
            }
        }
    }

    fn same(&mut self, copy: &str, of: &str) {
        let (a, b) = (self.int(copy), self.int(of));
        if a != b {
            self.warn(format!(
                "{copy} = {a} but {of} = {b} (the header is a copy the book shows)"
            ));
        }
    }
}

/// The rules `s` breaks; empty when it keeps them all. `names` (from the executable) adds the
/// checks that need the level tables.
pub fn check(s: &Save, names: Option<&Names>) -> Vec<Problem> {
    let mut c = Ctx {
        s,
        fields: schema::fields_for(Some(s)),
        out: Vec::new(),
    };
    header(&mut c);
    location(&mut c, names);
    character(&mut c);
    items(&mut c);
    player(&mut c);
    levels(&mut c, names);
    world_state(&mut c);
    quests(&mut c, names);
    shops(&mut c);
    c.out
}

fn header(c: &mut Ctx) {
    if (
        c.int("header.picture_width"),
        c.int("header.picture_height"),
    ) != (160, 120)
    {
        c.warn("header.picture_width/height are not 160 x 120: the book shows no picture".into());
    }
    c.same("header.world", "location.world");
    c.same("header.stage", "location.stage");
    c.same("header.hp", "character.hp");
    c.same("header.hp_max", "character.hp_max");
    c.same("header.mana", "character.mana");
    c.same("header.mana_max", "character.mana_max");
    c.same("header.play_time", "play_time");
    let want = (0..5).fold(0, |m, i| {
        let owned = c.int(&format!("items[{}]", CRYSTAL_GREEN + i)) != 0;
        m | i64::from(owned) << i
    });
    if c.int("header.crystals") != want {
        c.warn(format!(
            "header.crystals = {:#x} but the crystals owned make {want:#x}",
            c.int("header.crystals")
        ));
    }
    if c.bytes("header.pad").iter().any(|&b| b != 0) {
        c.warn("header.pad is not zero".into());
    }
}

fn location(c: &mut Ctx, names: Option<&Names>) {
    let world = c.int("location.world");
    if !(1..=8).contains(&world) {
        c.error(format!(
            "location.world = {world}: the game reads its level table out of range (1..=8)"
        ));
        return;
    }
    if let Some(n) = names {
        let stage = c.int("location.stage");
        let count = n.stage_count(world as usize) as i64;
        if !(1..=count).contains(&stage) {
            c.error(format!(
                "location.stage = {stage}: {} has stages 1..={count}",
                schema::WORLDS[world as usize]
            ));
        } else if n.stage(world as usize, stage as usize).is_none() {
            c.error(format!(
                "location.stage = {stage}: that stage of {} has no level",
                schema::WORLDS[world as usize]
            ));
        }
    } else {
        c.range("location.stage", 1, 29, true, "a stage of the world");
    }
    c.range(
        "location.entry",
        0,
        23,
        true,
        "the entry point table has 24 entries",
    );
    c.range("location.map_variant", 0, 1, false, "only 0 and 1 are seen");
    c.range("videos_seen", 0, 15, true, "an index into the video table");
}

fn character(c: &mut Ctx) {
    let level = c.int("character.level");
    if !(1..=20).contains(&level) {
        c.error(format!(
            "character.level = {level}: 0 divides by zero in the stats screen, above 20 reads past the experience table"
        ));
    } else {
        let xp = c.int("character.xp");
        if xp < XP[level as usize] {
            c.warn(format!(
                "character.xp = {xp} is below level {level}'s {} (the experience bar shows wrong)",
                XP[level as usize]
            ));
        }
        if level < 20 && xp >= XP[level as usize + 1] {
            c.warn(format!(
                "character.xp = {xp} reaches level {} (the game levels up at once)",
                level + 1
            ));
        }
    }
    for a in ["str", "int", "con", "mys"] {
        c.range(
            &format!("character.{a}"),
            0,
            40,
            false,
            "attributes stop at 40",
        );
    }
    let (con, int) = (c.int("character.con"), c.int("character.int"));
    let hp_max = c.int("character.hp_max");
    if hp_max < 2 {
        c.error(format!(
            "character.hp_max = {hp_max}: below 2 the regeneration divides by zero"
        ));
    }
    if (1..=20).contains(&level) && hp_max != 5 * con + 6 * level + 4 {
        c.warn(format!(
            "character.hp_max = {hp_max}, the game makes it 5 CON + 6 level + 4 = {} on the next change",
            5 * con + 6 * level + 4
        ));
    }
    if c.int("character.mana_max") != 10 * int + 5 {
        c.warn(format!(
            "character.mana_max = {}, the game makes it 10 INT + 5 = {} on the next change",
            c.int("character.mana_max"),
            10 * int + 5
        ));
    }
    if c.int("character.hp") > hp_max {
        c.warn("character.hp is above character.hp_max".into());
    }
    if c.int("character.hp") <= 0 {
        c.warn("character.hp is not above 0".into());
    }
    if c.int("character.mana") > c.int("character.mana_max") {
        c.warn("character.mana is above character.mana_max".into());
    }
    c.range(
        "character.poison_effect",
        0,
        0x1000,
        false,
        "the effect's strength",
    );
    if c.bytes("character.pad").iter().any(|&b| b != 0) || c.int("pad") != 0 {
        c.warn("character.pad / pad are not zero".into());
    }
}

fn items(c: &mut Ctx) {
    for i in 0..256 {
        let path = format!("items[{i}]");
        let n = c.int(&path);
        if i >= ITEMS_NAMED {
            if n != 0 {
                c.warn(format!(
                    "{path} = {n}: past the item table (the game never reads it)"
                ));
            }
        } else if i == GOLDCOIN {
            if n > 999_999 {
                c.warn(format!("{path} (gold) = {n}: the display has six digits"));
            }
        } else if n > 99 && n != 255 {
            c.warn(format!(
                "{path} = {n}: the game keeps counts at 0..99 (the next change clamps it)"
            ));
        }
    }
    if c.int(&format!("items[{Q_HONEY_TWIN}]")) != 0 {
        c.warn(format!(
            "items[{Q_HONEY_TWIN}] (the second Q_HONEY) is not 0: loading adds it to items[84]"
        ));
    }
    let mut seen = Vec::new();
    for i in 0..schema::BAG_SLOTS {
        let path = format!("bag[{i}]");
        let v = c.int(&path);
        if v < 0 {
            continue;
        }
        if v >= ITEMS_NAMED as i64 {
            c.error(format!(
                "{path} = {v}: past the item table (the game reads a null name and crashes)"
            ));
            continue;
        }
        if seen.contains(&v) {
            c.warn(format!("{path}: item {v} is in the bag twice"));
        }
        seen.push(v);
        if c.int(&format!("items[{v}]")) == 0 {
            c.warn(format!("{path}: item {v} is in the bag with a count of 0"));
        }
    }
    for (slot, first, len) in [
        ("weapon", 0, 8),
        ("spell", 8, 7),
        ("tool", 23, 4),
        ("defense", 15, 8),
    ] {
        let path = format!("equip.{slot}");
        let v = c.int(&path);
        if v == 0 {
            continue;
        }
        if v > len {
            c.error(format!("{path} = {v}: the ring has {len} entries"));
        } else if c.int(&format!("items[{}]", first + v as usize - 1)) == 0 {
            c.error(format!(
                "{path} = {v}: the item is not owned (loading looks for it without a bound and very likely crashes)"
            ));
        }
    }
    c.range("bag_cursor.x", 0, 2, false, "the bag has three columns");
    c.range("bag_cursor.y", 0, 8, false, "the bag has nine rows");
}

fn player(c: &mut Ctx) {
    c.range("player.heading", 0, 4095, false, "a 12-bit angle");
    c.range(
        "player.camera",
        0,
        4095,
        false,
        "loading does not mask the camera's angle",
    );
    let torch = c.int("player.torch") & 0x7fff_ffff;
    if torch > 42_768_000 {
        c.warn(format!(
            "player.torch burns {torch}: the game keeps it at most 42768000"
        ));
    }
    c.range(
        "player.shield",
        i64::MIN,
        32767,
        false,
        "it is copied into 16 bits",
    );
    let spawn = [
        c.int("player.spawn_x"),
        c.int("player.spawn_y"),
        c.int("player.spawn_z"),
    ];
    if spawn != [-1, -1, -1] && (spawn[0] >= 0x80000 || spawn[2] >= 0x80000) {
        c.warn("player.spawn_x/z: beyond what a level slot's return position can pack".into());
    }
}

fn levels(c: &mut Ctx, names: Option<&Names>) {
    let mask = c.int("levels.mask");
    if mask >> schema::SLOT_COUNT != 0 {
        c.warn(format!(
            "levels.mask = {mask:#x}: bits from 22 on name no slot"
        ));
    }
    for slot in 0..schema::SLOT_COUNT {
        let p = format!("slots[{slot}]");
        let counts: i64 = schema::GROUPS
            .iter()
            .map(|(g, _)| c.int(&format!("{p}.count.{g}")))
            .sum();
        if counts > 256 {
            c.error(format!(
                "{p}: the groups need {counts} bits, the bitfield has 256"
            ));
        }
        if mask >> slot & 1 == 0 {
            continue;
        }
        let bits = c.bytes(&format!("{p}.bits"));
        let stray = (counts.max(0) as usize..256).find(|&b| bits[b / 8] >> (b % 8) & 1 == 1);
        if let Some(b) = stray {
            c.warn(format!(
                "{p}.bits: bit {b} is set past the groups ({counts} bits)"
            ));
        }
    }
    // The current level's slot must hold state once it was entered.
    if let Some(n) = names {
        let (w, st) = (c.int("location.world"), c.int("location.stage"));
        if let Some(slot) = n.stage_slot(w as usize, st as usize) {
            if mask >> slot & 1 == 0 && mask != 0 {
                c.warn(format!(
                    "levels.mask: the current level's slot {slot} holds no state (the level starts fresh)"
                ));
            }
        }
    }
    let creatures: Vec<(i64, i64)> = (0..16)
        .map(|i| {
            (
                c.int(&format!("enemies[{i}].hp")),
                c.int(&format!("enemies[{i}].creature")),
            )
        })
        .collect();
    for (i, (hp, cr)) in creatures.into_iter().enumerate() {
        if (hp == 0) != (cr == -2) {
            c.warn(format!(
                "enemies[{i}]: hp {hp} and creature {cr} disagree (a free record has 0 and -2)"
            ));
        }
    }
}

fn world_state(c: &mut Ctx) {
    let world = c.s.bytes()[LOCATION_WORLD];
    let len = worlds::LEN.get(world as usize).copied().unwrap_or(0);
    if len == 0 {
        return;
    }
    let state = &c.s.bytes()[schema::WORLD_STATE..schema::WORLD_STATE + 0x100];
    if state[len..].iter().any(|&b| b != 0) {
        c.warn(format!(
            "world.beyond: bytes past {}'s {len:#x} are not zero",
            schema::WORLDS[world as usize]
        ));
    }
    let timers = match world {
        1 | 6 => Some((8, 0xdead_beef_u32)),
        8 => Some((5, 0)),
        _ => None,
    };
    if let Some((n, empty)) = timers {
        let used = (0..n)
            .filter(|i| {
                let e = c.int(&format!("world.timer[{i}].event")) as u32;
                e != empty && e != 0
            })
            .count() as i64;
        let count = c.int("world.timers_used");
        if count != used {
            c.warn(format!(
                "world.timers_used = {count} but {used} timer slots are in use"
            ));
        }
    }
}

fn quests(c: &mut Ctx, names: Option<&Names>) {
    let count = c.int("quest_count");
    if !(0..=16).contains(&count) {
        c.error(format!("quest_count = {count}: the log has 16 entries"));
        return;
    }
    let mut keys = Vec::new();
    for i in 0..16 {
        let key = c.bytes(&format!("quests[{i}].key"));
        let text: String = key
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        if i < count {
            if text.is_empty() {
                c.error(format!("quests[{i}]: in use but without a key"));
            } else if keys.contains(&text) {
                c.warn(format!("quests[{i}]: {text} is in the log twice"));
            }
            if let Some(n) = names.filter(|n| !n.quests.is_empty()) {
                if !n.quests.contains(&text) {
                    c.warn(format!(
                        "quests[{i}]: {text} is no quest key of the game (it has no text; drawing it may crash)"
                    ));
                }
            }
            keys.push(text);
        } else if key.iter().any(|&b| b != 0) || c.int(&format!("quests[{i}].flags")) != 0 {
            c.warn(format!(
                "quests[{i}]: past quest_count but not empty (the game ignores it)"
            ));
        }
    }
}

/// The current world's shop list (offset 0 of its state, if it has one): types the trade
/// screen's name table has, an end marker within its entries.
fn shops(c: &mut Ctx) {
    let paths: Vec<String> = c
        .fields
        .iter()
        .filter(|f| f.path.starts_with("world.shop[") && f.path.ends_with(".item"))
        .map(|f| f.path.clone())
        .collect();
    if paths.is_empty() {
        return;
    }
    let mut ended = c
        .fields
        .iter()
        .any(|f| f.path == "world.shop_end" && c.s.get(f) == Value::Int(127));
    for p in &paths {
        let t = c.int(p);
        if t == 127 {
            ended = true;
            break;
        }
        if !(crate::names::SHOP_FIRST..=71).contains(&t) {
            c.error(format!(
                "{p} = {t}: no item type of the trade screen (-33..71; 127 ends the list)"
            ));
        }
    }
    if !ended {
        c.warn("world.shop: the list has no end marker (127)".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved game that keeps every rule: a fresh character in DREAMER's town.
    pub fn sound() -> Save {
        let mut s = Save::zeroed();
        let fields = schema::fields_for(Some(&s));
        let mut set = |path: &str, v: i64| {
            let f = fields.iter().find(|f| f.path == path).unwrap();
            s.set(f, &Value::Int(v)).unwrap();
        };
        for (p, v) in [
            ("header.picture_width", 160),
            ("header.picture_height", 120),
            ("header.world", 1),
            ("header.stage", 1),
            ("location.world", 1),
            ("location.stage", 1),
            ("header.hp", 10),
            ("character.hp", 10),
            ("header.hp_max", 10),
            ("character.hp_max", 10),
            ("header.mana_max", 5),
            ("character.mana_max", 5),
            ("character.level", 1),
        ] {
            set(p, v);
        }
        for i in 0..27 {
            set(&format!("bag[{i}]"), -1);
        }
        for i in 0..16 {
            set(&format!("enemies[{i}].creature"), -2);
        }
        s
    }

    #[test]
    fn a_sound_save_passes() {
        assert_eq!(check(&sound(), None), vec![]);
    }

    #[test]
    fn crashes_are_errors() {
        let mut s = sound();
        let fields = schema::fields_for(Some(&s));
        let f = |p: &str| fields.iter().find(|f| f.path == p).unwrap().clone();
        s.set(&f("equip.weapon"), &Value::Int(1)).unwrap(); // not owned
        s.set(&f("bag[0]"), &Value::Int(200)).unwrap();
        s.set(&f("character.level"), &Value::Int(0)).unwrap();
        let p = check(&s, None);
        assert!(p
            .iter()
            .any(|p| p.error && p.text.starts_with("equip.weapon")));
        assert!(p.iter().any(|p| p.error && p.text.starts_with("bag[0]")));
        assert!(p
            .iter()
            .any(|p| p.error && p.text.starts_with("character.level")));
    }
}
