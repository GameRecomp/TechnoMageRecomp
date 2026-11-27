// SPDX-License-Identifier: MIT
//! The current world's script state (block offsets 0x73c4..0x74c4), one layout per world,
//! worked out from the world scripts (their entry, leave and save functions and the code that
//! writes and compares each value with the dialogues, quest log entries and objects it goes
//! with). Values the analysis could not settle say so; bytes no code reads are named unused.

use crate::schema::{Label, Ty};

/// Bytes of state each world keeps (index: world 1..=8); the rest of the 256 are zero.
pub const LEN: [usize; 9] = [0, 0x94, 0xbc, 0x17, 0x100, 0xbc, 0x88, 0xd8, 0xf8];

/// A field of a world's state: offset in it, type, name (below `world.`), label, meaning.
#[derive(Debug, Clone)]
pub struct WField {
    pub off: usize,
    pub ty: Ty,
    pub name: String,
    pub label: Label,
    pub doc: String,
}

struct L(Vec<WField>);

impl L {
    fn f(
        &mut self,
        off: usize,
        ty: Ty,
        name: impl Into<String>,
        doc: impl Into<String>,
    ) -> &mut Self {
        self.l(off, ty, name, Label::None, doc)
    }

    fn l(
        &mut self,
        off: usize,
        ty: Ty,
        name: impl Into<String>,
        label: Label,
        doc: impl Into<String>,
    ) -> &mut Self {
        self.0.push(WField {
            off,
            ty,
            name: name.into(),
            label,
            doc: doc.into(),
        });
        self
    }

    fn unused(&mut self, off: usize, n: usize) -> &mut Self {
        let ty = if n == 1 { Ty::U8 } else { Ty::Bytes(n) };
        self.f(
            off,
            ty,
            format!("unused_{off:02x}"),
            "unused: no code reads or writes it",
        )
    }

    /// `n` timer slots from offset 0: an event id (`empty` when free), a countdown in 1/480 s
    /// (the event fires at <= 0) and the event's argument.
    fn timers(&mut self, n: usize, empty: &str, events: &str) -> &mut Self {
        for i in 0..n {
            let b = 12 * i;
            self.f(
                b,
                Ty::U32,
                format!("timer[{i}].event"),
                format!("timer slot {i}: event id ({empty} = free){events}"),
            );
            self.f(
                b + 4,
                Ty::I32,
                format!("timer[{i}].countdown"),
                format!("timer slot {i}: countdown in 1/480 s; the event fires at <= 0"),
            );
            self.f(
                b + 8,
                Ty::U32,
                format!("timer[{i}].arg"),
                format!("timer slot {i}: the event's argument"),
            );
        }
        self
    }

    /// A shop of `n` entries from offset 0.
    fn shop(&mut self, n: usize, name: &str) -> &mut Self {
        for i in 0..n {
            let b = 5 * i;
            self.l(b, Ty::I8, format!("shop[{i}].item"), Label::ShopItem, format!("{name} entry {i}: item type (the trade screen's own numbering; 127 ends the list, read up to 16 entries)"));
            self.f(b + 1, Ty::U8, format!("shop[{i}].stock"), format!("{name} entry {i}: units on offer (0 hides the entry); a purchase takes a pack: 10 for ammunition, 5 for chalk, else 1"));
            self.f(b + 2, Ty::U8, format!("shop[{i}].cap"), format!("{name} entry {i}: the most the merchant holds when buying back (selling needs stock < cap)"));
            self.f(
                b + 3,
                Ty::U8,
                format!("shop[{i}].buy"),
                format!("{name} entry {i}: price of a pack in 5 gold"),
            );
            self.f(
                b + 4,
                Ty::U8,
                format!("shop[{i}].sell"),
                format!(
                    "{name} entry {i}: what he pays for a pack in 5 gold (255: he does not buy it)"
                ),
            );
        }
        self
    }

    /// Bytes each a flag or stage, named `name[i]`.
    fn bytes(&mut self, off: usize, names: &[(&str, &str)]) -> &mut Self {
        for (i, (n, d)) in names.iter().enumerate() {
            self.f(off + i, Ty::U8, *n, *d);
        }
        self
    }

    /// Sorted, checked to cover `len` bytes, the rest of the 256 as `beyond`.
    fn finish(mut self, world: &str, len: usize) -> Vec<WField> {
        self.0.sort_by_key(|f| f.off);
        let mut at = 0;
        for f in &self.0 {
            assert_eq!(
                f.off, at,
                "{world}: {} at {:#x}, expected {at:#x}",
                f.name, f.off
            );
            at += f.ty.size();
        }
        assert_eq!(at, len, "{world}: covers {at:#x} of {len:#x}");
        if len < 0x100 {
            self.0.push(WField {
                off: len,
                ty: Ty::Bytes(0x100 - len),
                name: "beyond".into(),
                label: Label::None,
                doc: format!("past the {world} state ({len:#x} bytes): always zero"),
            });
        }
        self.0
    }
}

/// The layout of `world`'s state (empty for a world outside 1..=8).
pub fn layout(world: u8) -> Vec<WField> {
    match world {
        1 => dreamer(),
        2 => steamer(),
        3 => thehive(),
        4 => forest(),
        5 => canyon(),
        6 => tower(),
        7 => vulcano(),
        8 => ruins(),
        _ => Vec::new(),
    }
}

const DAY_QUESTS: &[&str] = &["BOOKS", "ROSE", "BALLON", "FERN", "STULLE", "WINE"];
const TOWN_INFO: &[&str] = &["QUAKE", "SHACH", "DANNY", "RACES", "LAW"];

fn dreamer() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.timers(8, "0xdeadbeef", "; 0x100 the bell (arg: strikes left), 0xa the mystic music (not kept in a save: the save drops ids 1..0xff)");
    l.f(0x60, Ty::U8, "night", "0 day, 1 night (set in the tutorial room once the combat tutorial is beaten and the COMPASS held): the town's doors lead to the night town, the day quest log entries go, people say their night lines");
    l.f(
        0x61,
        Ty::U8,
        "clock",
        "0..6: +1 a day quest finished; the clock's picture and the bell's strikes",
    );
    l.l(0x62, Ty::U8, "day_quests_done", Label::Flags(DAY_QUESTS), "day quests done (BOOKS: the book to Librarius, ROSE: Saris, BALLON: Danny, FERN: Salik, STULLE: Larisa, WINE: Hernt); all six (0x3f): the uncle's note goes, his door opens");
    l.l(0x63, Ty::U8, "town_info", Label::Flags(TOWN_INFO), "the town's news heard (QUAKE: guards, Sandem, Firna; SHACH: guard 2, Listach, Glacnore; DANNY: Danny, Klarisa, the old man, Ansgar; RACES and LAW: the books, Sabrina, Salik); all five (0x1f): the messengers come");
    l.l(
        0x64,
        Ty::U8,
        "day_quests_rewarded",
        Label::Flags(DAY_QUESTS),
        "day_quests_done when last rewarded: each new bit gives 1000 experience",
    );
    l.l(
        0x65,
        Ty::U8,
        "town_info_rewarded",
        Label::Flags(TOWN_INFO),
        "town_info when last rewarded: each new bit gives 400 experience",
    );
    l.bytes(0x66, &[
        ("drunken_talked", "1 = the drunkard talked to (adds QUESTLOG_FINDKEY)"),
        ("danny_talked", "1 = Danny talked to (QUESTLOG_BALLON); done is day_quests_done BALLON"),
        ("hernt", "0 not talked to, 1 wants wine (QUESTLOG_WINE), 2 wine brought (WINE done, clock + 1)"),
        ("salik", "0 not talked to, 1 wants a fern (QUESTLOG_FERN), 2 fern brought (FERN and LAW, clock + 1); the shadow fern gives Q_FARN below 2"),
        ("bachus", "0 not met, 1 met (FINDKEY done), 2 wine bought (5 gold; needs hernt 1)"),
        ("larisa_talked", "1 = Larisa talked to (QUESTLOG_STULLE)"),
        ("talissa_talked", "0 = Talissa's first line, else her second (nothing in the world script sets it)"),
        ("saris_talked", "1 = Saris talked to"),
        ("sabrina", "0 = Sabrina's first line (sets town_info RACES), 1 = her last"),
        ("vesneggs", "the night's rats: 0 not talked to, 1 told, rats left, 2 all rats dead, 3 thanked (QUESTLOG_RATS done)"),
        ("sengarn_thanked", "1 = Sengarn thanked the hero (needs BOOKS)"),
        ("librarius_talked", "1 = Librarius talked to (QUESTLOG_BOOKS)"),
        ("bellboy_talked", "1 = the bellboy talked to"),
        ("rissen", "0 not met, 1 met (GOUNCLE done), 2 sent to the cellar for the compass (needs messenger_came; QUESTLOG_COMPASS), 3 at night sent to his mother (COMPASS done)"),
        ("firna", "0 not met, 1 met by day (adds GOUNCLE, town_info QUAKE), 2 found at night (FINDMOTHER done, QUESTLOG_TRANQ added)"),
        ("syrandoron", "0 not met, 1 met, 2 Glacnore's news passed on (needs glacnore 2)"),
        ("hint_weapon", "1 = the weapon hint shown (WP_DAGGER held)"),
        ("silver_key_taken", "1 = KEY_SILVER picked up (the key is gone on later visits)"),
        ("hint_questitem", "1 = the quest item hint shown"),
        ("hint_useable", "1 = the usable item hint shown"),
        ("tutorial_hint_due", "1 = the next tutorial hint shows (set on entering any DREAMER level but the first)"),
        ("tutorial_hint_step", "0..5: 0 the map, 2 the map's shortcut, 4 experience, 1 and 3 silent, 5 done"),
        ("glacnore", "0 not met (sets town_info SHACH), 1 wants 5 gold, 2 paid, news bought"),
        ("guard3_asleep", "1 = the tranquilliser given to Firna: guard 3 sleeps, the back door lets through"),
        ("messenger_came", "1 = a messenger scene played (people say to go to the uncle; rissen can reach 2)"),
        ("combat_tutorial", "0 not started, 1 running (its doors shut, enemies), 2 beaten (its door opens)"),
        ("idle_hint", "level 5's idle hint: 0 off, 1 armed, 2 shown or cancelled"),
    ]);
    l.unused(0x81, 3);
    l.f(
        0x84,
        Ty::I32,
        "idle_hint_countdown",
        "1/480 s, from 7200 (15 s); the hint shows at <= 0 while the hero stands still",
    );
    l.f(
        0x88,
        Ty::I32,
        "idle_hint_x",
        "the hero's x when the hint was armed (moving cancels it)",
    );
    l.f(
        0x8c,
        Ty::I32,
        "idle_hint_z",
        "the hero's z when the hint was armed",
    );
    l.f(
        0x90,
        Ty::U8,
        "timers_used",
        "timer slots in use, 0..8 (no more are taken at 8)",
    );
    l.unused(0x91, 3);
    l.finish("DREAMER", 0x94)
}

const STEAMER_FLAGS: &[&str] = &[
    "spooky_scene",
    "miners_overheard",
    "corks_all_in",
    "crypt_c_cleared",
    "asfulo_caught_hero",
    "pneul_fitted",
    "oil_applied",
    "tanks_balanced",
    "left_house2_with_oilcan",
    "diary_read",
    "explosion_played",
    "fountain_running",
    "tumpo_latch",
    "crowbar1_placed",
    "crowbar2_placed",
    "arrived_steamer_a",
    "lockmaster_door_open",
    "miner_rescue_started",
    "spooky2_scene",
    "shop_open",
    "firewall_trap_running",
    "gold_key_dropped",
];

fn steamer() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.shop(13, "MERCHANT_TRADE");
    l.unused(0x41, 3);
    l.l(0x44, Ty::U32, "flags", Label::Flags(STEAMER_FLAGS), "STEAMER's events (tumpo_latch, shop_open, firewall_trap_running only while they last; bits 22..31 unused)");
    l.bytes(0x48, &[
        ("day_phase", "0 the first day, 1 night (the smith took the ore; the exits lead to the night levels 12..15), 2 the second day"),
        ("tumpo_beans", "0 nothing, 1 Q_LEBERTRAN put in Tumpo's oven, 2 beans cooked into the stew (Q_SUPPE_LEER taken)"),
        ("baras", "0 not met, 1 guards the miners' door (QUESTLOG_BARAS), 2 given the tainted stew: gone, the miners' door leads to level 22"),
        ("raake_cat", "0 not talked to, 1 find the cat (QUESTLOG_FINDCAT), 2 Q_KATZE handed over"),
        ("fathers_door", "0 not reached, 1 found locked (FINDHOUSE done, GETKEY added), 2 opened with KEY_COPPER (leads to level 19)"),
        ("smith", "0 not talked to, 1 talked to, 2 asked for ore (GETORE), 3 ore brought (night falls), 4 the second day, key given (GETKEY done), 5 sent to the power plant (needs flags explosion_played)"),
        ("engineer", "0 not met, 1 talked to (QUESTLOG_ENGINEER), 2 job taken (names the repair still missing), 3 done"),
        ("sonta_talked", "1 = Sonta talked to (QUESTLOG_CORK)"),
        ("bwall_blown", "1 = BWALL (level 5) blown with Q_TNT (broken again on loading)"),
        ("bwall2_blown", "1 = BWALL2 (level 7) blown"),
        ("bwall3_blown", "1 = BWALL3 (level 5) blown"),
        ("in_room_i", "1 while the hero is in crypt A's room I (its plasma door opens while all four places are taken)"),
        ("room_h_fight", "1 = room H's plasma door shut, its fight running; 0 once its enemies are dead"),
        ("room_f_fight", "the same for crypt B's room F"),
        ("jumped_c_to_a", "1 = took the jump to crypt A: its plasma door B stays open"),
    ]);
    for i in 0..6 {
        l.f(0x57 + i, Ty::U8, format!("cork[{i}]"), format!("CORK{}: 0 open, 1 plugged with Q_KORK (1..4 in STEAMER_A, 5 and 6 in STEAMER_C); all six: flags corks_all_in", i + 1));
    }
    l.bytes(0x5d, &[
        ("hop_game", "hopscotch with KID1: 0 never started, 1 between rounds or failed, 2 a round running, 3 won (SCRAPYARD done, the yard's fence broken)"),
        ("hop_round_len", "fields of the round: 3, then 5, then 7"),
        ("hop_jumped", "1 = jumped since the last field (while a round runs)"),
        ("hop_pos", "the next field (reset by a fault and each round)"),
        ("kid1_talked", "1 = KID1 talked to (QUESTLOG_SCRAPYARD)"),
    ]);
    for i in 0..3 {
        l.f(
            0x62 + i,
            Ty::U8,
            format!("tank_level[{i}]"),
            format!(
                "pressure in tank {}, 0..6 (6/0/0 when the fountain restarts; 2/2/2 balances them)",
                i + 1
            ),
        );
    }
    for i in 0..3 {
        l.f(
            0x65 + i,
            Ty::U8,
            format!("junction[{i}]"),
            format!(
                "JUNCT{}'s direction 0..3 (its lever cycles 0, 2, 1, 3)",
                i + 1
            ),
        );
    }
    l.bytes(0x68, &[
        ("lorry_node", "the mine lorry's track node 0..12 (7..9 junctions, 10 and 11 under the rock chute, 12 the dead end at BWALL4)"),
        ("lorry_dir", "the lorry's direction 0..3"),
        ("lorry_loaded", "1 = rocks fell into the lorry"),
        ("bwall4_smashed", "1 = the loaded lorry smashed BWALL4: the track goes on, level 4's lift door opens"),
    ]);
    l.l(
        0x6c,
        Ty::U16,
        "lorry_angle",
        Label::Angle,
        "the lorry's heading (restored on loading)",
    );
    l.bytes(
        0x6e,
        &[
            (
                "lorry_moving",
                "1 while the lorry is pushed (levers are ignored)",
            ),
            (
                "miner_freed",
                "1 = the injured miner rescued (FREEMINER done)",
            ),
            ("jola_talked", "1 = Jola talked to"),
            ("room_i_scene_seen", "1 = room I's camera scene played"),
            (
                "switchman_scene",
                "1 = the switchman's kick-out scene running (level 17)",
            ),
            (
                "light_switch_on",
                "the switch house's light: 1 on (the switchman goes to turn it off), 0 off",
            ),
            (
                "lorry_exploded",
                "1 = the lorry exploded (level 7's crash wall is used again)",
            ),
            (
                "talked_soldier1",
                "0 = guard 1's first line next, 1 = his night or second line",
            ),
            ("talked_soldier2", "the same for guard 2"),
        ],
    );
    for (i, who) in [
        "STATISTM1",
        "STATISTM2",
        "STATISTM3",
        "STATISTM4 (adds SCRAPYARD)",
        "STATISTW1 (adds FREEMINER)",
        "STATISTW2",
        "STATISTW3 (adds ENGINEER)",
        "STATISTW4 (adds SCRAPYARD)",
        "INNGUEST2",
        "NURSE1",
        "NURSE2",
        "VICTIM",
        "MOSTIK",
        "RULOF",
        "NANDA (adds FREEMINER)",
        "GRIMHILDE",
    ]
    .iter()
    .enumerate()
    {
        l.f(
            0x77 + i,
            Ty::U8,
            format!("talked[{i}]"),
            format!("{who}: 0 = the first line next (then 1), 1 = the second"),
        );
    }
    l.bytes(0x87, &[
        ("save_hint_seen", "1 = the scrapyard's save hint fired (its set-up is rebuilt on entry)"),
        ("trashy_killed", "0 alive (the junk guard blocks the way), 1 killed (Q_PNEUL dropped again if never taken)"),
    ]);
    l.unused(0x89, 3);
    l.f(
        0x8c,
        Ty::I32,
        "switchhouse_light",
        "the switch house's light blend 0..4096 (towards light_switch_on)",
    );
    l.f(
        0x90,
        Ty::U16,
        "switchman.x",
        "ST_SWITCHMAN: x >> 8 (restored on entering level 17 from a save)",
    );
    l.f(0x92, Ty::U16, "switchman.z", "ST_SWITCHMAN: z >> 8");
    l.l(
        0x94,
        Ty::U16,
        "switchman.heading",
        Label::Angle,
        "ST_SWITCHMAN: heading",
    );
    l.f(
        0x96,
        Ty::U16,
        "switchman.waypoint",
        "ST_SWITCHMAN: path node (0xffff = none)",
    );
    l.f(0x98, Ty::U8, "switchman.ai_state", "ST_SWITCHMAN: 1 sent to the switch, 3 at it, 4 turned it off, 7 noticed the hero (0 idle, unsure)");
    l.f(
        0x99,
        Ty::U8,
        "switchman.path_dir",
        "ST_SWITCHMAN: walking direction along his path (0 or 1; which is forward unsure)",
    );
    l.l(0x9a, Ty::U8, "corks_found", Label::Flags(&["haus1", "haus2", "haus3", "gasthaus", "haus1_dropped", "haus2_dropped", "haus3_dropped", "gasthaus_dropped"]), "Q_KORK's search spots: a cork collected there (bits 0..3), or dropped but not yet taken (4..7, dropped again on entry)");
    l.unused(0x9b, 1);
    for (n, base) in [(1, 0x9c), (2, 0xac)] {
        let p = format!("lifepool{n}");
        l.f(
            base,
            Ty::I32,
            format!("{p}.level"),
            format!(
                "healing fountain {n} (crypt {}): its fill, HP << 12 (30 HP when full)",
                if n == 1 { "A" } else { "B" }
            ),
        );
        l.f(
            base + 4,
            Ty::I32,
            format!("{p}.capacity"),
            "its full level, HP << 12",
        );
        l.f(
            base + 8,
            Ty::I32,
            format!("{p}.drain"),
            "what is still to drain after a use",
        );
        l.f(
            base + 0xc,
            Ty::U8,
            format!("{p}.refilling"),
            "1 = refilling",
        );
        l.f(
            base + 0xd,
            Ty::U8,
            format!("{p}.ready"),
            "1 = usable (a use sets 0)",
        );
        l.unused(base + 0xe, 2);
    }
    l.finish("STEAMER", 0xbc)
}

fn thehive() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.bytes(0, &[
        ("symbol_puzzle_on", "level 8: 1 between its start and stop triggers (the symbols are checked only then)"),
        ("door_north_open", "1 = DOORNORTH open (with SYMBOL_2, 4, 9 active; one door at a time: north, east, south)"),
        ("door_east_open", "1 = DOOREAST open (SYMBOL_7, 3, 6); 1 at the world's start"),
        ("door_south_open", "1 = DOORSOUTH open (SYMBOL_1, 5, 8)"),
        ("riddle1_presses", "level 2: SWITCHRIDDLE1 pressed, counting to 2"),
        ("riddle2_presses", "level 2: SWITCHRIDDLE2 pressed, counting to 2"),
        ("fredo", "Fredo: 0 not talked to, 1 the short pass (gives 2000 experience, then 2), 2 done"),
        ("fairy_exit_open", "1 = the fairy exit opened after the big spider died (opened again on entering level 10)"),
        ("father_met", "0: QUESTLOG_FATHER added on entry; 1 = the father's scene done (EXITHIVE added, 3000 experience)"),
        ("teleporter_a", "level 6: 1 = plant A activated with TELEKEYBLK (its ring and fields on)"),
        ("teleporter_b", "1 = plant B activated with TELEKEYRED"),
        ("teleporter_c", "1 = plant C activated with TELEKEYBLU"),
        ("teleporter_d", "1 = plant D activated with TELEKEYGRE"),
    ]);
    for (i, (lvl, n)) in [("level 4", 7), ("level 7", 2), ("level 8", 2)]
        .iter()
        .enumerate()
    {
        l.f(0x0d + i, Ty::U8, format!("bridges[{i}]"), format!("{lvl}: bit i = BRIDGE(i+1) extended (bits 0..{}), bit 7 = taken when leaving the level", n - 1));
    }
    l.bytes(
        0x10,
        &[
            (
                "crystal",
                "level 7: 0 the crystal on, 1 switched off, 2 the vision shown",
            ),
            ("spiderdoor_shown", "1 = the spider door's opening shown"),
            (
                "spider_fight_started",
                "1 = the spider's prelude done (with spider_dead 0 the boss comes back on entry)",
            ),
            ("spider_dead", "1 = the big spider killed"),
            ("intro_done", "1 = level 1's arrival done (Q_KORK taken)"),
            (
                "foundstone_said",
                "1 = Melvin's line on the first stone found",
            ),
        ],
    );
    l.l(
        0x16,
        Ty::U8,
        "bwalls",
        Label::Flags(&["bwall1", "bwall2", "bwall_1_level2"]),
        "walls broken (BWALL1 and BWALL2 in level 5, BWALL_1 in level 2; broken again on entry)",
    );
    l.finish("THEHIVE", 0x17)
}

const FOREST_FLAGS: &[&str] = &[
    "harp_strung",
    "lisa_latch",
    "fairy_gold_used",
    "near_radunzchen",
    "mantis_silk",
    "shrink_exit_pending",
    "basket1_placed",
    "basket2_placed",
    "mantis_scene",
    "maze_cleared",
    "bit10",
    "heliana_smiled",
    "raga_defeated",
    "poppa_staked",
    "fiona_latch",
    "duel_light1",
    "duel_light2",
    "race_won",
    "after_race_scene",
    "gwen_out",
    "beam_effect",
    "target_shot",
    "horn_solved",
    "shop_open",
    "right_formula_read",
    "wrong_formula_read",
    "race_quest_given",
    "level3_intro",
    "mantises_killed",
    "raft_at_target",
];

/// A raft's record (0x2e bytes): snapshot when leaving the level and saving, restored on
/// entering its level from a save.
fn raft(l: &mut L, base: usize, name: &str) {
    let p = name.to_lowercase();
    l.f(
        base,
        Ty::U32,
        format!("{p}.pos"),
        format!("{name}: position, bits 0..9 x / 512, 10..19 y / 512, 20..29 z / 512"),
    );
    l.f(
        base + 4,
        Ty::I32,
        format!("{p}.rock_phase"),
        "rocking clock",
    );
    l.f(
        base + 8,
        Ty::I32,
        format!("{p}.from_x"),
        "the current leg's start: x",
    );
    l.f(
        base + 0xc,
        Ty::I32,
        format!("{p}.from_z"),
        "the current leg's start: z",
    );
    l.f(
        base + 0x10,
        Ty::I32,
        format!("{p}.to_x"),
        "its destination: x",
    );
    l.f(
        base + 0x14,
        Ty::I32,
        format!("{p}.to_z"),
        "its destination: z",
    );
    l.f(
        base + 0x18,
        Ty::I32,
        format!("{p}.leg_len"),
        "the leg's length",
    );
    l.f(
        base + 0x1c,
        Ty::I32,
        format!("{p}.progress"),
        "-1 idle, else the distance travelled",
    );
    l.f(
        base + 0x20,
        Ty::I16,
        format!("{p}.turning"),
        "-1 no, 0 turning to turn_heading",
    );
    l.f(
        base + 0x22,
        Ty::I16,
        format!("{p}.dip_phase"),
        "-1 none, 0..0x800 the dip when someone steps on",
    );
    l.l(
        base + 0x24,
        Ty::U16,
        format!("{p}.turn_heading"),
        Label::Angle,
        "the turn's target heading",
    );
    l.f(
        base + 0x26,
        Ty::U16,
        format!("{p}.ease_mode"),
        "0 or 1, set at each leg's start",
    );
    l.f(
        base + 0x28,
        Ty::U16,
        format!("{p}.rider_latch"),
        "1 while someone is aboard",
    );
    l.l(
        base + 0x2a,
        Ty::U16,
        format!("{p}.heading"),
        Label::Angle,
        "the raft's heading",
    );
    l.f(
        base + 0x2c,
        Ty::U16,
        format!("{p}.activated"),
        "0 = does nothing until first boarded",
    );
}

fn forest() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.shop(13, "LONICERA_TRADE");
    l.unused(0x41, 3);
    raft(&mut l, 0x44, "RAFT");
    l.unused(0x72, 2);
    raft(&mut l, 0x74, "RAFT2");
    l.unused(0xa2, 2);
    for k in 0..5 {
        let b = 0xa4 + 8 * k;
        l.f(
            b,
            Ty::U32,
            format!("cable[{k}].pos"),
            format!("CABLE{}: position, packed as a raft's (level 2)", k + 1),
        );
        l.f(
            b + 4,
            Ty::U16,
            format!("cable[{k}].board_timer"),
            "counts up to 0x200 while someone waits aboard, then it departs",
        );
        l.f(
            b + 6,
            Ty::U16,
            format!("cable[{k}].state"),
            "0 to end B, 1 to end A, 2 parked, waiting, 3 arrived with the rider aboard",
        );
    }
    l.l(0xcc, Ty::U32, "flags", Label::Flags(FOREST_FLAGS), "FOREST's events (the latches, near_radunzchen, poppa_staked, the duel lights, beam_effect, shop_open only while they last; bits 30, 31 unused)");
    l.bytes(0xd0, &[
        ("l10_once", "level 10's set-up ran (cleared on every entry)"),
        ("siniver", "0 not talked to, 1 talked to (QUESTLOG_FINDBUCKETF), 2 Merina got her water, 3 ticket given (QUESTLOG_FREEGWEN), 4 Gwen freed, done"),
        ("merina", "0 not talked to, 1 asked for water, 2 Q_BUCKET_F handed over"),
        ("myrka", "0 not talked to, 1 sent to the meadow (QUESTLOG_GOMEADOW), 2 fairy gold made"),
        ("elebeth", "0 not talked to, 1 wants Q_MANTISSILK and Q_BEESTING (QUESTLOG_REAGENTS), 2 both given"),
        ("duna", "0 not talked to, 1 seals quest given (FINDSEALS, FINDHERB), 2 the three hearts brought, 3 Q_WEED too, done"),
        ("teli_talked", "1 = Teli talked to"),
        ("fendia_talked", "1 = Fendia talked to"),
        ("jeck", "0 not talked to, 1 passing, 2 riddle asked, 3 solved"),
        ("corina", "0 not talked to, 1 basket quest given (QUESTLOG_FINDBASKET), 2 both baskets placed"),
        ("irwen", "0 not talked to, 1 talked to, 2 wants Q_BUCKET_E, 3 bucket swapped for Q_FLOWER_WI and Q_BUCKET_F"),
        ("aruna", "0 not talked to, 1 wants fork and knife (QUESTLOG_FINDTOOLS), 2 both shown, Q_STRINGS given, 3 harp strung"),
        ("apydia", "0 not talked to, 1 Q_BEEGLASS given (QUESTLOG_GETBEES), 2 five bees brought"),
        ("lonicera", "0 not talked to, 1 wants honey, 2 Q_HONEY brought, 3 ray glass offered (needs horpach 5), 4 ray glass given (needs horpach 7); her shop opens from 2"),
        ("horpach", "0 not talked to, 1 wants the wand, 2 got Q_WAND, 3 asked for flowers, 4 all flowers but no formula, 5 antidote made, 6 duel ready, 7 lost the duel (Q_ZAHNRAD taken)"),
        ("amber_talked", "1 = Amber talked to: the horn melody puzzle is armed"),
        ("bombground1", "1 = BOMBGROUND1 (level 6, heart C) blown"),
        ("bombground2", "1 = BOMBGROUND2 (level 4, heart B) blown"),
        ("bombground3", "1 = BOMBGROUND3 (level 3, heart A) blown"),
        ("heliana", "0 not healed, 1 her smile scene started, 2 talked to after it"),
    ]);
    l.f(
        0xe4,
        Ty::I8,
        "raft_route",
        "the level 0x1b raft's route (row of its route table)",
    );
    l.f(0xe5, Ty::I8, "raft_step", "the next waypoint of the route");
    l.f(
        0xe6,
        Ty::I8,
        "raft_target",
        "chosen route: 0 the start, 1..3 the switches, 4 the exit",
    );
    l.bytes(0xe7, &[
        ("aruna_fork", "not 0 = Q_TUNINGFORK shown to Aruna"),
        ("aruna_knife", "not 0 = Q_KNIFE shown to Aruna"),
        ("gwen_freed", "1 = Raga's and Gwen's scene done"),
        ("fiona_won", "1 = Fiona's game won"),
        ("sarina", "0 not talked to, 1 asked to clear the maze (QUESTLOG_HELPSARINA), 2 cleared"),
        ("leona", "0 not talked to, 1 the maze not done, 2 flower given"),
        ("meriven", "0 not talked to, 1 talked to, 2 wants Q_RAYGLASS_E, 3 told how to use it (QUESTLOG_USEVESSEL), 4 Q_RAYGLASS_F shown"),
    ]);
    for i in 0..5 {
        l.f(
            0xee + i,
            Ty::U8,
            format!("bee_caught[{i}]"),
            format!("1 = BEE{} gone (level 2; hidden on entry)", i + 1),
        );
    }
    l.bytes(
        0xf3,
        &[
            ("guardian_passed", "1 = past the Guardian with Q_TICKET"),
            (
                "duna_herb_seen",
                "1 = Q_WEED shown to Duna before the seals",
            ),
            ("fiona_met", "1 = Fiona talked to"),
            (
                "first_l1d",
                "first arrival in level 0x1d done (it clears the quest log)",
            ),
            ("first_l1", "first arrival in level 1 done"),
            ("first_l1a", "first arrival in level 0x1a done"),
            ("save_hint", "1 = level 0x1c's save hint fired"),
            (
                "irwen_bucket_asked",
                "1 = Irwen asked for a bucket (QUESTLOG_FINDBUCKET)",
            ),
            ("sarina_rewarded", "1 = Sarina's reward paid"),
            ("kobold_door_open", "1 = the kobold door in level 5 opened"),
            (
                "raft_respawn",
                "1 = the raft's respawn point reached (stops its route's reset)",
            ),
        ],
    );
    l.f(
        0xfe,
        Ty::I16,
        "trap_rearm",
        "1/480 s until WD_TRAP is armed again (1440 when it fires)",
    );
    l.finish("FOREST", 0x100)
}

const CANYON_FLAGS: &[&str] = &[
    "oldone_rescued",
    "raft_puzzle_solved",
    "statues_done",
    "lens_beam_running",
    "raft1_far",
    "raft2_far",
    "raft3_far",
    "creatures_spawned",
    "creatures_dead",
    "lever_drill1_a",
    "lever_drill1_b",
    "lever_drill2_a",
    "lever_drill2_b",
    "drill_a_driving",
    "drill_b_driving",
    "pump_a_running",
    "pump_b_running",
    "mana_at_elevator",
    "pumps_synchronised",
    "crane_solved",
    "force_field_destroyed",
    "talis_scene1",
    "deathball_defeated",
    "lever_fitted",
    "elevator_used",
    "shop_open",
    "bwall_blown",
    "crane_xp_paid",
];

/// An object's 20-byte snapshot (CANYON's drills): position, movement target, its state.
fn snapshot20(l: &mut L, base: usize, name: &str) {
    l.f(
        base,
        Ty::U32,
        format!("{name}.pos"),
        "position: bits 0..9 x / 512, 10..19 y / 512, 20..29 z / 512 (y is not restored)",
    );
    l.f(
        base + 4,
        Ty::U32,
        format!("{name}.target"),
        "movement target, packed the same way",
    );
    for (i, d) in [
        "object +0x8a",
        "object +0x8c",
        "object +0x88",
        "object +0x8e",
        "object +0x8c again",
    ]
    .iter()
    .enumerate()
    {
        l.f(
            base + 8 + 2 * i,
            Ty::U16,
            format!("{name}.w{i}"),
            format!("a copy of the {d} (meaning unsure)"),
        );
    }
    l.f(
        base + 0x12,
        Ty::U8,
        format!("{name}.move_state"),
        "0 idle, 1 turning, 2 ready, 3 driving",
    );
    l.unused(base + 0x13, 1);
}

fn canyon() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.shop(12, "TORLORE_TRADE");
    l.l(
        0x3c,
        Ty::I8,
        "shop_end",
        Label::ShopItem,
        "the shop's end marker (127)",
    );
    l.unused(0x3d, 7);
    l.f(
        0x44,
        Ty::I32,
        "crane_x",
        "CMINE KRAN's position: x (saved leaving level 9, restored on entry when not 0)",
    );
    l.f(0x48, Ty::I32, "crane_y", "the crane's position: y");
    l.f(0x4c, Ty::I32, "crane_z", "the crane's position: z");
    l.l(
        0x50,
        Ty::U32,
        "flags",
        Label::Flags(CANYON_FLAGS),
        "CANYON's events (lens_beam_running and shop_open while they last; bits 28..31 unused)",
    );
    l.l(
        0x54,
        Ty::U32,
        "chests",
        Label::Flags(&[
            "bit0",
            "redcrystal6",
            "bit2",
            "bit3",
            "bit4",
            "crystal_blue_2",
            "bit6",
            "bit7",
            "bit8",
            "bit9",
            "bit10",
            "bit11",
            "crystal_yellow_1",
        ]),
        "containers opened (restored opened): REDCRYSTAL6, CRYSTAL_BLUE_2, CRYSTAL_YELLOW_1",
    );
    l.f(
        0x58,
        Ty::U8,
        "sock_red_1",
        "1 = CNCRYSTAL_RED put in at E_RED1",
    );
    l.f(
        0x59,
        Ty::U8,
        "sock_red_3",
        "1 = CNCRYSTAL_RED put in at E_RED3",
    );
    l.f(
        0x5a,
        Ty::U8,
        "sock_blue_1",
        "1 = CNCRYSTAL_BLUE put in at E_BLUE1",
    );
    l.unused(0x5b, 2);
    l.f(
        0x5d,
        Ty::U8,
        "sock_blue_4",
        "1 = CNCRYSTAL_BLUE put in at E_BLUE4",
    );
    l.f(
        0x5e,
        Ty::U8,
        "sock_green_1",
        "1 = CNCRYSTAL_GREEN put in at E_GREEN1",
    );
    l.f(
        0x5f,
        Ty::U8,
        "sock_green_2",
        "1 = CNCRYSTAL_GREEN put in at E_AFTERBABE",
    );
    l.unused(0x60, 1);
    l.f(
        0x61,
        Ty::U8,
        "sock_yellow_2",
        "1 = CNCRYSTAL_YEL put in at E_YELLOW2",
    );
    l.f(
        0x62,
        Ty::U8,
        "sock_yellow_3",
        "1 = CNCRYSTAL_YEL put in at E_YELLOW3",
    );
    l.bytes(
        0x63,
        &[
            (
                "door1_open",
                "1 = the first door's trigger fired (SHUTDOOR, NOCOLOR_1 open)",
            ),
            (
                "door2_open",
                "1 = the second door's trigger fired (NOCOLOR_2 opens)",
            ),
            (
                "lever2",
                "0 no lever, 1 Q_LEVER fitted (3000 experience), 2 pulled, PLATFORM1 moved",
            ),
            ("deadend_rock", "1 = the dead end's earthquake played"),
            (
                "torlore_met",
                "0 = Torlore's first talk next; 1 = talks open his shop",
            ),
        ],
    );
    l.l(
        0x68,
        Ty::U8,
        "statues",
        Label::Flags(&["statue1", "statue2", "statue3", "statue4"]),
        "statues smashed (all four: QUESTLOG_MONSTERS done)",
    );
    l.l(
        0x69,
        Ty::U8,
        "lenses_set",
        Label::Flags(&["red", "green", "blue"]),
        "lenses placed",
    );
    l.f(
        0x6a,
        Ty::U8,
        "lens_turned",
        "the lens in the beam: 1, 2 or 4 (each use halves it, 0 wraps to 4)",
    );
    l.l(
        0x6b,
        Ty::U8,
        "field_broken",
        Label::Flags(&["red", "green", "blue"]),
        "force field colours destroyed (all three: QUESTLOG_MINE done, 10000 experience)",
    );
    l.f(
        0x6c,
        Ty::U8,
        "lenskeeper",
        "0 not talked to, 1 wants Q_HORN (QUESTLOG_LENS), 2 horn given, Q_LENS_BLUE received",
    );
    for i in 0..4 {
        l.f(
            0x6d + i,
            Ty::U8,
            format!("raft[{i}]"),
            format!(
                "raft {}'s stop: 0 or 1 (its lever toggles it); 2 when all four are solved",
                i + 1
            ),
        );
    }
    l.bytes(0x71, &[
        ("talis_goon", "1 = the mine task given"),
        ("talis", "0 not met, 1 met (the scorpions come), 2 scorpions dead, 3 rewarded (QUESTLOG_MINE added)"),
        ("babe_scene", "1 = the cave scene played"),
        ("l3_cleared", "1 = level 3 cleared"),
        ("plant_key", "1 = TELEKEYBLK placed: teleporters A1 and A2 on"),
    ]);
    l.f(
        0x76,
        Ty::I8,
        "drill_a_step",
        "drill A's step 0..5 (4 = at its end)",
    );
    l.f(
        0x77,
        Ty::I8,
        "drill_b_step",
        "drill B's step 0..4 (3 = BWALL blown)",
    );
    snapshot20(&mut l, 0x78, "drill_a");
    snapshot20(&mut l, 0x8c, "drill_b");
    l.f(
        0xa0,
        Ty::I8,
        "crane_dir_x",
        "the crane's direction along x: +1 or -1",
    );
    l.f(
        0xa1,
        Ty::I8,
        "crane_dir_z",
        "the crane's direction along z: +1 or -1",
    );
    for i in 0..20 {
        l.f(
            0xa2 + i,
            Ty::U8,
            format!("crane_grid[{i}]"),
            format!(
                "crane grid cell row {}, column {}: 0 empty, 1..3 CRATE1..3",
                i / 4,
                i % 4
            ),
        );
    }
    l.bytes(
        0xb6,
        &[
            ("crane_load", "the crate on the hook (0 none, 1..3)"),
            ("hammer_done", "1 = QUESTLOG_HAMMER done"),
            ("statues_xp", "1 = the statues' experience paid"),
            (
                "deathball_hint",
                "1 = the save hint shown (the deathball waits at its stand)",
            ),
        ],
    );
    l.unused(0xba, 2);
    l.finish("CANYON", 0xbc)
}

fn tower() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.timers(8, "0xdeadbeef", "; ids per level (1 level 2's switch check, 0x100 the library door, 0x101..0x103 level 5 and 6 events, 0x105 the bomb's ticking): the save drops ids 1..0xff");
    l.l(
        0x60,
        Ty::U32,
        "flags",
        Label::Flags(&[
            "librarian_scene",
            "beam_area5_powered",
            "battle_running",
            "talis_fallen",
            "dragon_shake",
        ]),
        "TOWER's events (no other bits are read)",
    );
    l.f(
        0x64,
        Ty::U16,
        "heal_fountain",
        "the healing fountain's charge, written leaving level 1 (never read back)",
    );
    l.f(
        0x66,
        Ty::U16,
        "mana_fountain",
        "the mana fountain's charge (never read back)",
    );
    l.f(
        0x68,
        Ty::U16,
        "plasma_charge",
        "level 6's plasma charge 0..0x3ff (above 0 it shocks the hero)",
    );
    l.unused(0x6a, 2);
    for i in 0..4 {
        let d = if i < 3 {
            "2 strong, 1 weak, 0 off; each lever press steps 2, 1, 0"
        } else {
            "steps with every press; the crate KISTE2 blocks it"
        };
        l.f(
            0x6c + i,
            Ty::I8,
            format!("laser[{i}]"),
            format!("laser {}: {d}", i + 1),
        );
    }
    for i in 0..5 {
        l.f(
            0x70 + i,
            Ty::U8,
            format!("block_lower[{i}]"),
            format!(
                "1 = block SCHIEBE_{} was low when the ghost scene began (never read)",
                i + 1
            ),
        );
    }
    l.bytes(
        0x75,
        &[
            (
                "bully_stage",
                "0 not met, 1 crying (QUESTLOG_HELPBULLY), 2 helped",
            ),
            (
                "old_one_stage",
                "0 the first Old One scene not seen, 1 seen, 2 the second seen (he fights)",
            ),
        ],
    );
    l.l(0x77, Ty::U8, "qt_switch_mask", Label::Flags(&["qt1", "qt2", "qt3", "qt4", "qt5", "qt6", "qt7", "qt8"]), "switches QT1..8 on (0x52 opens CONFIDOOR8); rebuilt every second, so writing it does nothing");
    l.f(0x78, Ty::U8, "blocks_solved", "1 = all five blocks up");
    l.l(
        0x79,
        Ty::U8,
        "ritual",
        Label::Flags(&["ash", "belladonna", "black_pearl", "bit3", "spellbook_used"]),
        "the ritual's ingredients placed; spellbook_used: the ritual done (QUESTLOG_GOTOLABOR)",
    );
    l.f(
        0x7a,
        Ty::I8,
        "glasses_stage",
        "0 not started, 1 quest given (QUESTLOG_FINDGLASSES), 2 glasses returned",
    );
    l.f(0x7b, Ty::U8, "timers_used", "timer slots in use, 0..8");
    l.bytes(
        0x7c,
        &[
            (
                "beam_stage",
                "0 level 5 never entered, 1 the beam puzzle open, 2 solved (10000 experience)",
            ),
            ("lvl6_init", "1 = level 6 set up once"),
            ("lvl1_visited", "set on entering level 1 (never read)"),
            (
                "glasses_tm1talk",
                "1 = the hero's line on the glasses played",
            ),
        ],
    );
    l.unused(0x80, 1);
    l.bytes(
        0x81,
        &[
            (
                "shortcut_4",
                "1 = the warps between levels 1 and 4 armed, FOYERDOOR2 open",
            ),
            ("shortcut_5", "1 = the warps between levels 1 and 5 armed"),
            ("shortcut_6", "1 = the warps between levels 1 and 6 armed"),
            ("save_hint_shown", "1 = the dragon's save hint shown"),
            (
                "blocks_rewarded",
                "1 = the block puzzle's 10000 experience paid",
            ),
        ],
    );
    l.unused(0x86, 2);
    l.finish("TOWER", 0x88)
}

fn vulcano() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.shop(14, "SHACH_TRADERX_TRADE");
    l.l(
        0x46,
        Ty::I8,
        "shop_end",
        Label::ShopItem,
        "the shop's end marker (127)",
    );
    l.unused(0x47, 4);
    l.unused(0x4b, 0x3e);
    l.l(
        0x89,
        Ty::U8,
        "reflect_mask",
        Label::Flags(&["reflect_a", "reflect_b", "reflect_c", "reflect_d"]),
        "mirrors turned",
    );
    l.f(
        0x8a,
        Ty::I8,
        "ball_stage",
        "0 not started, 1..3 ball goals reached (raising PLATFORM3, 2, 1); 3 the riddle done",
    );
    l.bytes(0x8b, &[
        ("shachguard_done", "1 = seven Q_GAEN handed over (QUESTLOG_FIND7GAENS done)"),
        ("l6_arrival_seen", "1 = the arrival scene played"),
        ("l6_oldone_seen", "1 = the Old One's scene played (Talis hidden after it)"),
        ("platform4_state", "0 idle, 1 switch pulled, 2 PLATFORM4 moving"),
        ("master_talked", "1 = the master talked to (QUESTLOG_GET3COINS)"),
        ("prayer_stage", "0 not talked to, 2 the coin quest given"),
        ("fredo_stage", "0 none, 1 within a talk, 2 QUESTLOG_BRINGCRYSTAL, 3 Q_CRYST_BLUE delivered"),
        ("brother_stage", "0 not talked to, 1 talked to before the master, 2 evidence quest, 3 Q_DOKU handed over, Q_COIN_C received"),
    ]);
    l.unused(0x93, 1);
    l.bytes(
        0x94,
        &[
            ("coins_found", "0..3 coins brought to the master"),
            ("pinboard_next", "the pinboard's next note 0..3"),
            (
                "worker_stage",
                "0 not met, 1 needs a bomb, 3 bomb done (QUESTLOG_FIXPIPE), 4 pipe fixed",
            ),
            ("tntwall_blown", "1 = the TNT wall blown"),
            (
                "forkboy_talked",
                "0 = the fork boy's first line next, 1 = his second",
            ),
            ("trader1_scene", "2 = the first trader's guard scene played"),
            (
                "shovel_scene_done",
                "1 = the worker's bomb and shovel scenes done",
            ),
            ("master_moved", "1 met, 2 the master replaced"),
        ],
    );
    l.l(
        0x9c,
        Ty::U8,
        "rules_read",
        Label::Flags(&[
            "rule1", "rule2", "rule3", "rule4", "rule5", "rule6", "rule7", "rule8",
        ]),
        "the rules read",
    );
    l.bytes(0x9d, &[
        ("rules_count", "rules read (5 or more with prayer_stage 2 starts the arrest)"),
        ("arrested", "1 = the arrest scene started"),
        ("magnet_turned", "0 the gambler cheats, 1 the magnet turned: he can be beaten"),
        ("gambler_beaten", "1 = the gambler beaten, Q_COIN_B received"),
        ("zockerwoman_gone", "1 = her scene played, she is hidden"),
        ("pipe_fixed", "1 = the pipe fixed with Q_EIMERP"),
        ("steam_off", "1 = the steam off after the wall's blast (back to 0 in the repair scene; name unsure)"),
        ("jailguard_posted", "1 = the guard at the jail door"),
        ("barter_stage", "the H_* barter chain: 0 start, 1 H_MET to H_BUCH, 2 or 3 the book traded on, 4/5 and 7/8 the next trades, 9 RING and PILZCHEN for Q_BALLBLUE (done)"),
        ("barter_trader", "the trader of the current barter talk 1..4 (scratch)"),
        ("magnetfield_armed", "1 = the magnetic field armed again once"),
        ("stomp_noise", "the stamping press's noise on or off"),
        ("press_scene_seen", "1 = the press scene with Q_EIMER played"),
        ("grandpa1_done", "1 = grandpa 1's thanks played"),
        ("riddlelight_shown", "1 = the code shown"),
        ("lift_intro_seen", "0 = entering adds QUESTLOG_FINDTALIS and RUINSLOC"),
        ("gear_scene_seen", "1 = the gear's camera scene seen"),
        ("l2_visited", "1 = restore manabrunnen1_charge on entry"),
    ]);
    l.unused(0xaf, 1);
    l.f(
        0xb0,
        Ty::U16,
        "manabrunnen1_charge",
        "MANABRUNNEN_1's charge (saved leaving level 2, restored on entry)",
    );
    l.bytes(
        0xb2,
        &[
            ("door7_solved", "1 = the switch riddle solved"),
            ("in_riddle_zone", "1 while the hero is in the riddle's area"),
            (
                "switch3_state",
                "1 = switches 1..3 set, DOOR2 and DOOR3 open",
            ),
        ],
    );
    l.unused(0xb5, 3);
    for i in 0..3 {
        l.l(
            0xb8 + 4 * i,
            Ty::I32,
            format!("wheel[{i}]"),
            Label::Angle,
            format!("RAD{}'s turn (restored on entry)", i + 1),
        );
    }
    l.f(
        0xc4,
        Ty::I8,
        "moon_progress",
        "correct steps of the moon riddle's 16; 17 solved; a mistake resets it",
    );
    l.f(
        0xc5,
        Ty::U8,
        "wyrm_fight",
        "0 not started, 2 in the arena, 1 left it, 99 dead",
    );
    l.bytes(
        0xc6,
        &[
            ("wyrm_dead", "1 = the wyrm killed"),
            ("movingplatform1", "written, never read"),
            ("wyrm_emerged", "1 = the wyrm's emergence played"),
        ],
    );
    l.f(
        0xc9,
        Ty::I8,
        "symbols_lit",
        "0..12 symbols stepped on in order",
    );
    l.f(
        0xca,
        Ty::I8,
        "wrong_symbol",
        "0 none, else the wrong symbol's number + 1",
    );
    l.bytes(0xcb, &[
        ("map_stage", "0 not met, 1 failed, 2 all correct, 3 entering symbols, 4 symbols correct, 5 the ruins' place known"),
        ("starter_seen", "1 = the starter scene played"),
        ("final_intro_seen", "1 = the final sequence's intro seen"),
    ]);
    l.l(
        0xce,
        Ty::U8,
        "chain_mask",
        Label::Flags(&[
            "bit0",
            "schalter1_used",
            "chain3_lowered",
            "chain4_lowered",
            "chain5_lowered",
        ]),
        "SCHALTER1 used (DOOR1 open), chains lowered",
    );
    l.f(0xcf, Ty::U8, "savehint_shown", "1 = the save hint shown");
    l.f(
        0xd0,
        Ty::U32,
        "traderx_busy",
        "bit 0: the trader's talk or shop open (while it lasts)",
    );
    l.f(
        0xd4,
        Ty::U8,
        "shachguard_met",
        "0 the first talk next (adds the quest), 1 met",
    );
    l.unused(0xd5, 3);
    l.finish("VULCANO", 0xd8)
}

const RUINS_FLAGS: &[&str] = &[
    "symbols1_aligned",
    "symbols2_aligned",
    "symbols3_aligned",
    "keylocks_a_done",
    "keylocks_b_done",
    "keylocks_c_done",
    "parrot_code_a",
    "parrot_code_b",
    "parrot_code_c",
    "parrot_code_d",
    "parrot_code_e",
    "bit11",
    "bigape_killed",
    "magicbox_broken",
    "magickey_risen",
    "torches_solved",
    "mstones_destroyed",
    "weapon_trial",
    "defence_trial",
    "tool_trial",
    "magic_trial",
    "seal_scroll",
    "seal_shield",
    "seal_gear",
    "seal_sword",
    "intro_done",
    "berries_picked",
    "repair_clock_open",
    "wall_open_now",
    "wall_was_opened",
    "wall_closed_again",
    "boulder_destroyed",
];

fn ruins() -> Vec<WField> {
    let mut l = L(Vec::new());
    l.timers(5, "0", "; ids per level (trial strip, torch check, platform steps, the bomb's ticking, the fish, the parrot, the lever, the switches' expiry)");
    for (i, n) in ["weapon", "spell", "tool", "defence"].iter().enumerate() {
        l.f(0x3c + i, Ty::U8, format!("trial_sel[{i}]"), format!("the hero's selected {n} while a trial took the equipment (restored on the trial's exit)"));
    }
    for i in 0..4 {
        l.f(
            0x40 + 4 * i,
            Ty::U32,
            format!("trial_sel_rest[{i}]"),
            "the rest of that copy (never read)",
        );
    }
    l.f(
        0x50,
        Ty::I32,
        "lifepool_level",
        "the life pool's fill, HP << 12 (150 HP full)",
    );
    l.f(0x54, Ty::I32, "lifepool_max", "its full fill");
    l.f(0x58, Ty::I32, "lifepool_pending", "healing still to drain");
    l.f(0x5c, Ty::U8, "lifepool_refilling", "1 = refilling");
    l.f(0x5d, Ty::U8, "lifepool_ready", "1 = usable");
    l.unused(0x5e, 2);
    for t in 0..4 {
        let b = 0x60 + 24 * t;
        let p = format!("turtle[{t}]");
        l.f(b, Ty::U32, format!("{p}.pos"), format!("TURTLE{}: position, packed (10 bits each of x, y, z in 512 units); restored when turtle 1's is not 0", t + 1));
        l.f(
            b + 4,
            Ty::U32,
            format!("{p}.waypoint_a"),
            "a waypoint, packed",
        );
        l.f(
            b + 8,
            Ty::U32,
            format!("{p}.waypoint_b"),
            "another waypoint, packed",
        );
        l.f(
            b + 0xc,
            Ty::U32,
            format!("{p}.swim"),
            "two words of its swimming state (meaning unsure)",
        );
        l.f(
            b + 0x10,
            Ty::U32,
            format!("{p}.field_a"),
            "an object field (meaning unsure)",
        );
        l.f(
            b + 0x14,
            Ty::U32,
            format!("{p}.field_b"),
            "an object field (meaning unsure)",
        );
    }
    l.f(
        0xc0,
        Ty::I32,
        "fish_x",
        "a thrown R_FISH: x (not 0: it is there again on entry)",
    );
    l.f(0xc4, Ty::I32, "fish_y", "the fish: y");
    l.f(0xc8, Ty::I32, "fish_z", "the fish: z");
    l.l(
        0xcc,
        Ty::U32,
        "flags",
        Label::Flags(RUINS_FLAGS),
        "RUINS's events (one parrot code at a time)",
    );
    l.f(
        0xd0,
        Ty::U32,
        "weapon_trial_waves",
        "the weapon trial's enemies: an alive bit each that moves to a killed bit",
    );
    l.f(
        0xd4,
        Ty::U32,
        "tool_trial_holes",
        "the tool trial's holes filled (bit column + 4 (row - 1)); 0xffff solved",
    );
    l.f(0xd8, Ty::U8, "timers_used", "timer slots in use, 0..5");
    for (i, n) in ["1a", "1b", "2a", "2b", "3a", "3b"].iter().enumerate() {
        l.f(
            0xd9 + i,
            Ty::U8,
            format!("symbol_{n}"),
            "a stone's rotation 0..2 (solved: 1a 0 and 1b 0, 2a 0 and 2b 0, 3a 2 and 3b 0)",
        );
    }
    l.f(
        0xdf,
        Ty::I8,
        "mpl_next",
        "the next of the platforms MPL_1A..1E to step on",
    );
    l.f(
        0xe0,
        Ty::U8,
        "mpl_last",
        "the last correct platform (0xff none)",
    );
    l.l(
        0xe1,
        Ty::U8,
        "keylocks_a",
        Label::Flags(&["a1", "a2", "a3", "a4", "a5", "a6"]),
        "KEYLOCK_A1..6 filled",
    );
    l.l(
        0xe2,
        Ty::U8,
        "keylocks_b",
        Label::Flags(&["b1", "b2", "b3", "b4"]),
        "KEYLOCK_B1..4 filled",
    );
    l.l(
        0xe3,
        Ty::U8,
        "keylocks_c",
        Label::Flags(&["c1", "c2"]),
        "KEYLOCK_C1..2 filled",
    );
    l.f(
        0xe4,
        Ty::U8,
        "parrot_seq",
        "codes C, D, E taught in order: 1, 2, 3 (3 opens DOOR3, QUESTLOG_MASTERDEFENCE)",
    );
    l.f(
        0xe5,
        Ty::U8,
        "oldone_stage",
        "0 not started, 10/20/30/40 waves 1..4 beaten, 50 the finale done",
    );
    l.l(
        0xe6,
        Ty::U8,
        "mana_crystals",
        Label::Flags(CRYSTALS5),
        "crystals in the machine (0xff fired); reset on entering level 1",
    );
    l.unused(0xe7, 1);
    l.f(
        0xe8,
        Ty::U16,
        "switchd",
        "SWITCHD01..05 pressed in a timed run (bits 0..4); reset on entry",
    );
    l.f(0xea, Ty::U16, "torches", "torches lit (bits 0..11; 0x969 solved); rebuilt from the objects, so writing it does nothing");
    l.bytes(
        0xec,
        &[
            (
                "weapon_master_talk",
                "0 never, 1 trial started, 2 talked to after passing",
            ),
            (
                "magic_master_talk",
                "0 never, 1 trial started, 2 talked to after passing",
            ),
            (
                "tool_master_talk",
                "0 never, 1 trial started, 2 talked to after passing",
            ),
            (
                "defence_master_talk",
                "0 never, 1 trial started, 2 talked to after passing",
            ),
            (
                "boulder_started",
                "1 = the boulder chase reached the first fight",
            ),
            ("temple_chief_talk", "1 = the repair quest given"),
        ],
    );
    l.l(
        0xf2,
        Ty::U8,
        "fires",
        Label::Flags(&["fire1", "fire2"]),
        "fires lit (both at the start)",
    );
    l.f(
        0xf3,
        Ty::U8,
        "dagomar_done",
        "1 = the final dialogue played",
    );
    l.f(
        0xf4,
        Ty::U8,
        "needpearl_said",
        "1 = the hero's line about the power pearl said",
    );
    l.unused(0xf5, 3);
    l.finish("RUINS", 0xf8)
}

const CRYSTALS5: &[&str] = &["green", "yellow", "blue", "red", "white"];
