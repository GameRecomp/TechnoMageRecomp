// SPDX-License-Identifier: MIT
//! The controls: a gamepad as the PlayStation version's pad, and the game's "Configure input
//! devices" page with three tabs (keyboard and mouse, gamepad, touch).
//!
//! The PC version reads its keys, mouse and DirectInput joystick into the PlayStation pad's
//! buttons (RESEARCH.md, "The controls"): L2 0x01, R2 0x02, L1 0x04, R1 0x08, triangle 0x10,
//! circle 0x20, cross 0x40, square 0x80, select 0x100, L3 0x200, R3 0x400, start 0x800, the
//! directions 0x1000 (up), 0x2000 (right), 0x4000 (down) and 0x8000 (left). Its joystick's
//! buttons go through a table the game builds from the page's "Joy" bindings (none by default);
//! ours puts the PlayStation version's controller set-up in front of it. The HLE's joystick
//! reports the gamepads' buttons in the pad's order (`rc_platform::pad`), so a button means what
//! it meant on the PlayStation.

use rc_runtime::cpu::ESP;
use rc_runtime::dispatch::{call_guest, call_original};
use rc_runtime::mem::Mem;
use rc_runtime::Cpu;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

/// The input page: run and drawn once per frame while it is open.
pub const CONFIG_PAGE: u32 = 0x0040_1140;
/// Sets the joystick's button table (its argument); returns the one before.
pub const SET_PAD_TABLE: u32 = 0x0053_8790;
/// The joystick's button table in use: entries of a button, a byte unused and the pad bits
/// (`u16`), up to a button 0xff.
const PAD_TABLE: u32 = 0x0080_88e0;
/// The pad's buttons pressed in this frame (with the game's repeat), as the page reads them.
const PRESSED: u32 = 0x00ee_93a4;
/// Whether the page waits for a key to bind (1 or 2: the column).
const CAPTURING: u32 = 0x0059_d72c;
/// The page's entries (0x50 bytes each, up to a null key): the label at +4, the bindings' names
/// at +0xa and +0x2c.
const ENTRIES: u32 = 0x0055_e8f8;
const ENTRY_SIZE: u32 = 0x50;
const ENTRY_COUNT: u32 = 18;
/// The page's font and line height.
const PAGE_FONT: u32 = 0x0059_d6d0;
const ROW_STEP: u32 = 0x0059_d6dc;
/// The pad's buttons held (the page clicks its switches while square or cross is).
const HELD: u32 = 0x0059_cb10;
/// The mouse's position in the game's pixels.
const MOUSE_X: u32 = 0x0059_cb14;
const MOUSE_Y: u32 = 0x0059_cb16;
const SCREEN_W: u32 = 0x00f5_b320;
const LANGUAGE: u32 = 0x0059_de18;
const DRAW_TEXT: u32 = 0x0040_10d7;

// The pad's bits, as the game's actions have them.
const L2: u16 = 0x01;
const R2: u16 = 0x02;
const L1: u16 = 0x04;
const R1: u16 = 0x08;
const TRIANGLE: u16 = 0x10;
const CIRCLE: u16 = 0x20;
const CROSS: u16 = 0x40;
const SQUARE: u16 = 0x80;
const SELECT: u16 = 0x100;
const L3: u16 = 0x200;
const R3: u16 = 0x400;
const START: u16 = 0x800;
const RIGHT: u32 = 0x2000;
/// The page's "walk to the pointer" entry (up and down at once: no pad button has it).
const MOUSE_WALK: u16 = 0x5000;
const LEFT: u32 = 0x8000;

/// The PlayStation version's controller set-ups (its options' "Set-up A" to "D"): for each of
/// the gamepad's buttons in `rc_platform::pad`'s order (square, triangle, cross, circle, L2,
/// L1, R2, R1, select, start, L3, R3), the pad bit it gives. Set-up A is the default the
/// manual shows: triangle a spell, circle a tool, cross a jump, square the action, L2 and R2
/// the camera, L1 the quest log, R1 the map. B swaps triangle with circle and cross with
/// square, C swaps L2 with R2 (the camera turns the other way), D does both (the PlayStation
/// executable's table at 0x80097a70, RESEARCH.md).
const SETUPS: [[u16; 12]; 4] = [
    [
        SQUARE, TRIANGLE, CROSS, CIRCLE, L2, L1, R2, R1, SELECT, START, L3, R3,
    ],
    [
        CROSS, CIRCLE, SQUARE, TRIANGLE, L2, L1, R2, R1, SELECT, START, L3, R3,
    ],
    [
        SQUARE, TRIANGLE, CROSS, CIRCLE, R2, L1, L2, R1, SELECT, START, L3, R3,
    ],
    [
        CROSS, CIRCLE, SQUARE, TRIANGLE, R2, L1, L2, R1, SELECT, START, L3, R3,
    ],
];

/// The set-up chosen (0 to 3).
static SETUP: AtomicU8 = AtomicU8::new(0);

/// The game's own joystick table (from the page's "Joy" bindings), behind ours.
static GAME_TABLE: AtomicU32 = AtomicU32::new(0);

/// The setting's name and value in `TechnoMageRecomp.cfg`.
pub const SETTING: &str = "gamepad_setup";

pub fn setting() -> String {
    char::from(b'A' + SETUP.load(Ordering::Relaxed)).to_string()
}

pub fn set_setting(v: &str) {
    if let Some(i) = v.bytes().next().and_then(|b| b.checked_sub(b'A')) {
        if usize::from(i) < SETUPS.len() {
            SETUP.store(i, Ordering::Relaxed);
        }
    }
}

// --- The joystick's table -----------------------------------------------------------------------

/// Our table, made on first use: the set-up's buttons, then the game's.
fn table(m: Mem) -> u32 {
    static AT: Mutex<u32> = Mutex::new(0);
    const CAP: u32 = 64;
    let mut at = AT.lock().expect("pad table");
    if *at == 0 {
        *at = rc_runtime::vm::heap_alloc(m, 4 * CAP, true);
    }
    let mut p = *at;
    let setup = SETUPS[usize::from(SETUP.load(Ordering::Relaxed))];
    for (button, bits) in setup.iter().enumerate() {
        m.st8(p, button as u8);
        m.st8(p + 1, 0);
        m.st16(p + 2, *bits);
        p += 4;
    }
    // The touch screen's on-screen buttons: their actions, whatever the set-up.
    for (k, (_, bits)) in TOUCH_BUTTONS.iter().enumerate() {
        m.st8(p, (rc_hle::dinput::TOUCH_FIRST + k as u32) as u8);
        m.st8(p + 1, 0);
        m.st16(p + 2, *bits);
        p += 4;
    }
    let mut g = GAME_TABLE.load(Ordering::Relaxed);
    while g != 0 && m.ld8(g) != 0xff && p < *at + 4 * (CAP - 1) {
        m.st32(p, m.ld32(g));
        p += 4;
        g += 4;
    }
    m.st8(p, 0xff);
    *at
}

/// The game sets the joystick's table (after reading the page's bindings): ours in its place.
pub fn set_pad_table(c: &mut Cpu) {
    let m = c.mem();
    let sp = c.r[ESP];
    GAME_TABLE.store(m.ld32(sp + 4), Ordering::Relaxed);
    m.st32(sp + 4, table(m));
    call_original(c, SET_PAD_TABLE);
}

/// The set-up changed: the joystick's table again.
fn apply(m: Mem) {
    if GAME_TABLE.load(Ordering::Relaxed) != 0 {
        m.st32(PAD_TABLE, table(m));
    }
}

// --- The dialogues' key pictures ---------------------------------------------------------------

/// The game's pictures of the actions' keys (its texts' `§`, `=`, `~` and the like): ten sprites
/// made from the input page's bindings (`0x40105f`), each 12 bytes with its texture's u, v,
/// width and height at +7 … +10.
const KEY_SPRITES: u32 = 0x014d_fa60;
/// Their actions (pad bits): action, spell, tool, jump, camera left and right, quest log, map,
/// none (a fixed picture), rucksack.
const SPRITE_ACTIONS: [u16; 10] = [SQUARE, TRIANGLE, CIRCLE, CROSS, L2, R2, L1, R1, 0, SELECT];

/// The key pictures of this frame: where (screen surface pixels), how large, whose action.
static KEY_PICTURES: Mutex<Vec<KeyPicture>> = Mutex::new(Vec::new());

/// A key picture's place (x, y, width, height) and its action.
type KeyPicture = (i32, i32, u32, u32, u16);

/// Before the frame's ordering table is drawn (its last node): its interface's sprites that
/// are an action's key picture, noted (moved by the widescreen margin `ox`, as they will be).
pub fn note_key_pictures(m: Mem, last: u32, ox: i32) {
    if !rc_hle::dinput::pad_in_use() {
        return;
    }
    let sprites: Vec<(u32, u16)> = SPRITE_ACTIONS
        .iter()
        .enumerate()
        .filter(|(_, a)| **a != 0)
        .filter_map(|(i, &a)| {
            let sp = m.ld32(KEY_SPRITES + 4 * i as u32);
            (sp != 0).then(|| (m.ld32(sp + 7), a))
        })
        .collect();
    let first = last - 0x3ff * 16;
    let mut node = m.ld32(last);
    let mut bucket = 0x3ff;
    let mut guard = 0;
    let mut found = KEY_PICTURES.lock().expect("key pictures");
    while node != 0 && guard < 1 << 20 {
        if (first..=last).contains(&node) {
            bucket = (node - first) / 16;
        } else if bucket < crate::patches::WORLD_BUCKET && m.ld8(node + 0xf) & 0xfc == 0x64 {
            // A sprite: u, v at +0x14, width and height at +0x18 (as u16), x and y at +0x10.
            let key = u32::from(m.ld8(node + 0x14))
                | u32::from(m.ld8(node + 0x15)) << 8
                | u32::from(m.ld8(node + 0x18)) << 16
                | u32::from(m.ld8(node + 0x1a)) << 24;
            if let Some(&(_, action)) = sprites.iter().find(|(k, _)| *k == key) {
                if found.len() < 64 {
                    found.push((
                        i32::from(m.ld16(node + 0x10) as i16) + ox,
                        i32::from(m.ld16(node + 0x12) as i16),
                        u32::from(m.ld16(node + 0x18)),
                        u32::from(m.ld16(node + 0x1a)),
                        action,
                    ));
                }
            }
        }
        node = m.ld32(node);
        guard += 1;
    }
}

/// Over the frame, while the gamepad is in use: in each key picture's place the gamepad's
/// button that does its action now (the set-up's), drawn as its maker does.
pub fn overlay(_m: Mem, frame: &mut [u8], w: u32, h: u32, scale: f32) {
    let shown = std::mem::take(&mut *TOUCH_SHOWN.lock().expect("touch buttons"));
    let held = rc_platform::touch::held();
    for (k, [x, y, bw, bh]) in shown.into_iter().enumerate() {
        let place = (x * scale, y * scale, bw * scale, bh * scale);
        let pressed = held & (1 << k) != 0;
        match TOUCH_BUTTONS[k].0 {
            Touch::Ours(g) => crate::icons::draw_touch(frame, w, h, place, g, pressed),
            Touch::Game(_) => crate::icons::draw_rim(frame, w, h, place, pressed),
        }
    }
    let found = std::mem::take(&mut *KEY_PICTURES.lock().expect("key pictures"));
    if !rc_hle::dinput::pad_in_use() {
        return;
    }
    let setup = SETUPS[usize::from(SETUP.load(Ordering::Relaxed))];
    let style = rc_platform::pad::style();
    for (x, y, pw, ph, action) in found {
        let Some(button) = setup.iter().position(|&b| b == action) else {
            continue;
        };
        let place = (
            x as f32 * scale,
            y as f32 * scale,
            pw as f32 * scale,
            ph as f32 * scale,
        );
        crate::icons::draw(frame, w, h, place, button, style);
    }
}

// --- The touch screen's on-screen buttons -----------------------------------------------------

/// What an on-screen button shows: a picture of the loaded ones (by name) or one of ours.
#[derive(Clone, Copy)]
enum Touch {
    Game(&'static str),
    Ours(crate::icons::Glyph),
}

/// The on-screen buttons and their actions (pad bits): along the bottom on the right the camera
/// (left, right), the tool, the spell and the jump; at the top on the right the quest log, the
/// map and the rucksack. Pointing, the action and walking stay the fingers' (as the mouse).
const TOUCH_BUTTONS: [(Touch, u16); 8] = {
    use crate::icons::Glyph;
    [
        (Touch::Ours(Glyph::CameraLeft), L2),
        (Touch::Ours(Glyph::CameraRight), R2),
        (Touch::Ours(Glyph::Tool), CIRCLE),
        (Touch::Ours(Glyph::Spell), TRIANGLE),
        (Touch::Ours(Glyph::Jump), CROSS),
        (Touch::Game("ICON_QUESTLOG"), L1),
        (Touch::Game("ICON_MAP"), R1),
        (Touch::Ours(Glyph::Rucksack), SELECT),
    ]
};

/// The buttons as drawn this frame (the game's screen, margins included), for the overlay.
static TOUCH_SHOWN: Mutex<Vec<[f32; 4]>> = Mutex::new(Vec::new());

/// The game's state while it is played (no menu, map, rucksack or dialogue open).
const PLAYING_STATE: u32 = 0x800;
const PAUSE_STATE: u32 = 0x0056_3c84;
const MENU_FLAGS: u32 = 0x0059_de34;

/// Once per frame, before its ordering table is drawn: while touch is used and the game is
/// played, the on-screen buttons (their places to the platform, the game's pictures into the
/// table, ours to the overlay); none otherwise.
pub fn touch_bar(c: &mut Cpu) {
    let m = c.mem();
    let playing = m.ld32(PAUSE_STATE) == PLAYING_STATE && m.ld32(MENU_FLAGS) == 0;
    if !(rc_platform::touch::in_use() && playing) {
        rc_platform::touch::set_buttons(Vec::new());
        TOUCH_SHOWN.lock().expect("touch buttons").clear();
        return;
    }
    const SIZE: i32 = 34;
    const GAP: i32 = 6;
    const EDGE: i32 = 8;
    let w = m.ld32(SCREEN_W) as i32;
    // At the right edge of a wider screen (the interface's primitives are moved by one margin
    // into the middle; ours by one more).
    let ox = rc_hle::ddraw::margin() as i32;
    let mut places = Vec::new();
    for (k, (what, _)) in TOUCH_BUTTONS.iter().enumerate() {
        // Five along the bottom, three at the top, from the right.
        let (row, col, n) = if k < 5 {
            (1, k as i32, 5)
        } else {
            (0, k as i32 - 5, 3)
        };
        let x = w - EDGE - (n - col) * SIZE - (n - 1 - col) * GAP;
        let y = if row == 1 { 480 - EDGE - SIZE } else { EDGE };
        if let Touch::Game(name) = what {
            if let Some(rec) = sprite_record(c, name) {
                let (sw, sh) = (i32::from(m.ld8(rec + 9)), i32::from(m.ld8(rec + 10)));
                draw_record(m, rec, x + ox + (SIZE - sw) / 2, y + (SIZE - sh) / 2, 2);
            }
        }
        places.push([(x + 2 * ox) as f32, y as f32, SIZE as f32, SIZE as f32]);
    }
    rc_platform::touch::set_buttons(places.clone());
    *TOUCH_SHOWN.lock().expect("touch buttons") = places;
}

// --- The game's sprites, drawn by us ---------------------------------------------------------------

/// A sprite of the loaded pictures by its name (12 bytes: four 0xff, its texture page and mode,
/// its palette, u, v, width, height), or none.
const SPRITE_BY_NAME: u32 = 0x0040_1465;
/// The next free primitive and the ordering tables (two, 0x400 buckets of 16 bytes each).
const PRIM_NEXT: u32 = 0x0059_de10;
const OT_BUFFER: u32 = 0x0059_cb1c;
const OT_TABLES: u32 = 0x0079_5840;

/// The loaded sprite `name`'s record, if there is one.
fn sprite_record(c: &mut Cpu, name: &str) -> Option<u32> {
    let m = c.mem();
    let mut s = Strings::new(m);
    let n = s.put(m, name);
    let rec = call_guest(c, SPRITE_BY_NAME, &[n]);
    (rec != 0 && m.ld32(rec) == u32::MAX).then_some(rec)
}

/// Draw the sprite record `rec` at `(x, y)` (the game's pixels) into the ordering table's
/// `bucket`, as the game draws its sprites (`0x445830`): a mode primitive with its texture page,
/// then the sprite.
fn draw_record(m: Mem, rec: u32, x: i32, y: i32, bucket: u32) {
    let (w, h) = (m.ld8(rec + 9), m.ld8(rec + 10));
    let head = OT_TABLES + ((m.ld32(OT_BUFFER) << 10) + bucket) * 16;
    let p = m.ld32(PRIM_NEXT);
    // The sprite (variable size, textured, semi-transparent off).
    m.st8(p + 7, 0);
    m.st8(p + 0xf, 0x65);
    m.st16(p + 0x10, x as u16);
    m.st16(p + 0x12, y as u16);
    m.st8(p + 0x14, m.ld8(rec + 7));
    m.st8(p + 0x15, m.ld8(rec + 8));
    m.st16(p + 0x16, u16::from(m.ld8(rec + 6)) << 6);
    m.st16(p + 0x18, u16::from(w));
    m.st16(p + 0x1a, u16::from(h));
    m.st32(p, m.ld32(head));
    m.st32(head, p);
    // Before it in the bucket: the texture page and mode.
    let q = p + 0x1c;
    m.st8(q + 7, 0);
    m.st8(q + 0xf, 0x80);
    let mode = ((u16::from(m.ld8(rec + 5)) & 0xfff8) | u16::from(m.ld8(rec + 4))) << 9 | 0xa0;
    m.st16(q + 0x10, mode);
    m.st32(q, m.ld32(head));
    m.st32(head, q);
    m.st32(PRIM_NEXT, q + 0x14);
}

// --- The page's tabs ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Keys,
    Gamepad,
    Touch,
}

const TABS: [Tab; 3] = [Tab::Keys, Tab::Gamepad, Tab::Touch];

static TAB: AtomicU8 = AtomicU8::new(0);

fn tab() -> Tab {
    TABS[usize::from(TAB.load(Ordering::Relaxed)) % TABS.len()]
}

/// Our texts in the game's six languages (German, English, French, Italian, Spanish, Dutch).
type Text = [&'static str; 6];

// The game's font has no slash: "+" joins.
const T_KEYS: Text = [
    "Tastatur + Maus",
    "Keyboard + mouse",
    "Clavier + souris",
    "Tastiera + mouse",
    "Teclado + ratón",
    "Toetsenbord + muis",
];
const T_GAMEPAD: Text = [
    "Gamepad", "Gamepad", "Manette", "Gamepad", "Mando", "Gamepad",
];
const T_TOUCH: Text = ["Touch", "Touch", "Tactile", "Touch", "Táctil", "Aanraken"];
const T_SETUP: Text = [
    "Belegung",
    "Set-up",
    "Configuration",
    "Configurazione",
    "Configuración",
    "Indeling",
];
const T_DPAD: Text = [
    "Steuerkreuz",
    "D-pad",
    "Croix",
    "Croce direzionale",
    "Cruceta",
    "Kruistoets",
];
const T_STICK: Text = [
    "Linker Stick",
    "Left stick",
    "Stick gauche",
    "Levetta sinistra",
    "Stick izquierdo",
    "Linker stick",
];
const T_MENU: Text = [
    "Spielmenü",
    "Game menu",
    "Menu du jeu",
    "Menu di gioco",
    "Menú del juego",
    "Spelmenu",
];
const T_FINGER: Text = [
    "Ein Finger",
    "One finger",
    "Un doigt",
    "Un dito",
    "Un dedo",
    "Eén vinger",
];
const T_FINGER_DOES: Text = [
    "Zeiger, linker Mausknopf",
    "Pointer, left button",
    "Pointeur, bouton gauche",
    "Puntatore, tasto sinistro",
    "Puntero, botón izquierdo",
    "Aanwijzer, linkerknop",
];
const T_SECOND: Text = [
    "Zweiter Finger",
    "Second finger",
    "Deuxième doigt",
    "Secondo dito",
    "Segundo dedo",
    "Tweede vinger",
];
const T_SECOND_DOES: Text = [
    "Rechter Mausknopf",
    "Right button",
    "Bouton droit",
    "Tasto destro",
    "Botón derecho",
    "Rechterknop",
];
const T_BACK: Text = [
    "Zurück-Taste",
    "Back button",
    "Bouton Retour",
    "Tasto Indietro",
    "Botón Atrás",
    "Terugknop",
];
const T_BOTTOM: Text = [
    "Knöpfe unten rechts",
    "Buttons bottom right",
    "Boutons en bas",
    "Tasti in basso",
    "Botones abajo",
    "Knoppen onder",
];
const T_BOTTOM_DOES: Text = [
    "Kamera, Tool, Zauber, Sprung",
    "Camera, tool, spell, jump",
    "Caméra, outil, sort, saut",
    "Visuale, attrezzo, magia, salto",
    "Cámara, herramienta, hechizo, salto",
    "Camera, gereedschap, spreuk, sprong",
];
const T_TOP: Text = [
    "Knöpfe oben rechts",
    "Buttons top right",
    "Boutons en haut",
    "Tasti in alto",
    "Botones arriba",
    "Knoppen boven",
];
const T_TOP_DOES: Text = [
    "Questlog, Karte, Rucksack",
    "Quest log, map, rucksack",
    "Journal, carte, sac",
    "Diario, mappa, zaino",
    "Diario, mapa, mochila",
    "Logboek, kaart, rugzak",
];
const T_SQUARE: Text = [
    "Quadrat", "Square", "Carré", "Quadrato", "Cuadrado", "Vierkant",
];
const T_TRIANGLE: Text = [
    "Dreieck",
    "Triangle",
    "Triangle",
    "Triangolo",
    "Triángulo",
    "Driehoek",
];
const T_CROSS: Text = ["Kreuz", "Cross", "Croix", "Croce", "Equis", "Kruis"];
const T_CIRCLE: Text = ["Kreis", "Circle", "Rond", "Cerchio", "Círculo", "Cirkel"];

/// The buttons as the PlayStation names them, in `rc_platform::pad`'s order.
const PS_NAMES: [Text; 12] = [
    T_SQUARE,
    T_TRIANGLE,
    T_CROSS,
    T_CIRCLE,
    ["L2"; 6],
    ["L1"; 6],
    ["R2"; 6],
    ["R1"; 6],
    ["Select"; 6],
    ["Start"; 6],
    ["L3"; 6],
    ["R3"; 6],
];
/// As Xbox and Nintendo controllers name them (by place: the button below is Xbox's A and
/// Nintendo's B).
const XBOX_NAMES: [&str; 12] = [
    "X", "Y", "A", "B", "LT", "LB", "RT", "RB", "View", "Menu", "LS", "RS",
];
const NINTENDO_NAMES: [&str; 12] = [
    "Y", "X", "B", "A", "ZL", "L", "ZR", "R", "-", "+", "LS", "RS",
];

/// Button `i`'s names: as the gamepad in use has it, then as another maker (the PlayStation's,
/// or Xbox's for a PlayStation pad).
fn button_names(i: usize, l: usize) -> (String, String) {
    use rc_platform::pad::Style;
    let ps = PS_NAMES[i][l].to_string();
    match rc_platform::pad::style() {
        Style::PlayStation => (ps, XBOX_NAMES[i].to_string()),
        Style::Xbox => (XBOX_NAMES[i].to_string(), ps),
        Style::Nintendo => (NINTENDO_NAMES[i].to_string(), ps),
    }
}

fn lang(m: Mem) -> usize {
    (m.ld32(LANGUAGE) as usize).min(5)
}

/// Guest strings of ours for one frame's texts.
struct Strings {
    base: u32,
    next: u32,
}

const STRING_SIZE: u32 = 64;
const STRINGS: u32 = 48;

impl Strings {
    fn new(m: Mem) -> Strings {
        static AT: Mutex<u32> = Mutex::new(0);
        let mut at = AT.lock().expect("strings");
        if *at == 0 {
            *at = rc_runtime::vm::heap_alloc(m, STRING_SIZE * STRINGS, true);
        }
        Strings { base: *at, next: 0 }
    }

    /// `s` as a guest string (Windows-1252, as the game's texts are).
    fn put(&mut self, m: Mem, s: &str) -> u32 {
        let at = self.base + STRING_SIZE * (self.next % STRINGS);
        self.next += 1;
        let bytes: Vec<u8> = s
            .chars()
            .map(|ch| u8::try_from(u32::from(ch)).unwrap_or(b'?'))
            .take(STRING_SIZE as usize - 1)
            .collect();
        m.write(at, &bytes);
        m.st8(at + bytes.len() as u32, 0);
        at
    }
}

/// Colours as the page has them: labels white, the first column yellow, the second cyan.
const WHITE: [u32; 3] = [0x80, 0x80, 0x80];
const YELLOW: [u32; 3] = [0x80, 0x80, 0x40];
const CYAN: [u32; 3] = [0x40, 0x80, 0x80];
const GREY: [u32; 3] = [0x48, 0x48, 0x48];

fn draw(c: &mut Cpu, text: u32, x: i32, y: i32, [r, g, b]: [u32; 3]) {
    let font = c.mem().ld32(PAGE_FONT);
    call_guest(
        c,
        DRAW_TEXT,
        &[font, text, x as u32, y as u32, 0x14, r, g, b, 2, 0],
    );
}

/// The page's columns (thousandths of the width: label, first and second binding) and its
/// first line, as the page places them.
fn columns(m: Mem) -> [i32; 3] {
    let w = m.ld32(SCREEN_W) as i32;
    [w * 50 / 1000, w * 300 / 1000, w * 630 / 1000]
}

fn line_y(m: Mem, row: u32) -> i32 {
    let w = m.ld32(SCREEN_W) as i32;
    w * 30 / 1000 + m.ld32(ROW_STEP) as i32 * row as i32
}

/// Where the tabs are: their x and the line (beside the page's two switches, at the bottom).
fn tab_places(m: Mem) -> ([i32; 3], i32) {
    let w = m.ld32(SCREEN_W) as i32;
    (
        [w * 420 / 1000, w * 680 / 1000, w * 840 / 1000],
        line_y(m, 20),
    )
}

/// Where the set-up's line is: its label, the minus, the letter, the plus, the line.
fn setup_places(m: Mem) -> (i32, i32, i32, i32, i32) {
    let w = m.ld32(SCREEN_W) as i32;
    let (xs, y) = tab_places(m);
    let y = y + m.ld32(ROW_STEP) as i32 + 4;
    (xs[0], w * 700 / 1000, w * 740 / 1000, w * 775 / 1000, y)
}

/// The width a tab's label is taken to have, for a click.
fn tab_width(m: Mem, i: usize) -> i32 {
    let (xs, _) = tab_places(m);
    let end = xs
        .get(i + 1)
        .copied()
        .unwrap_or(m.ld32(SCREEN_W) as i32 * 960 / 1000);
    end - xs[i]
}

/// The tabs' labels, the chosen one bright.
fn draw_tabs(c: &mut Cpu, s: &mut Strings) {
    let m = c.mem();
    let l = lang(m);
    let (xs, y) = tab_places(m);
    for (i, t) in [T_KEYS, T_GAMEPAD, T_TOUCH].iter().enumerate() {
        let colour = if TABS[i] == tab() { YELLOW } else { GREY };
        let text = s.put(m, t[l]);
        draw(c, text, xs[i], y, colour);
    }
}

/// The page's entries' texts empty while it runs (the other tabs draw their own), put back
/// after: the labels' pointers and the bindings' names.
fn hide_entries(m: Mem, empty: u32) -> Vec<(u32, u32)> {
    let mut saved = Vec::new();
    for i in 0..ENTRY_COUNT {
        let e = ENTRIES + ENTRY_SIZE * i;
        if m.ld32(e) == 0 {
            break;
        }
        saved.push((e + 4, m.ld32(e + 4)));
        m.st32(e + 4, empty);
        for name in [e + 0xa, e + 0x2c] {
            saved.push((name, m.ld32(name)));
            m.st8(name, 0);
        }
    }
    saved
}

/// The gamepad's lines in the page's rows: each action's button in the set-up (the
/// PlayStation's name, then the Xbox controller's), the directions, the game menu; the set-up
/// below.
fn draw_gamepad(c: &mut Cpu, s: &mut Strings, labels: &[u32]) {
    let m = c.mem();
    let l = lang(m);
    let [lx, x1, x2] = columns(m);
    let setup = SETUPS[usize::from(SETUP.load(Ordering::Relaxed))];
    for row in 0..ENTRY_COUNT {
        let e = ENTRIES + ENTRY_SIZE * row;
        if m.ld32(e) == 0 {
            break;
        }
        let bits = m.ld16(e + 8);
        let y = line_y(m, row);
        let label = labels.get(row as usize).copied().unwrap_or(0);
        // The directions: the directional buttons and the left stick.
        let (first, second) = if matches!(bits, 0x1000 | 0x2000 | 0x4000 | 0x8000) {
            (T_DPAD[l].to_string(), T_STICK[l].to_string())
        } else if bits == MOUSE_WALK {
            // Walking to the pointer (the mouse's): the game menu's button in its place.
            let i = setup.iter().position(|&b| b == START).unwrap_or(9);
            let menu = s.put(m, T_MENU[l]);
            draw(c, menu, lx, y, WHITE);
            let (a, b) = button_names(i, l);
            let (a, b) = (s.put(m, &a), s.put(m, &b));
            draw(c, a, x1, y, YELLOW);
            draw(c, b, x2, y, CYAN);
            continue;
        } else if bits != 0 {
            match setup.iter().position(|&b| b == bits) {
                Some(i) => button_names(i, l),
                None => continue,
            }
        } else {
            continue;
        };
        if label != 0 {
            draw(c, label, lx, y, WHITE);
        }
        let (a, b) = (s.put(m, &first), s.put(m, &second));
        draw(c, a, x1, y, YELLOW);
        draw(c, b, x2, y, CYAN);
    }
    // The set-up below the tabs: "Set-up:  - B +" (the font has no arrows); left and right, or
    // a click on the minus or the plus, change it.
    let (label_x, minus_x, letter_x, plus_x, y) = setup_places(m);
    let label = s.put(m, &format!("{}:", T_SETUP[l]));
    draw(c, label, label_x, y, WHITE);
    let letter = char::from(b'A' + SETUP.load(Ordering::Relaxed)).to_string();
    for (t, x, colour) in [
        ("-", minus_x, CYAN),
        (letter.as_str(), letter_x, YELLOW),
        ("+", plus_x, CYAN),
    ] {
        let t = s.put(m, t);
        draw(c, t, x, y, colour);
    }
}

/// The touch screen's controls: one finger, a second one, the back button, the on-screen buttons
/// at the bottom and the top on the right.
fn draw_touch(c: &mut Cpu, s: &mut Strings) {
    let m = c.mem();
    let l = lang(m);
    let [lx, x1, _] = columns(m);
    for (row, (what, does)) in [
        (T_FINGER, T_FINGER_DOES),
        (T_SECOND, T_SECOND_DOES),
        (T_BACK, T_MENU),
        (T_BOTTOM, T_BOTTOM_DOES),
        (T_TOP, T_TOP_DOES),
    ]
    .iter()
    .enumerate()
    {
        // The buttons a line further down.
        let row = if row >= 3 { row + 1 } else { row };
        let y = line_y(m, row as u32);
        let (a, b) = (s.put(m, what[l]), s.put(m, does[l]));
        draw(c, a, lx, y, WHITE);
        draw(c, b, x1, y, YELLOW);
    }
}

/// A click of the mouse's left button since the last frame, where it was.
fn click(m: Mem) -> Option<(i32, i32)> {
    static HELD: AtomicU8 = AtomicU8::new(0);
    let down = rc_hle::dinput::mouse_button_down(0);
    let was = HELD.swap(u8::from(down), Ordering::Relaxed) != 0;
    (down && !was).then(|| {
        (
            i32::from(m.ld16(MOUSE_X) as i16),
            i32::from(m.ld16(MOUSE_Y) as i16),
        )
    })
}

/// The input page, once per frame: the tabs (L1 and R1, or a click), then the keyboard's
/// page as the game has it or one of ours on its frame.
pub fn config_page(c: &mut Cpu) {
    let m = c.mem();
    let pressed = m.ld32(PRESSED);
    let capturing = m.ld32(CAPTURING) != 0;
    let clicked = if capturing { None } else { click(m) };
    if !capturing {
        let n = TABS.len() as u8;
        let mut t = TAB.load(Ordering::Relaxed) % n;
        if pressed & u32::from(L1) != 0 {
            t = (t + n - 1) % n;
        }
        if pressed & u32::from(R1) != 0 {
            t = (t + 1) % n;
        }
        if let Some((x, y)) = clicked {
            let (xs, ty) = tab_places(m);
            for (i, tx) in xs.iter().enumerate() {
                if (*tx..tx + tab_width(m, i)).contains(&x) && (ty..ty + 20).contains(&y) {
                    t = i as u8;
                }
            }
        }
        TAB.store(t, Ordering::Relaxed);
        m.st32(PRESSED, pressed & !u32::from(L1 | R1));
    }
    let mut s = Strings::new(m);
    if tab() == Tab::Keys {
        call_original(c, CONFIG_PAGE);
        draw_tabs(c, &mut s);
        return;
    }
    let mut step = 0i8;
    if tab() == Tab::Gamepad {
        if pressed & LEFT != 0 {
            step = -1;
        } else if pressed & RIGHT != 0 {
            step = 1;
        } else if let Some((x, y)) = clicked {
            let (_, minus_x, _, plus_x, line) = setup_places(m);
            if (line..line + 20).contains(&y) {
                if (minus_x - 8..minus_x + 16).contains(&x) {
                    step = -1;
                } else if (plus_x - 8..plus_x + 16).contains(&x) {
                    step = 1;
                }
            }
        }
    }
    if step != 0 {
        let n = SETUPS.len() as u8;
        let now = SETUP.load(Ordering::Relaxed);
        let next = if step < 0 {
            (now + n - 1) % n
        } else {
            (now + 1) % n
        };
        SETUP.store(next, Ordering::Relaxed);
        apply(m);
        crate::patches::save_settings();
    }
    // The page itself only frames ours and is left as it is on the keyboard's tab (escape and
    // start, select): nothing else reaches it, neither keys nor confirming (square, cross).
    let labels: Vec<u32> = (0..ENTRY_COUNT)
        .map(|i| ENTRIES + ENTRY_SIZE * i)
        .take_while(|&e| m.ld32(e) != 0)
        .map(|e| m.ld32(e + 4))
        .collect();
    let empty = s.put(m, "");
    let saved = hide_entries(m, empty);
    m.st32(PRESSED, m.ld32(PRESSED) & u32::from(START | SELECT));
    let held = m.ld32(HELD);
    m.st32(HELD, held & !u32::from(SQUARE | CROSS));
    call_original(c, CONFIG_PAGE);
    m.st32(HELD, held);
    for (a, v) in saved.into_iter().rev() {
        m.st32(a, v);
    }
    match tab() {
        Tab::Gamepad => draw_gamepad(c, &mut s, &labels),
        _ => draw_touch(c, &mut s),
    }
    draw_tabs(c, &mut s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setup_gives_every_button_once() {
        let mut a = SETUPS[0];
        a.sort_unstable();
        for s in SETUPS {
            let mut s = s;
            s.sort_unstable();
            assert_eq!(s, a);
        }
    }

    #[test]
    fn setups_as_the_playstation_has_them() {
        // Its table at 0x80097a70, by the button's bit (L2, R2, L1, R1, triangle, circle,
        // cross, square): the bit the button gives.
        const PS: [[u16; 8]; 4] = [
            [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80],
            [0x01, 0x02, 0x04, 0x08, 0x20, 0x10, 0x80, 0x40],
            [0x02, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80],
            [0x02, 0x01, 0x04, 0x08, 0x20, 0x10, 0x80, 0x40],
        ];
        // Our buttons' order (square, triangle, cross, circle, L2, L1, R2, R1) as those bits.
        const OURS: [u16; 8] = [SQUARE, TRIANGLE, CROSS, CIRCLE, L2, L1, R2, R1];
        for (ours, ps) in SETUPS.iter().zip(PS) {
            for (i, button) in OURS.iter().enumerate() {
                let k = button.trailing_zeros() as usize;
                assert_eq!(ours[i], ps[k], "button {button:#x}");
            }
        }
    }

    #[test]
    fn the_setting_round_trips() {
        set_setting("C");
        assert_eq!(setting(), "C");
        set_setting("Z");
        assert_eq!(setting(), "C");
        set_setting("A");
    }
}
