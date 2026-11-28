// SPDX-License-Identifier: MIT
//! The game project's changes to TechnoMage (the `[binary.patch]` hooks in `game.toml`): two
//! entries in the pause menu, "Configure graphics" and "Show information", that open the
//! options panel with lines of their own (the render resolution; this build and the game). The
//! texts come from a table here in the game's language, served through the game's own string
//! lookup. Addresses are of version 1.04 (the only one lifted); docs/RESEARCH.md ("The pause
//! menu, the options panel and the texts") describes the structures.
//!
//! With a window, the game's mouse pointer becomes a hardware cursor: its draws are taken aside,
//! rendered into an image and handed to the platform instead of the frame.
//!
//! Used by the native shell and the browser player (`technomage-web` includes this file).

use rc_runtime::cpu::{EAX, ESP};
use rc_runtime::dispatch::{call_guest, call_original, hook};
use rc_runtime::imports::ret_cdecl;
use rc_runtime::mem::Mem;
use rc_runtime::Cpu;
use std::sync::Mutex;

// --- The game's functions and data -----------------------------------------------------------

/// `const char *text(const char *key)`: a text of the loaded string table by key. Functions
/// reached through the linker's jump thunks are known (lifted, hooked) by the thunk's address.
const TEXT: u32 = 0x0040_2252;
/// `draw_text(font, text, x, y, size, r, g, b, 2, flags)`, cdecl; the flags' low bits align
/// the text: 0 from `x` on, 1 ending at `x`.
const DRAW_TEXT: u32 = 0x0040_10d7;
const ALIGN_LEFT: u32 = 0;
const ALIGN_RIGHT: u32 = 1;
/// Opens a menu: `menu_open(table, …)`; a table of 16-byte entries ends with a NULL key.
const MENU_OPEN: u32 = 0x0040_1393;
/// The pause menu's callbacks: the value text of entry `i`, and entry `i` chosen.
const PAUSE_VALUE: u32 = 0x0045_0a60;
const PAUSE_CHOSEN: u32 = 0x0045_0b80;
/// The options panel: set up (once per level), each frame of the pause, one entry drawn.
const OPTIONS_SETUP: u32 = 0x0040_25f9;
const OPTIONS_FRAME: u32 = 0x0040_11b8;
const OPTIONS_ENTRY: u32 = 0x0046_0a30;

/// The pause menu: resume, options, input devices, video, back to the main menu.
const PAUSE_TABLE: u32 = 0x0056_3ee8;
/// Where our entries go in it: after "Configure input devices".
const OUR_INDEX: u32 = 3;
const MENU_ENTRY: u32 = 16;
/// At most this many entries (the menu's arrays).
const MENU_MAX: u32 = 16;
/// The table of the open menu.
const MENU_TABLE: u32 = 0x0059_e290;
/// The open menu's box height (its list starts at a fixed point inside and grows downwards by
/// the font's line height, byte 0x13 of the font).
const MENU_BOX_HEIGHT: u32 = 0x0059_e23c;
const MENU_FONT: u32 = 0x0059_e2a0;
const FONT_LINE: u32 = 0x13;

/// The options panel's entries (0x24 bytes each, ended by a NULL key): four and the end, and room
/// for one more before the next data (zeros, so it ends a table of five).
const OPTIONS_TABLE: u32 = 0x0056_4c80;
const OPTIONS_ENTRY_SIZE: u32 = 0x24;
const OPTIONS_BYTES: usize = 6 * OPTIONS_ENTRY_SIZE as usize;
/// The panel's title text, its fonts (entries; metrics), the selected entry, the slide position.
const OPTIONS_TITLE: u32 = 0x0059_fb80;
const OPTIONS_FONT: u32 = 0x0059_fb84;
const OPTIONS_METRICS: u32 = 0x0059_fb74;
const OPTIONS_SELECTED: u32 = 0x0081_0c74;
const OPTIONS_SLIDE: u32 = 0x0059_fb8c;

/// The pause state: bit 1 shows the options panel, bit 11 is cleared with it; the last state
/// seen and the menu's own flags.
const PAUSE_STATE: u32 = 0x0056_3c84;
const PAUSE_SEEN: u32 = 0x0056_3c88;
const MENU_FLAGS: u32 = 0x0059_de34;
/// The language: an index into GER, ENG, FRA, ITA, SPA, NL (`LANGUAGE=` of `TECHMAGE.CFG`).
const LANGUAGE: u32 = 0x0059_de18;
/// Puts a 3D object at a screen point into the frame's ordering table; the mouse pointer is put
/// there from one place (returning to `POINTER_RETURN`), at the mouse position (the 3rd and 4th
/// argument).
const DRAW_OBJECT: u32 = 0x0040_1dc0;
const POINTER_RETURN: u32 = 0x0042_8191;
/// The ordering table: 0x400 buckets of 16-byte empty nodes per frame buffer (`0x4000` bytes
/// each, the buffer's index at `OT_BUFFER`), chained from the last to the first; a primitive is
/// chained in after the bucket of its depth (its first dword: the next node). Primitives come
/// from an arena whose free pointer is `PRIM_NEXT`.
const OT_LAST: u32 = 0x0079_9830;
const OT_BUFFER: u32 = 0x0059_cb1c;
const PRIM_NEXT: u32 = 0x0059_de10;
/// Draws an ordering table from its last node (once per frame, then the table is emptied).
const DRAW_OT: u32 = 0x0053_0e60;

/// The screen size the game lays its menus out on, and a 4096-step sine table (i16 every 4 bytes).
const SCREEN_W: u32 = 0x00f5_b320;
/// The projection's centre on the screen (x; y follows), set each frame by `CAMERA` from the
/// screen size and the camera's shake.
const CENTER_X: u32 = 0x014f_c7a0;
/// The clip rectangle's width (x, y, width, height from `0x14f6ce0`), set once with the display
/// mode (`0x40c0a0`, from `0x41b816`): the level's geometry (`0x4482e0`, a test on its packed
/// vertices), objects and effects skip what lies wholly outside it.
const CLIP_W: u32 = 0x014f_6ce8;
/// Each frame of a level the game's main loop calls the camera (it sets the projection's centre
/// from the screen size and the camera's shake), the functions that put the world's primitives
/// into the ordering table (the level's geometry first: projected, culled to the screen,
/// `SCREEN_W`), and functions that tell which objects are on the screen.
const CAMERA: u32 = 0x0040_10b9;
const WORLD: [u32; 9] = [
    0x0040_148d, // the level's geometry
    0x0040_1ff0, // objects, effects
    0x0040_25ef,
    0x0040_1721,
    0x0040_21da, // which objects are on the screen
    0x0040_1357,
    0x0040_201d,
    // Two more that project points of the world and chain primitives by depth (a water's
    // bubbles and ripples among them): on the narrower screen they lay a margin to the left.
    0x0040_1f6e,
    0x0040_22f2,
];
/// Sets how many tiles around the camera the level's geometry is drawn for (its argument, 12
/// in the rooms seen; the square's corner shows on a wider screen as floor cut off in steps).
const SET_TILE_RADIUS: u32 = 0x0040_25a4;
/// How far widescreen enlarges it at most (to 15): at 16 the last column of a layer of the
/// level's tiles reaches into the next row (its buffers hold 32 vertices a row, a radius of 16
/// needs 33), and a semi-transparent tile becomes a thin strip over the whole width; from 17
/// on the geometry falls apart.
const TILE_RADIUS_WIDER: u32 = 3;
/// Functions the widened world functions call that ask whether `SCREEN_W` is 640 (else the
/// 320-pixel mode: texture scrolling and sprite animation at half the steps, a cape wrong).
/// Also one that sizes a sprite by `320 / SCREEN_W` (the hero's body; on the wider screen he
/// shrank by its share, smaller than his cape).
const ASKS_640: [u32; 3] = [0x0040_1654, 0x0040_2135, 0x0040_1910];
/// Reads the input; with the mouse it compares the pointer with the hero's place on the screen
/// (`HERO`'s +0x38, +0x3a), which the widened projection put a margin further right.
const READ_INPUT: u32 = 0x0040_2504;
const HERO: u32 = 0x0059_142c;
/// Plays a video (Bink, into the primary surface, centred in 640 × 480); the handle of the one
/// playing (its width and height first).
const PLAY_VIDEO: u32 = 0x0053_c8d0;
const VIDEO: u32 = 0x0080_8ce8;
/// The ordering table's buckets the world's primitives go to start here (their depth); the
/// menus', texts' and panels' are below (0 to 0x1b seen, the world from 0xf8 on).
pub(crate) const WORLD_BUCKET: u32 = 0x80;
const SCREEN_H: u32 = 0x00f5_b324;
const SINE: u32 = 0x0058_6a30;

// --- Our texts ---------------------------------------------------------------------------------

// Our keys start with `RC_`, which none of the game's do.
const KEY_GRAPHICS: &str = "RC_GRAPHICS";
const KEY_INFO: &str = "RC_INFO";
const KEY_RESOLUTION: &str = "RC_RESOLUTION";
const KEY_NATIVE: &str = "RC_NATIVE";
const KEY_WIDESCREEN: &str = "RC_WIDESCREEN";
const KEY_ANTIALIAS: &str = "RC_ANTIALIAS";
const KEY_SHOW_FPS: &str = "RC_SHOW_FPS";
const KEY_REFINE: &str = "RC_REFINE";
const KEY_SMOOTH_TEXTURES: &str = "RC_SMOOTH_TEX";
const KEY_SMOOTH_FONTS: &str = "RC_SMOOTH_FONT";
const KEY_VERSION: &str = "RC_VERSION";
const KEY_BUILT: &str = "RC_BUILT";
const KEY_COMMIT: &str = "RC_COMMIT";
const KEY_PLATFORM: &str = "RC_PLATFORM";
const KEY_GAME: &str = "RC_GAME";
const KEY_LANGUAGE: &str = "RC_LANGUAGE";

/// Our texts by key, in the game's languages (GER, ENG, FRA, ITA, SPA, NL).
const TEXTS: &[(&str, [&str; 6])] = &[
    (
        KEY_GRAPHICS,
        [
            "Grafik konfigurieren",
            "Configure graphics",
            "Configurer les graphismes",
            "Configura la grafica",
            "Configurar gráficos",
            "Grafische instellingen",
        ],
    ),
    (
        KEY_INFO,
        [
            "Zeige Informationen",
            "Show information",
            "Afficher les informations",
            "Mostra informazioni",
            "Mostrar información",
            "Informatie tonen",
        ],
    ),
    (
        KEY_RESOLUTION,
        [
            "Renderauflösung",
            "Render resolution",
            "Résolution de rendu",
            "Risoluzione di rendering",
            "Resolución de renderizado",
            "Renderresolutie",
        ],
    ),
    (
        KEY_NATIVE,
        [
            "Native Auflösung",
            "Native resolution",
            "Résolution native",
            "Risoluzione nativa",
            "Resolución nativa",
            "Native resolutie",
        ],
    ),
    (
        KEY_WIDESCREEN,
        [
            "Breitbild",
            "Widescreen",
            "Écran large",
            "Schermo panoramico",
            "Pantalla panorámica",
            "Breedbeeld",
        ],
    ),
    (
        KEY_ANTIALIAS,
        [
            "Kantenglättung",
            "Antialiasing",
            "Anticrénelage",
            "Antialiasing",
            "Suavizado de bordes",
            "Anti-aliasing",
        ],
    ),
    (
        KEY_SHOW_FPS,
        [
            "Bildrate anzeigen",
            "Show frame rate",
            "Afficher les images par seconde",
            "Mostra fotogrammi al secondo",
            "Mostrar fotogramas por segundo",
            "Framerate tonen",
        ],
    ),
    (
        KEY_REFINE,
        [
            "Grafik verfeinern",
            "Refine graphics",
            "Affiner les graphismes",
            "Affina la grafica",
            "Refinar gráficos",
            "Graphics verfijnen",
        ],
    ),
    (
        KEY_SMOOTH_TEXTURES,
        [
            "Texturen glätten",
            "Smooth textures",
            "Lisser les textures",
            "Ammorbidisci le texture",
            "Suavizar texturas",
            "Texturen gladmaken",
        ],
    ),
    (
        KEY_SMOOTH_FONTS,
        [
            "Schrift glätten",
            "Smooth fonts",
            "Lisser les polices",
            "Ammorbidisci i caratteri",
            "Suavizar fuentes",
            "Lettertypen gladmaken",
        ],
    ),
    (
        KEY_VERSION,
        [
            "Version", "Version", "Version", "Versione", "Versión", "Versie",
        ],
    ),
    (
        KEY_BUILT,
        [
            "Erstellt",
            "Built",
            "Compilé",
            "Compilato",
            "Compilado",
            "Gebouwd",
        ],
    ),
    (
        KEY_COMMIT,
        ["Commit", "Commit", "Commit", "Commit", "Commit", "Commit"],
    ),
    (
        KEY_PLATFORM,
        [
            "Plattform",
            "Platform",
            "Plate-forme",
            "Piattaforma",
            "Plataforma",
            "Platform",
        ],
    ),
    (KEY_GAME, ["Spiel", "Game", "Jeu", "Gioco", "Juego", "Spel"]),
    (
        KEY_LANGUAGE,
        ["Sprache", "Language", "Langue", "Lingua", "Idioma", "Taal"],
    ),
];

/// The game's languages by their own names.
const LANGUAGES: [&str; 6] = [
    "Deutsch",
    "English",
    "Français",
    "Italiano",
    "Español",
    "Nederlands",
];

/// The game's version: the only executable lifted (German cover CD, also run with the English
/// data).
const GAME_VERSION: &str = "TechnoMage 1.04";

/// What this program runs on.
fn platform() -> String {
    if cfg!(target_family = "wasm") {
        return "Browser (WebAssembly)".into();
    }
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    format!("{os} {}", std::env::consts::ARCH)
}

/// A page of ours in the options panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Graphics,
    Refine,
    Info,
}

/// Our pause-menu entries from `OUR_INDEX` on: their text and the page they open.
const OURS: [(&str, Page); 3] = [
    (KEY_GRAPHICS, Page::Graphics),
    (KEY_REFINE, Page::Refine),
    (KEY_INFO, Page::Info),
];
const OUR_COUNT: u32 = OURS.len() as u32;

/// The lines of a page: the key of the label and the value.
fn rows(m: Mem, page: Page) -> Vec<(&'static str, String)> {
    match page {
        Page::Graphics => {
            let (w, h) = rc_hle::d3d::screen_render_size();
            vec![(KEY_RESOLUTION, format!("{w} x {h}"))]
        }
        Page::Refine => Vec::new(),
        Page::Info => {
            let lang = m.ld32(LANGUAGE) as usize;
            vec![
                (KEY_VERSION, env!("CARGO_PKG_VERSION").to_string()),
                (
                    KEY_BUILT,
                    option_env!("RC_BUILD_TIME").unwrap_or("?").to_string(),
                ),
                (
                    KEY_COMMIT,
                    option_env!("RC_COMMIT").unwrap_or("?").to_string(),
                ),
                (KEY_PLATFORM, platform()),
                (KEY_GAME, GAME_VERSION.to_string()),
                (
                    KEY_LANGUAGE,
                    LANGUAGES.get(lang).unwrap_or(&"?").to_string(),
                ),
            ]
        }
    }
}

/// Our text for `key` in language `lang` (English for a language we do not know).
fn text_for(key: &[u8], lang: u32) -> Option<&'static str> {
    let (_, t) = TEXTS.iter().find(|(k, _)| k.as_bytes() == key)?;
    Some(t.get(lang as usize).unwrap_or(&t[1]))
}

/// `s` in the game's code page (Windows-1252); `?` for what it lacks.
fn ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|ch| {
            u16::try_from(u32::from(ch))
                .ok()
                .and_then(rc_hle::codepage::from_unicode)
        })
        .map(|b| b.unwrap_or(b'?'))
        .collect()
}

// --- State ---------------------------------------------------------------------------------------

/// Guest memory of ours, made on first use.
#[derive(Clone, Copy)]
struct Blocks {
    /// The pause menu with our entries.
    menu: u32,
    /// Our keys, then a text buffer per key, then a buffer per value of a page's lines.
    strings: u32,
}

const STR_KEYS: u32 = 0;
const STR_KEYS_SIZE: u32 = 512;
const STR_TEXTS: u32 = STR_KEYS + STR_KEYS_SIZE;
const STR_TEXT_SIZE: u32 = 64;
const STR_VALUES: u32 = STR_TEXTS + STR_TEXT_SIZE * TEXTS.len() as u32;
const STR_VALUE_SIZE: u32 = 64;
const ROWS_MAX: u32 = 8;
const STR_BYTES: u32 = STR_VALUES + STR_VALUE_SIZE * ROWS_MAX;

struct State {
    blocks: Option<Blocks>,
    /// The options panel's entries, title and selection while ours are in their place.
    saved: Option<(Vec<u8>, u32, u32)>,
    /// Our page in the options panel, while it is shown.
    page: Option<Page>,
}

static STATE: Mutex<State> = Mutex::new(State {
    blocks: None,
    saved: None,
    page: None,
});

fn blocks(m: Mem) -> Blocks {
    let mut s = STATE.lock().expect("patch state");
    *s.blocks.get_or_insert_with(|| {
        let menu = rc_runtime::vm::heap_alloc(m, MENU_ENTRY * (MENU_MAX + 1), true);
        let strings = rc_runtime::vm::heap_alloc(m, STR_BYTES, true);
        let mut at = strings + STR_KEYS;
        for (k, _) in TEXTS {
            m.write(at, k.as_bytes());
            m.st8(at + k.len() as u32, 0);
            at += k.len() as u32 + 1;
        }
        Blocks { menu, strings }
    })
}

/// The guest address of our key `key`.
fn key_addr(b: Blocks, key: &str) -> u32 {
    let mut at = b.strings + STR_KEYS;
    for (k, _) in TEXTS {
        if *k == key {
            return at;
        }
        at += k.len() as u32 + 1;
    }
    unreachable!("{key} is one of ours")
}

fn put_str(m: Mem, at: u32, cap: u32, s: &[u8]) -> u32 {
    let n = s.len().min(cap as usize - 1);
    m.write(at, &s[..n]);
    m.st8(at + n as u32, 0);
    at
}

fn arg(c: &Cpu, i: u32) -> u32 {
    c.mem().ld32(c.r[ESP] + 4 + 4 * i)
}

// --- The string lookup: our keys first -----------------------------------------------------------

/// Our text for `key` in the game's language, as a guest string (None: not ours).
fn our_text(m: Mem, key: &[u8]) -> Option<u32> {
    let t = text_for(key, m.ld32(LANGUAGE))?;
    let i = TEXTS.iter().position(|(n, _)| n.as_bytes() == key)? as u32;
    let at = blocks(m).strings + STR_TEXTS + STR_TEXT_SIZE * i;
    Some(put_str(m, at, STR_TEXT_SIZE, &ansi(t)))
}

fn text(c: &mut Cpu) {
    let m = c.mem();
    let key = arg(c, 0);
    if key != 0 {
        let k = m.cstr(key, 32);
        if let Some(at) = our_text(m, &k) {
            ret_cdecl(c, at);
            return;
        }
    }
    call_original(c, TEXT);
}

// --- The pause menu with our entry ---------------------------------------------------------------

/// The pause menu's entries up to and with the end, as the menu reads them.
fn pause_len(m: Mem) -> u32 {
    let mut n = 0;
    while n < MENU_MAX - 1 && m.ld32(PAUSE_TABLE + MENU_ENTRY * n) != 0 {
        n += 1;
    }
    n
}

/// An entry of our pause menu: the game's (by its index there) or ours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entry {
    Game(u32),
    Ours(Page),
}

/// Our menu's entry `i`.
fn entry(i: u32) -> Entry {
    if i < OUR_INDEX {
        Entry::Game(i)
    } else if i < OUR_INDEX + OUR_COUNT {
        Entry::Ours(OURS[(i - OUR_INDEX) as usize].1)
    } else {
        Entry::Game(i - OUR_COUNT)
    }
}

/// The pause menu's entry `i` in ours.
fn our_index(i: u32) -> u32 {
    if i < OUR_INDEX {
        i
    } else {
        i + OUR_COUNT
    }
}

/// Copy the entries' state (all but the key) between the pause menu and ours.
fn sync(m: Mem, to_original: bool) {
    let Some(b) = STATE.lock().expect("patch state").blocks else {
        return;
    };
    for i in 0..=pause_len(m) {
        let ours = b.menu + MENU_ENTRY * our_index(i);
        let orig = PAUSE_TABLE + MENU_ENTRY * i;
        for w in 1..4 {
            let (from, to) = if to_original {
                (ours, orig)
            } else {
                (orig, ours)
            };
            m.st32(to + 4 * w, m.ld32(from + 4 * w));
        }
    }
}

fn menu_open(c: &mut Cpu) {
    let m = c.mem();
    if arg(c, 0) == PAUSE_TABLE {
        let b = blocks(m);
        let n = pause_len(m);
        let mut to = b.menu;
        for i in 0..=n {
            if i == OUR_INDEX {
                for (key, _) in OURS {
                    // Like "Configure input devices": an action that closes the menu.
                    let like = PAUSE_TABLE + MENU_ENTRY * 2;
                    m.st32(to, key_addr(b, key));
                    m.st32(to + 4, m.ld32(like + 4));
                    m.st32(to + 8, 0);
                    m.st32(to + 12, 1);
                    to += MENU_ENTRY;
                }
            }
            let from = PAUSE_TABLE + MENU_ENTRY * i;
            for w in 0..4 {
                m.st32(to + 4 * w, m.ld32(from + 4 * w));
            }
            to += MENU_ENTRY;
        }
        m.st32(c.r[ESP] + 4, b.menu);
        call_original(c, MENU_OPEN);
        // Its box was sized for the game's entries: a line more for each of ours.
        let line = u32::from(m.ld8(m.ld32(MENU_FONT) + FONT_LINE));
        m.st32(MENU_BOX_HEIGHT, m.ld32(MENU_BOX_HEIGHT) + OUR_COUNT * line);
        return;
    }
    call_original(c, MENU_OPEN);
}

/// A pause-menu callback with an entry index first: ours, or the original's with its index.
fn pause_callback(c: &mut Cpu, f: u32, ours: fn(&mut Cpu, Page)) {
    let open = STATE.lock().expect("patch state").blocks.map(|b| b.menu);
    if open.is_none() || c.mem().ld32(MENU_TABLE) != open.unwrap_or(0) {
        call_original(c, f); // not our menu (a build lifted without the hooks)
        return;
    }
    let i = arg(c, 0);
    match entry(i) {
        Entry::Ours(page) => ours(c, page),
        Entry::Game(j) => {
            let m = c.mem();
            if f == PAUSE_CHOSEN && j == 1 {
                restore_options(m); // the real options, after ours
            }
            m.st32(c.r[ESP] + 4, j);
            sync(m, true);
            call_original(c, f);
            sync(m, false);
        }
    }
}

fn pause_value(c: &mut Cpu) {
    pause_callback(c, PAUSE_VALUE, |c, _| ret_cdecl(c, 0));
}

fn pause_chosen(c: &mut Cpu) {
    pause_callback(c, PAUSE_CHOSEN, |c, page| {
        show_page(c, page);
        let m = c.mem();
        // As "Options" does: the panel slides in, the menu waits for it.
        m.st32(PAUSE_SEEN, u32::MAX);
        m.st32(PAUSE_STATE, (m.ld32(PAUSE_STATE) & !0x800) | 2);
        m.st32(MENU_FLAGS, m.ld32(MENU_FLAGS) | 2);
        ret_cdecl(c, 0);
    });
}

// --- The options panel with our pages -------------------------------------------------------------

/// Put our page into the options panel: one entry that draws its lines, and its title (the
/// panel's own are kept until it is hidden).
fn show_page(c: &mut Cpu, page: Page) {
    let m = c.mem();
    let b = blocks(m);
    {
        let mut s = STATE.lock().expect("patch state");
        if s.saved.is_none() {
            let mut entries = vec![0u8; OPTIONS_BYTES];
            m.read(OPTIONS_TABLE, &mut entries);
            s.saved = Some((entries, m.ld32(OPTIONS_TITLE), m.ld32(OPTIONS_SELECTED)));
        }
        s.page = Some(page);
    }
    // Laid out as the first volume line; not selectable, no slider (drawn in options_entry).
    let e = OPTIONS_TABLE;
    let like = m.ld32(e + 4);
    let top = i32::from(m.ld16(e + 6) as i16);
    m.write(e, &[0u8; OPTIONS_BYTES - OPTIONS_ENTRY_SIZE as usize]);
    if let Some((key, _)) = rows(m, page).first() {
        m.st32(e, key_addr(b, key));
        m.st32(e + 4, like);
        m.st16(e + 8, (-60i16) as u16);
        m.st16(e + 0xc, 2);
        m.st16(e + 0x20, 0xffff);
    }
    let mut selected = e;
    CHECKBOX_QUIET.store(
        rc_platform::frame_number() + 15,
        std::sync::atomic::Ordering::Relaxed,
    );
    if !switches(page).is_empty() {
        // The switches (below the line): selectable, a value of 0 or 1 that the panel's own left
        // and right keys change.
        let first = first_switch(page);
        for (i, sw) in switches(page).iter().enumerate() {
            let k = switch_entry(page, i);
            m.st32(k, key_addr(b, sw.key));
            m.st32(k + 4, like);
            m.st16(k + 6, (top + ROW_STEP * (i as i32 + first as i32)) as u16);
            m.st16(k + 8, (-60i16) as u16);
            m.st16(k + 0xc, 2);
            m.st16(k + 0xe, 1);
            m.st16(k + 0x10, u16::from((sw.get)()));
            m.st16(k + 0x12, 1);
            m.st16(k + 0x20, 0xffff);
        }
        selected = switch_entry(page, 0);
    }
    let title_key = OURS
        .iter()
        .find(|o| o.1 == page)
        .map_or(KEY_GRAPHICS, |o| o.0);
    let title = call_guest(c, TEXT, &[key_addr(b, title_key)]);
    m.st32(OPTIONS_TITLE, title);
    m.st32(OPTIONS_SELECTED, selected);
}

/// The options panel's own entries again.
fn restore_options(m: Mem) {
    let saved = {
        let mut s = STATE.lock().expect("patch state");
        s.page = None;
        s.saved.take()
    };
    if let Some((entries, title, selected)) = saved {
        m.write(OPTIONS_TABLE, &entries);
        m.st32(OPTIONS_TITLE, title);
        m.st32(OPTIONS_SELECTED, selected);
    }
}

fn showing() -> Option<Page> {
    STATE.lock().expect("patch state").page
}

fn options_setup(c: &mut Cpu) {
    restore_options(c.mem()); // a level loads: the panel is set up from its own entries
    call_original(c, OPTIONS_SETUP);
}

fn options_frame(c: &mut Cpu) {
    call_original(c, OPTIONS_FRAME);
    let m = c.mem();
    if showing().is_some() && m.ld32(PAUSE_STATE) & 2 == 0 && m.ld32(OPTIONS_SLIDE) == 0 {
        restore_options(m); // slid out
    }
}

/// The distance of our lines (thousandths of the screen height; the volumes' is 70).
const ROW_STEP: i32 = 55;

/// A switch of the graphics page: its label, and the setting it turns on and off.
struct Switch {
    key: &'static str,
    /// Its name in the settings file.
    name: &'static str,
    get: fn() -> bool,
    set: fn(bool),
}

/// The graphics page's switches, the options panel's entries from the second on (the table
/// has room for five entries and the end).
const GRAPHICS: [Switch; 4] = [
    Switch {
        key: KEY_NATIVE,
        name: "native_resolution",
        get: rc_platform::render::native,
        set: rc_platform::render::set_native,
    },
    Switch {
        key: KEY_WIDESCREEN,
        name: "widescreen",
        get: rc_platform::render::widescreen,
        set: rc_platform::render::set_widescreen,
    },
    Switch {
        key: KEY_ANTIALIAS,
        name: "antialiasing",
        get: rc_platform::render::antialias,
        set: rc_platform::render::set_antialias,
    },
    Switch {
        key: KEY_SHOW_FPS,
        name: "show_fps",
        get: rc_platform::render::show_fps,
        set: rc_platform::render::set_show_fps,
    },
];

/// The refining page's switches (no line above them).
const REFINE: [Switch; 2] = [
    Switch {
        key: KEY_SMOOTH_TEXTURES,
        name: "smooth_textures",
        get: rc_platform::render::smooth_textures,
        set: rc_platform::render::set_smooth_textures,
    },
    Switch {
        key: KEY_SMOOTH_FONTS,
        name: "smooth_fonts",
        get: rc_platform::render::smooth_fonts,
        set: rc_platform::render::set_smooth_fonts,
    },
];

// Our pages' entries and the end fit the table.
const _: () = assert!((2 + GRAPHICS.len()) * OPTIONS_ENTRY_SIZE as usize <= OPTIONS_BYTES);
const _: () = assert!(GRAPHICS.len() <= SWITCHES_MAX && REFINE.len() <= SWITCHES_MAX);
const _: () = assert!((1 + REFINE.len()) * OPTIONS_ENTRY_SIZE as usize <= OPTIONS_BYTES);

/// The most switches a page has.
const SWITCHES_MAX: usize = 5;

/// A page's switches.
fn switches(page: Page) -> &'static [Switch] {
    match page {
        Page::Graphics => &GRAPHICS,
        Page::Refine => &REFINE,
        Page::Info => &[],
    }
}

/// Every switch, for the settings file.
fn all_switches() -> impl Iterator<Item = &'static Switch> {
    GRAPHICS.iter().chain(REFINE.iter())
}

/// Whether a page has lines of text (`rows`; the first entry draws them).
fn has_lines(page: Page) -> bool {
    page != Page::Refine
}

/// The entry a page's switches begin at (after its lines, if it has them).
fn first_switch(page: Page) -> u32 {
    u32::from(has_lines(page))
}

/// The options entry of switch `i` of `page`.
fn switch_entry(page: Page, i: usize) -> u32 {
    OPTIONS_TABLE + OPTIONS_ENTRY_SIZE * (first_switch(page) + i as u32)
}

/// The switch an options entry is, if it is one.
fn switch_at(page: Page, e: u32) -> Option<usize> {
    (0..switches(page).len()).find(|&i| switch_entry(page, i) == e)
}

/// The first frame the switch takes input again (a moment after each turn and after the page
/// opened: the press that chose the page is still down).
static CHECKBOX_QUIET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Where an options entry's text goes: the label's right end, the value's start (where a
/// slider is), its top, as the game places its entries (sliding in from the side).
fn entry_place(m: Mem, e: u32) -> (i32, i32, i32) {
    let i16_at = |a: u32| i32::from(m.ld16(a) as i16);
    let w = m.ld32(SCREEN_W) as i32;
    let h = m.ld32(SCREEN_H) as i32;
    let (x, y) = (i16_at(e + 4), i16_at(e + 6));
    let slide = m.ld32(OPTIONS_SLIDE) as i32 + i16_at(e + 0xa);
    let x = if slide < 0 {
        -500
    } else if slide > 0x400 {
        x
    } else {
        ((i16_at(SINE + 4 * (slide & 0xfff) as u32) * (x + 500)) >> 12) - 500
    };
    let metrics = m.ld32(OPTIONS_METRICS);
    let number_x = w * x / 1000 - i32::from(m.ld8(metrics + 9));
    let label_x = i16_at(e + 8) * w / 1000 + number_x;
    // The game aligns both to the right: the label before the number, the number where its
    // slider begins. Our values are longer than a number: they go where the slider is.
    (label_x, number_x + w / 50, y * h / 1000)
}

/// A line of the panel: `label` ending at `label_x`, `value` from `value_x` on (guest strings).
fn draw_line(c: &mut Cpu, label: u32, value: u32, (label_x, value_x, y): (i32, i32, i32)) {
    let (font, colour) = (c.mem().ld32(OPTIONS_FONT), 0x80);
    for (t, at, align) in [(label, label_x, ALIGN_RIGHT), (value, value_x, ALIGN_LEFT)] {
        call_guest(
            c,
            DRAW_TEXT,
            &[
                font, t, at as u32, y as u32, 0x12, colour, colour, colour, 2, align,
            ],
        );
    }
}

/// One entry of the options panel; ours draws its page's lines (the label, and the value where
/// a slider would be), and the graphics page's box.
fn options_entry(c: &mut Cpu) {
    let e = arg(c, 0);
    let page = showing();
    match page {
        Some(page) if e == OPTIONS_TABLE && has_lines(page) => {
            let m = c.mem();
            let (label_x, value_x, top) = entry_place(m, e);
            let h = m.ld32(SCREEN_H) as i32;
            let b = blocks(m);
            for (r, (key, value)) in rows(m, page)
                .into_iter()
                .take(ROWS_MAX as usize)
                .enumerate()
            {
                let label = our_text(m, key.as_bytes()).unwrap_or(0);
                let at = b.strings + STR_VALUES + STR_VALUE_SIZE * r as u32;
                let value = put_str(m, at, STR_VALUE_SIZE, &ansi(&value));
                let y = top + ROW_STEP * r as i32 * h / 1000;
                draw_line(c, label, value, (label_x, value_x, y));
            }
            ret_cdecl(c, c.r[EAX]);
        }
        Some(page) if switch_at(page, e).is_some() => {
            // The game's own drawing of a non-slider entry: the highlight when selected, the
            // selection by the mouse.
            call_original(c, OPTIONS_ENTRY);
            checkbox(c, page, switch_at(page, e).expect("a switch"));
        }
        _ => call_original(c, OPTIONS_ENTRY),
    }
}

/// Switch `i` of a page: its value from the panel's left and right keys, Enter or
/// Space while it is selected, or a click on its line; the setting kept. Shown with the game's
/// own words for on and off (its font has no box or brackets).
fn checkbox(c: &mut Cpu, page: Page, i: usize) {
    use std::sync::atomic::{AtomicU8, Ordering};
    /// Per switch, the left button, Enter and Space as last seen (bits 0, 1, 2), to act on
    /// presses only.
    static HELD: [AtomicU8; SWITCHES_MAX] = [const { AtomicU8::new(0) }; SWITCHES_MAX];
    const DIK_RETURN: u8 = 0x1c;
    const DIK_SPACE: u8 = 0x39;
    let m = c.mem();
    let sw = &switches(page)[i];
    let k = switch_entry(page, i);
    let place = entry_place(m, k);
    let h = m.ld32(SCREEN_H) as i32;
    // The mouse position as the platform has it (the game's own follows a frame later).
    let (mx, my) = rc_hle::user32::cursor();
    let on_line = my >= place.2 && my < place.2 + 40 * h / 1000 && mx < place.1 + 60;
    let now = u8::from(rc_hle::dinput::mouse_button_down(0))
        | u8::from(rc_hle::dinput::key_down(DIK_RETURN)) << 1
        | u8::from(rc_hle::dinput::key_down(DIK_SPACE)) << 2;
    let pressed = now & !HELD[i].swap(now, Ordering::Relaxed);
    let selected = m.ld32(OPTIONS_SELECTED) == k;
    // One turn per press (the keys' state can flicker within one), and none from the press
    // that opened the page.
    let frame = rc_platform::frame_number();
    if ((pressed & 1 != 0 && on_line) || (pressed & 6 != 0 && selected))
        && frame >= CHECKBOX_QUIET.load(Ordering::Relaxed)
    {
        CHECKBOX_QUIET.store(frame + 8, Ordering::Relaxed);
        m.st16(k + 0x10, 1 - m.ld16(k + 0x10).min(1));
    }
    let on = m.ld16(k + 0x10) != 0;
    if on != (sw.get)() {
        (sw.set)(on);
        save_settings();
    }
    let b = blocks(m);
    let label = our_text(m, sw.key.as_bytes()).unwrap_or(0);
    let key = put_str(
        m,
        b.strings + STR_VALUES + STR_VALUE_SIZE * (ROWS_MAX - 1),
        STR_VALUE_SIZE,
        if on { b"OPT_ON" } else { b"OPT_OFF" },
    );
    let value = call_guest(c, TEXT, &[key]);
    draw_line(c, label, value, place);
}

// --- Smooth fonts -----------------------------------------------------------------------------------

/// The outline fonts the game's are redrawn with (DejaVu, Bitstream Vera's license: see
/// `assets/fonts/LICENSE`), cut to Windows-1252: the dialogue font is a Verdana-like sans, the
/// status font its bold.
const FACE: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const FACE_BOLD: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf");

/// Text drawn: with smooth fonts its font's glyphs told to the renderer (`rc_hle::glyphs`) first.
fn draw_text(c: &mut Cpu) {
    if rc_platform::render::smooth_fonts() {
        register_font(c.mem(), arg(c, 0));
    }
    call_original(c, DRAW_TEXT);
}

/// A font as the game keeps it: its name (16 bytes), at 0x13 its line height, at 0x14 its
/// glyph count, then pointers to its table from character to glyph (256 bytes) and to the
/// glyphs' x, y, width and height on the texture page (a byte each per glyph).
fn register_font(m: Mem, font: u32) {
    use std::collections::HashMap;
    /// The fonts told so far, by address, with what was read of them.
    static TOLD: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);
    if font == 0 {
        return;
    }
    let count = m.ld32(font + 0x14);
    let tables = [0x18, 0x1c, 0x20, 0x24, 0x28].map(|o| m.ld32(font + o));
    if count == 0 || count > 256 || tables.contains(&0) {
        return;
    }
    // What identifies it as read: its count and tables (a font loaded anew elsewhere differs).
    let stamp = tables
        .iter()
        .fold(count, |a, &t| a.wrapping_mul(31).wrapping_add(t));
    let mut told = TOLD.lock().expect("fonts");
    let told = told.get_or_insert_with(HashMap::new);
    if told.get(&font) == Some(&stamp) {
        return;
    }
    told.insert(font, stamp);
    let [map, xs, ys, ws, hs] = tables;
    // The first character of each glyph (a glyph without one, or the space's, is left).
    let mut chars = vec![None; count as usize];
    for b in (0x21..=0xffu32).rev() {
        let g = m.ld8(map + b) as usize;
        if g < chars.len() && b != 0x20 && g != usize::from(m.ld8(map + 0x20)) {
            let ch = char::from_u32(u32::from(rc_hle::codepage::to_unicode(b as u8)));
            chars[g] = ch.or(chars[g]);
        }
    }
    let cells = chars
        .iter()
        .enumerate()
        .filter_map(|(g, ch)| {
            let g = g as u32;
            Some(rc_hle::glyphs::Cell {
                rect: [xs, ys, ws, hs].map(|t| u16::from(m.ld8(t + g))),
                ch: (*ch)?,
            })
        })
        .collect();
    let name = m.cstr(font, 16);
    let face = if name.as_slice() == b"STATUS" {
        FACE_BOLD
    } else {
        FACE
    };
    rc_hle::glyphs::register(font, rc_hle::glyphs::GlyphSet { cells, face });
}

// --- Settings ------------------------------------------------------------------------------------

/// The project's settings, in the user directory beside the game's own `TechnoMage.ini`.
const SETTINGS: &str = "TechnoMageRecomp.cfg";

fn settings_path() -> Option<std::path::PathBuf> {
    rc_runtime::vfs::get().user_dir.map(|d| d.join(SETTINGS))
}

fn load_settings() {
    let text = settings_path().and_then(|p| rc_runtime::host::read_to_string(&p));
    for line in text.unwrap_or_default().lines() {
        if let Some((k, v)) = line.split_once('=') {
            if let Some(sw) = all_switches().find(|sw| sw.name == k.trim()) {
                (sw.set)(v.trim() == "1");
            } else if k.trim() == crate::controls::SETTING {
                crate::controls::set_setting(v.trim());
            }
        }
    }
}

pub(crate) fn save_settings() {
    let Some(p) = settings_path() else {
        return;
    };
    if let Some(dir) = p.parent() {
        rc_runtime::host::create_dir_all(dir);
    }
    let mut text: String = all_switches()
        .map(|sw| format!("{}={}\n", sw.name, u8::from((sw.get)())))
        .collect();
    text += &format!(
        "{}={}\n",
        crate::controls::SETTING,
        crate::controls::setting()
    );
    let how = rc_runtime::host::Open {
        write: true,
        create: true,
        truncate: true,
        ..Default::default()
    };
    if let Err(e) =
        rc_runtime::host::File::open(&p, how).and_then(|mut f| f.write_all(text.as_bytes()))
    {
        rc_runtime::log!("technomage: {}: {e}", p.display());
    }
}

// --- The mouse pointer as a hardware cursor --------------------------------------------------------

/// The part of the screen around the mouse position the pointer is looked for in (it is about
/// 30 pixels long, its glow about 50 wide).
const POINTER_BOX: i32 = 128;

/// Guest buffers the pointer is rendered into: colour and depth (32-bit each, the depth with 24
/// bits as the game's: the glow lies just behind the stone), screen-sized.
static POINTER_BUFFERS: Mutex<Option<(u32, u32, u32, u32)>> = Mutex::new(None);

/// The pointer's primitives of this frame, taken out of the ordering table, and the mouse
/// position; drawn on their own after the frame's table (see `draw_ot`).
static POINTER: Mutex<Option<(Vec<u32>, i32, i32)>> = Mutex::new(None);
/// Set while `draw_ot` draws the pointer's primitives through the game's own table drawing.
static DRAWING_POINTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn draw_object(c: &mut Cpu) {
    let m = c.mem();
    if m.ld32(c.r[ESP]) != POINTER_RETURN {
        call_original(c, DRAW_OBJECT);
        return;
    }
    if !rc_platform::cursor::enabled() {
        // Drawn in the frame, among the world's buckets: placed on the wider screen like it.
        let at = c.r[ESP] + 4 + 4 * 2;
        let x = m.ld32(at) as i32;
        m.st32(at, (x + game_to_screen_shift(x)) as u32);
        wide_screen(c, |c| call_original(c, DRAW_OBJECT));
        return;
    }
    let (x, y) = (arg(c, 2) as i32, arg(c, 3) as i32);
    let from = m.ld32(PRIM_NEXT);
    call_original(c, DRAW_OBJECT);
    let to = m.ld32(PRIM_NEXT);
    // Unchain the primitives just made, keeping their order.
    let mut taken = Vec::new();
    let mut node = OT_LAST + (m.ld32(OT_BUFFER) << 14);
    let mut guard = 0;
    while node != 0 && guard < 1 << 20 {
        let next = m.ld32(node);
        if (from..to).contains(&next) {
            taken.push(next);
            m.st32(node, m.ld32(next));
        } else {
            node = next;
        }
        guard += 1;
    }
    *POINTER.lock().expect("pointer") = Some((taken, x, y));
}

/// The frame's table drawn; then the pointer's primitives on their own, taken aside as a cursor.
fn draw_ot(c: &mut Cpu) {
    use std::sync::atomic::Ordering;
    if DRAWING_POINTER.load(Ordering::Relaxed) {
        call_original(c, DRAW_OT);
        return;
    }
    let open = c.mem().ld32(PAUSE_STATE) == 0x40;
    SPLIT.store(open, Ordering::Relaxed);
    let since = if open {
        0
    } else {
        MENU_SINCE.load(Ordering::Relaxed).saturating_add(1)
    };
    MENU_SINCE.store(since, Ordering::Relaxed);
    crate::controls::touch_bar(c);
    let m = c.mem();
    crate::controls::note_key_pictures(m, arg(c, 0), rc_hle::ddraw::margin() as i32);
    draw_split(c);
    let Some((prims, x, y)) = POINTER.lock().expect("pointer").take() else {
        return;
    };
    if prims.is_empty() {
        return;
    }
    // Each primitive in a chain of its own (an empty node, it, an empty node), in the table's
    // order: drawn together, the game's renderer would group them by kind and put the glow over
    // the stone, which the frame's other primitives keep it from.
    let m = c.mem();
    let ends = pointer_chain(m);
    let (head, tail) = (ends, ends + 16);
    DRAWING_POINTER.store(true, Ordering::Relaxed);
    let draws = rc_hle::d3d::capture(|| {
        for &p in &prims {
            m.write(head, &[0u8; 32]);
            m.st32(head, p);
            m.st32(p, tail);
            call_guest(c, DRAW_OT, &[head]);
        }
    });
    DRAWING_POINTER.store(false, Ordering::Relaxed);
    if let Some(image) = render_pointer(m, &draws, x, y) {
        rc_platform::cursor::show(smooth_pointer(image));
    }
}

// --- Widescreen ------------------------------------------------------------------------------------

/// Widescreen (`rc_platform::render::widescreen`): the HLE makes the screen wider than the
/// game's 640 × 480 by a margin on each side and moves what the game draws into the middle
/// (`rc_hle::ddraw::margin`), so menus, panels, maps and videos stay as they are, in the middle.
/// The world is drawn over the whole width: the camera and the functions that put the world's
/// primitives into the ordering table see the wider screen (its size, the projection's centre
/// moved by the margin, the clip rectangle's width), so they cull to it and place the world on its pixels; the ordering
/// table's world buckets are then drawn on the whole width, the rest moved into the middle.
/// The tile radius, larger by the wider screen's share (at the level's start: a window
/// resized later keeps it until the next level).
fn set_tile_radius(c: &mut Cpu) {
    let r = arg(c, 0);
    let ox = rc_hle::ddraw::margin();
    if ox > 0 && (1..=24).contains(&r) {
        let wide = (r * (640 + 2 * ox))
            .div_ceil(640)
            .min(r + TILE_RADIUS_WIDER);
        c.mem().st32(c.r[ESP] + 4, wide);
    }
    call_original(c, SET_TILE_RADIUS);
}

/// Whether a world function ran since the last ordering table was drawn, and whether one did
/// for the table being drawn.
static WORLD_DRAWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WORLD_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// `SCREEN_W` as the game set it while a world function sees the wider screen (0: none does).
static NARROW_W: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A function of `ASKS_640` with the game's own `SCREEN_W`.
fn narrow(c: &mut Cpu, f: u32) {
    let w = NARROW_W.load(std::sync::atomic::Ordering::Relaxed);
    if w == 0 {
        return call_original(c, f);
    }
    let m = c.mem();
    let wide = m.ld32(SCREEN_W);
    m.st32(SCREEN_W, w);
    call_original(c, f);
    m.st32(SCREEN_W, wide);
}

fn asks_640_0(c: &mut Cpu) {
    narrow(c, ASKS_640[0]);
}

fn asks_640_1(c: &mut Cpu) {
    narrow(c, ASKS_640[1]);
}

fn asks_640_2(c: &mut Cpu) {
    narrow(c, ASKS_640[2]);
}

/// The input read with the hero's place on the screen in the game's pixels, as the pointer is.
fn read_input(c: &mut Cpu) {
    let m = c.mem();
    let ox = rc_hle::ddraw::margin() as i16;
    let hero = m.ld32(HERO);
    if ox == 0 || hero == 0 {
        return call_original(c, READ_INPUT);
    }
    let x = m.ld16(hero + 0x38);
    m.st16(hero + 0x38, (x as i16).wrapping_sub(ox) as u16);
    call_original(c, READ_INPUT);
    // Put back unless the function placed the hero anew.
    if m.ld16(hero + 0x38) == (x as i16).wrapping_sub(ox) as u16 {
        m.st16(hero + 0x38, x);
    }
}

fn wide_screen<R>(c: &mut Cpu, f: impl FnOnce(&mut Cpu) -> R) -> R {
    let m = c.mem();
    let ox = rc_hle::ddraw::margin();
    if ox == 0 {
        return f(c);
    }
    let (w, cx, clip) = (m.ld32(SCREEN_W), m.ld32(CENTER_X), m.ld32(CLIP_W));
    m.st32(SCREEN_W, w + 2 * ox);
    m.st32(CENTER_X, cx.wrapping_add(ox));
    m.st32(CLIP_W, clip + 2 * ox);
    let outer = NARROW_W.swap(w, std::sync::atomic::Ordering::Relaxed);
    let r = f(c);
    NARROW_W.store(outer, std::sync::atomic::Ordering::Relaxed);
    m.st32(SCREEN_W, w);
    m.st32(CENTER_X, m.ld32(CENTER_X).wrapping_sub(ox));
    m.st32(CLIP_W, clip);
    r
}

static PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn play_video(c: &mut Cpu) {
    use std::sync::atomic::Ordering;
    PLAYING.store(true, Ordering::Relaxed);
    call_original(c, PLAY_VIDEO);
    PLAYING.store(false, Ordering::Relaxed);
}

/// Where the video playing is on the screen (the game centres it): widescreen shows only it,
/// enlarged. The videos are of two kinds (RESEARCH.md, the videos in "The pause menu, …"): the
/// intro, the outro and the cut scenes are 640 × 368, nearly 16:9, and letterboxed by the game
/// on its 640 × 480 screen, so they may lose a little of their picture to fill the window; the
/// logos and the visions are 640 × 480, full frame, and are shown whole.
fn video_picture(m: Mem) -> Option<rc_hle::ddraw::Picture> {
    if !PLAYING.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let h = m.ld32(VIDEO);
    if h == 0 {
        return None;
    }
    let (w, ht) = (m.ld32(h), m.ld32(h + 4));
    let (gw, gh, _) = rc_hle::ddraw::display_mode();
    (w <= gw && ht <= gh).then(|| rc_hle::ddraw::Picture {
        x: (gw - w) / 2,
        y: (gh - ht) / 2,
        w,
        h: ht,
        letterboxed: ht < gh,
    })
}

fn camera(c: &mut Cpu) {
    wide_screen(c, |c| call_original(c, CAMERA));
}

/// The world's functions (`WORLD`), each on the wider screen.
macro_rules! world_fns {
    ($($name:ident = $i:expr),*) => {
        $(fn $name(c: &mut Cpu) {
            WORLD_DRAWN.store(true, std::sync::atomic::Ordering::Relaxed);
            wide_screen(c, |c| call_original(c, WORLD[$i]));
        })*
        const WORLD_FNS: [fn(&mut Cpu); WORLD.len()] = [$($name),*];
    };
}
world_fns!(
    w0 = 0,
    w1 = 1,
    w2 = 2,
    w3 = 3,
    w4 = 4,
    w5 = 5,
    w6 = 6,
    w7 = 7,
    w8 = 8
);

/// The offsets of a primitive's screen positions (x, then y: two i16), by its command byte
/// (`+0xf`). The primitives are PlayStation GPU packets after an 8-byte link: a colour and the
/// command, then the vertices, each with its texture coordinates (textured) and the next
/// vertex's colour before it (Gouraud).
fn vertex_offsets(code: u8) -> Vec<u32> {
    match code & 0xe0 {
        0x20 => {
            let n = if code & 0x08 != 0 { 4 } else { 3 };
            let stride = 4 + 4 * u32::from(code & 0x04 != 0) + 4 * u32::from(code & 0x10 != 0);
            (0..n).map(|i| 0x10 + i * stride).collect()
        }
        // Lines (not poly-lines, whose length the packet does not say).
        0x40 if code & 0x08 == 0 => {
            let stride = 4 + 4 * u32::from(code & 0x10 != 0);
            vec![0x10, 0x10 + stride]
        }
        0x60 => vec![0x10],
        _ => Vec::new(),
    }
}

/// Draw the frame's ordering table (from its last node, the argument) on the whole width: the
/// world as it was put there (on the wider screen), the other buckets' primitives moved into
/// the middle for the draw and back after it.
fn draw_split(c: &mut Cpu) {
    let m = c.mem();
    let last = arg(c, 0);
    let ox = rc_hle::ddraw::margin();
    if ox == 0 {
        call_original(c, DRAW_OT);
        return;
    }
    rc_hle::profile::begin(rc_hle::profile::Section::OtWalk);
    let first = last - 0x3ff * 16;
    let gw = m.ld32(SCREEN_W);
    // Whether the world is drawn this frame (not in the load and save book): `ui_shift`.
    WORLD_SHOWN.store(
        WORLD_DRAWN.swap(false, std::sync::atomic::Ordering::Relaxed),
        std::sync::atomic::Ordering::Relaxed,
    );
    // What changes for the draw (an address and its value before), put back after it.
    let mut changed: Vec<(u32, u8)> = Vec::new();
    let mut set = |a: u32, v: u8| {
        changed.push((a, m.ld8(a)));
        m.st8(a, v);
    };
    let mut node = m.ld32(last);
    let mut bucket = 0x3ff;
    let mut guard = 0;
    while node != 0 && guard < 1 << 20 {
        if (first..=last).contains(&node) {
            bucket = (node - first) / 16;
        } else if bucket == 0x3ff {
            // The background: quads of a dark gradient over the 640 pixels, drawn first and
            // setting the depth (the game clears it no other way). Over the whole width, and
            // black: beyond the 640 pixels it would show where the game never shows anything
            // (beside a room's black surroundings).
            let code = m.ld8(node + 0xf);
            let offsets = vertex_offsets(code);
            let xs: Vec<i16> = offsets.iter().map(|&o| m.ld16(node + o) as i16).collect();
            if xs.iter().all(|x| (0..=gw as i16).contains(x)) {
                // A rectangle (the input page's dark tile) by its width too: otherwise what
                // lies beyond it is never cleared and keeps old pictures.
                if let Some((at, w)) = rect_width(m, node, code) {
                    let wide = (w * (gw + 2 * ox) as i32 / gw as i32) as u16;
                    set(node + at, wide as u8);
                    set(node + at + 1, (wide >> 8) as u8);
                }
                for (o, x) in offsets.into_iter().zip(xs) {
                    let wide = (i32::from(x) * (gw + 2 * ox) as i32 / gw as i32) as u16;
                    set(node + o, wide as u8);
                    set(node + o + 1, (wide >> 8) as u8);
                    // The vertex's colour before it (the first one's is the command's word).
                    if code & 0x10 != 0 || o == 0x10 {
                        for b in 0..3 {
                            set(node + o - 4 + b, 0);
                        }
                    }
                }
            }
        } else if bucket < WORLD_BUCKET {
            let code = m.ld8(node + 0xf);
            let offsets = vertex_offsets(code);
            if offsets.is_empty() {
                node = m.ld32(node);
                guard += 1;
                continue;
            }
            let xs: Vec<i32> = offsets
                .iter()
                .map(|&o| i32::from(m.ld16(node + o) as i16))
                .collect();
            let width = rect_width(m, node, code);
            let (lo, hi) = (
                *xs.iter().min().expect("a vertex"),
                *xs.iter().max().expect("a vertex") + width.map_or(0, |(_, w)| w),
            );
            let gw = gw as i32;
            let ox = ox as i32;
            if lo <= 0 && hi >= gw {
                // Over the whole 640 pixels (a cut scene's black bars, a fade): over the whole
                // width.
                let wide = |v: i32| v * (gw + 2 * ox) / gw;
                for (&o, &x) in offsets.iter().zip(&xs) {
                    let v = wide(x) as u16;
                    set(node + o, v as u8);
                    set(node + o + 1, (v >> 8) as u8);
                }
                if let Some((at, w)) = width {
                    let v = (wide(xs[0] + w) - wide(xs[0])) as u16;
                    set(node + at, v as u8);
                    set(node + at + 1, (v >> 8) as u8);
                }
            } else if let Some((at, w)) = width.filter(|_| {
                // The fade from black when a level opens (an iris): black rectangles in bucket
                // 0 around the opening; those at the 640 pixels' left or right edge reach the
                // screen's edge, or the margins show the world beside a black 4:3 box.
                bucket == 0
                    && code & 0xfc == 0x60
                    && m.ld32(node + 0xc) & 0xff_ffff == 0
                    && (lo <= 0 || hi >= gw)
            }) {
                let x = if lo <= 0 { xs[0] } else { xs[0] + ox };
                let v = (w + ox) as u16;
                set(node + at, v as u8);
                set(node + at + 1, (v >> 8) as u8);
                set(node + offsets[0], x as u8);
                set(node + offsets[0] + 1, (x >> 8) as u8);
            } else {
                let top = offsets
                    .iter()
                    .map(|&o| i32::from(m.ld16(node + o + 2) as i16))
                    .min()
                    .unwrap_or(0);
                let shift = ui_shift(m, bucket, (lo + hi) / 2, top, gw, ox);
                for (&o, &x) in offsets.iter().zip(&xs) {
                    let v = (x + shift) as u16;
                    set(node + o, v as u8);
                    set(node + o + 1, (v >> 8) as u8);
                }
            }
        }
        node = m.ld32(node);
        guard += 1;
    }
    rc_hle::profile::end();
    // The renderer drops a primitive wholly beyond the screen's size: the wider screen's.
    wide_screen(c, |c| {
        rc_hle::d3d::on_whole_width(|| call_original(c, DRAW_OT))
    });
    for (a, v) in changed.into_iter().rev() {
        m.st8(a, v);
    }
}

/// A rectangle primitive's width: where it is (offset) and how much (none for other kinds).
fn rect_width(m: Mem, node: u32, code: u8) -> Option<(u32, i32)> {
    match code & 0xfc {
        0x60 => Some((0x14, i32::from(m.ld16(node + 0x14)))),
        0x64 => Some((0x18, i32::from(m.ld16(node + 0x18)))),
        _ => None,
    }
}

/// How far an interface primitive of `bucket` (its middle at `centre`, its top at `top`)
/// moves on the wider screen: into the middle (`ox`), or to an edge. The life and mana tubes
/// (buckets 12 to 15, in the bottom left corner) and the coins (the same buckets, in the top
/// left corner, shown in the menus) keep to the left edge while the world is drawn (a menu, a
/// dialogue over it); the pause menu's title shares the buckets, the load and save
/// book draws its question there. The character
/// menu's buckets (2 to 11, 16 to 24) above its description (y 340) go to the edge they are
/// nearer to, while it is open (state 0x40) and while it slides out (the game is played again
/// then): left of the middle to the left edge, right of it to the right one. Its description
/// (with the key pictures in its text), a dialogue, the pause menu, the map stay in the middle
/// (RESEARCH.md, "Widescreen").
pub(crate) fn ui_shift(m: Mem, bucket: u32, centre: i32, top: i32, gw: i32, ox: i32) -> i32 {
    // The tubes and the coins: their buckets' bottom and top left corners (the pause menu's
    // title shares them).
    if (12..=15).contains(&bucket)
        && centre < 140
        && !(60..=300).contains(&top)
        && WORLD_SHOWN.load(std::sync::atomic::Ordering::Relaxed)
    {
        return 0;
    }
    let state = m.ld32(PAUSE_STATE);
    let flags = m.ld32(MENU_FLAGS);
    let closing = MENU_SINCE.load(std::sync::atomic::Ordering::Relaxed) < MENU_CLOSING;
    let menu = state == 0x40 || (closing && state == 0x800 && flags == 0);
    let split = matches!(bucket, 2..=11 | 16..=24) && top < 340;
    if menu && split {
        return if centre < gw / 2 { 0 } else { 2 * ox };
    }
    ox
}

/// Whether the interface is split to the edges now (the character menu, `ui_shift`): kept per
/// frame for the pointer, which input moves between frames.
/// Frames since the character menu was last open (its state 0x40), and how long it slides
/// out after it is closed (the game played again meanwhile): its parts keep to the edges.
static MENU_SINCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
const MENU_CLOSING: u32 = 40;

static SPLIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How far a point of the interface at the game's `x` is moved on the wider screen.
fn game_to_screen_shift(x: i32) -> i32 {
    let ox = rc_hle::ddraw::margin() as i32;
    if !SPLIT.load(std::sync::atomic::Ordering::Relaxed) {
        return ox;
    }
    if x < GAME_W / 2 {
        0
    } else {
        2 * ox
    }
}

/// The game's x of a point of the screen (margins included): the inverse of
/// `game_to_screen_shift` (in the split interface's gap: as if in the middle).
fn screen_to_game(x: i32) -> i32 {
    let ox = rc_hle::ddraw::margin() as i32;
    if !SPLIT.load(std::sync::atomic::Ordering::Relaxed) {
        return x - ox;
    }
    if x < GAME_W / 2 {
        x
    } else if x >= GAME_W / 2 + 2 * ox {
        x - 2 * ox
    } else {
        x - ox
    }
}

/// The interface's width in the game's pixels.
const GAME_W: i32 = 640;

/// Two empty nodes of ours for the pointer's chain.
fn pointer_chain(m: Mem) -> u32 {
    static NODES: Mutex<u32> = Mutex::new(0);
    let mut n = NODES.lock().expect("nodes");
    if *n == 0 {
        *n = rc_runtime::vm::heap_alloc(m, 32, true);
    }
    *n
}

/// Whether a draw adds light (blending on, source and destination factor one): the pointer's
/// glow, which a system cursor cannot show (it can only cover what is below it).
fn adds_light(d: &rc_hle::d3d::Draw) -> bool {
    let rs = |k: u32| d.state.render.get(&k).copied().unwrap_or(0);
    const ALPHABLENDENABLE: u32 = 27;
    const SRCBLEND: u32 = 19;
    const DESTBLEND: u32 = 20;
    const ONE: u32 = 2;
    rs(ALPHABLENDENABLE) != 0 && rs(SRCBLEND) == ONE && rs(DESTBLEND) == ONE
}

/// The pointer's draws as a cursor image with straight alpha, without its glow. Rendered
/// twice, over black and over white: how much of the background still shows is the alpha, the
/// colour over black divided by it the colour.
fn render_pointer(
    m: Mem,
    draws: &[rc_hle::d3d::Draw],
    x: i32,
    y: i32,
) -> Option<rc_platform::cursor::Image> {
    let draws: Vec<_> = draws.iter().filter(|d| !adds_light(d)).collect();
    if draws.is_empty() {
        return None;
    }
    let (sw, sh, _) = rc_hle::ddraw::display_mode();
    let (color, depth) = {
        let mut b = POINTER_BUFFERS.lock().expect("pointer buffers");
        if b.is_none_or(|b| (b.0, b.1) != (sw, sh)) {
            let color = rc_runtime::vm::heap_alloc(m, sw * sh * 4, true);
            let depth = rc_runtime::vm::heap_alloc(m, sw * sh * 4, true);
            *b = Some((sw, sh, color, depth));
        }
        let b = b.expect("made above");
        (b.2, b.3)
    };
    let info = rc_hle::ddraw::SurfaceInfo {
        buf: color,
        width: sw,
        height: sh,
        pitch: sw * 4,
        bpp: 32,
        masks: [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0],
        src_key: None,
    };
    let z = rc_hle::ddraw::SurfaceInfo {
        buf: depth,
        pitch: sw * 4,
        bpp: 32,
        masks: [0; 4],
        ..info
    };
    // The box around the mouse position, on the screen.
    let half = POINTER_BOX / 2;
    let x0 = (x - half).clamp(0, sw as i32) as u32;
    let y0 = (y - half).clamp(0, sh as i32) as u32;
    let x1 = (x + half).clamp(0, sw as i32) as u32;
    let y1 = (y + half).clamp(0, sh as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (bw, bh) = (x1 - x0, y1 - y0);
    let pass = |background: u32| -> Vec<u8> {
        let row: Vec<u8> = background.to_le_bytes().repeat(bw as usize);
        let zrow = vec![0xffu8; 4 * bw as usize];
        for r in y0..y1 {
            m.write(color + (r * sw + x0) * 4, &row);
            m.write(depth + (r * sw + x0) * 4, &zrow);
        }
        for d in &draws {
            rc_hle::raster::draw_into(
                m,
                &d.state,
                info,
                Some(z),
                d.primitive,
                d.fvf,
                &d.vertices,
                &d.indices,
            );
        }
        let mut out = vec![0u8; (bw * bh * 4) as usize];
        for r in 0..bh {
            let at = ((r * bw) * 4) as usize;
            m.read(
                color + ((y0 + r) * sw + x0) * 4,
                &mut out[at..at + 4 * bw as usize],
            );
        }
        out
    };
    let black = pass(0);
    let white = pass(0x00ff_ffff);

    // BGRA in memory (masks above) → straight-alpha RGBA, and the bounds of what is visible.
    let mut rgba = vec![0u8; (bw * bh * 4) as usize];
    let (mut lx, mut ly, mut hx, mut hy) = (bw, bh, 0, 0);
    for i in 0..(bw * bh) as usize {
        let b = [black[4 * i + 2], black[4 * i + 1], black[4 * i]];
        let w = [white[4 * i + 2], white[4 * i + 1], white[4 * i]];
        let a = (0..3)
            .map(|k| 255 - i32::from(w[k]) + i32::from(b[k]))
            .max()
            .unwrap_or(0)
            .clamp(0, 255);
        if a == 0 {
            continue;
        }
        for k in 0..3 {
            rgba[4 * i + k] = (i32::from(b[k]) * 255 / a).min(255) as u8;
        }
        rgba[4 * i + 3] = a as u8;
        let (px, py) = ((i as u32) % bw, (i as u32) / bw);
        (lx, ly, hx, hy) = (lx.min(px), ly.min(py), hx.max(px), hy.max(py));
    }
    // The mouse position inside the image too: it is the hotspot.
    let (hot_x, hot_y) = ((x - x0 as i32).max(0) as u32, (y - y0 as i32).max(0) as u32);
    let (lx, ly) = (lx.min(hot_x), ly.min(hot_y));
    let (hx, hy) = (hx.max(hot_x).min(bw - 1), hy.max(hot_y).min(bh - 1));
    let (w, h) = (hx - lx + 1, hy - ly + 1);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for r in ly..=hy {
        let at = 4 * (r * bw + lx) as usize;
        out.extend_from_slice(&rgba[at..at + 4 * w as usize]);
    }
    Some(rc_platform::cursor::Image {
        width: w,
        height: h,
        hotspot: (hot_x - lx, hot_y - ly),
        rgba: out,
        scale: 1,
    })
}

/// With "smooth textures" the pointer as the textures: enlarged four times by the same scaler
/// and sharpened (`rc_hle::upscale`). Its sway repeats, so the images made are kept (by their
/// look) and seldom made again.
fn smooth_pointer(image: rc_platform::cursor::Image) -> rc_platform::cursor::Image {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    static MADE: Mutex<Option<HashMap<u64, rc_platform::cursor::Image>>> = Mutex::new(None);
    if !rc_platform::render::smooth_textures() || image.width > 128 || image.height > 128 {
        return image;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    image.hash(&mut h);
    let key = h.finish();
    let mut made = MADE.lock().expect("pointers");
    let map = made.get_or_insert_with(HashMap::new);
    if let Some(i) = map.get(&key) {
        return i.clone();
    }
    let (w, h) = (image.width as usize, image.height as usize);
    let texels: Vec<[u8; 4]> = image.rgba.as_chunks::<4>().0.to_vec();
    let big = rc_hle::upscale::scale4x(&texels, w, h);
    let smooth = rc_platform::cursor::Image {
        width: 4 * image.width,
        height: 4 * image.height,
        // The middle of the hotspot's pixel.
        hotspot: (4 * image.hotspot.0 + 2, 4 * image.hotspot.1 + 2),
        rgba: big.into_iter().flatten().collect(),
        scale: 4,
    };
    if map.len() >= 256 {
        map.clear();
    }
    map.insert(key, smooth.clone());
    smooth
}

/// Install the hooks (the addresses `game.toml` routes through the dispatcher).
pub fn register() {
    load_settings();
    hook(TEXT, text);
    hook(MENU_OPEN, menu_open);
    hook(PAUSE_VALUE, pause_value);
    hook(PAUSE_CHOSEN, pause_chosen);
    hook(OPTIONS_SETUP, options_setup);
    hook(OPTIONS_FRAME, options_frame);
    hook(OPTIONS_ENTRY, options_entry);
    hook(DRAW_OBJECT, draw_object);
    hook(DRAW_OT, draw_ot);
    hook(CAMERA, camera);
    hook(SET_TILE_RADIUS, set_tile_radius);
    hook(ASKS_640[0], asks_640_0);
    hook(ASKS_640[1], asks_640_1);
    hook(ASKS_640[2], asks_640_2);
    hook(READ_INPUT, read_input);
    hook(PLAY_VIDEO, play_video);
    hook(DRAW_TEXT, draw_text);
    hook(crate::controls::CONFIG_PAGE, crate::controls::config_page);
    hook(
        crate::controls::SET_PAD_TABLE,
        crate::controls::set_pad_table,
    );
    rc_hle::ddraw::set_picture_fn(video_picture);
    rc_hle::ddraw::set_overlay_fn(crate::controls::overlay);
    rc_hle::set_cursor_map(screen_to_game);
    for (f, w) in WORLD.iter().zip(WORLD_FNS) {
        hook(*f, w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_in_every_language() {
        for (k, t) in TEXTS {
            for (lang, s) in t.iter().enumerate() {
                assert_eq!(text_for(k.as_bytes(), lang as u32), Some(*s));
                assert!(!ansi(s).contains(&b'?'), "{s} in Windows-1252");
                assert!(ansi(s).len() < STR_TEXT_SIZE as usize, "{s} fits");
            }
        }
        for l in LANGUAGES {
            assert!(!ansi(l).contains(&b'?'), "{l} in Windows-1252");
        }
        let keys: usize = TEXTS.iter().map(|(k, _)| k.len() + 1).sum();
        assert!(keys <= STR_KEYS_SIZE as usize);
        assert_eq!(text_for(b"RC_GRAPHICS", 9), Some("Configure graphics"));
        assert_eq!(text_for(b"OPT_OPTIONS", 0), None);
    }

    #[test]
    fn packet_vertices() {
        // Gouraud line, textured quad, Gouraud textured quad and triangle, Gouraud quad.
        assert_eq!(vertex_offsets(0x52), [0x10, 0x18]);
        assert_eq!(vertex_offsets(0x2f), [0x10, 0x18, 0x20, 0x28]);
        assert_eq!(vertex_offsets(0x3c), [0x10, 0x1c, 0x28, 0x34]);
        assert_eq!(vertex_offsets(0x34), [0x10, 0x1c, 0x28]);
        assert_eq!(vertex_offsets(0x38), [0x10, 0x18, 0x20, 0x28]);
        assert_eq!(vertex_offsets(0x20), [0x10, 0x14, 0x18]);
        // Rectangles: their corner; the game's own packets (0x80): none.
        assert_eq!(vertex_offsets(0x65), [0x10]);
        assert!(vertex_offsets(0x80).is_empty());
        assert!(vertex_offsets(0x4a).is_empty());
    }

    #[test]
    fn entries_around_ours() {
        assert_eq!(entry(2), Entry::Game(2));
        assert_eq!(entry(OUR_INDEX), Entry::Ours(Page::Graphics));
        assert_eq!(entry(OUR_INDEX + 1), Entry::Ours(Page::Refine));
        assert_eq!(entry(OUR_INDEX + 2), Entry::Ours(Page::Info));
        assert_eq!(entry(OUR_INDEX + 3), Entry::Game(3));
        assert_eq!(our_index(3), OUR_INDEX + 3);
        assert_eq!(our_index(2), 2);
    }
}
