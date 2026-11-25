// SPDX-License-Identifier: MIT
//! The game project's changes to TechnoMage (the `[binary.patch]` hooks in `game.toml`): a
//! "Configure graphics" entry in the pause menu that opens the options panel with one line,
//! the render resolution. The texts come from a table here in the game's language, served
//! through the game's own string lookup. Addresses are of version 1.04 (the only one lifted);
//! docs/RESEARCH.md ("The pause menu and the options panel") describes the structures.
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
/// Where our entry goes in it: after "Configure input devices".
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

/// The options panel's entries (0x24 bytes each, ended by a NULL key), five of them with the end.
const OPTIONS_TABLE: u32 = 0x0056_4c80;
const OPTIONS_ENTRY_SIZE: u32 = 0x24;
const OPTIONS_BYTES: usize = 5 * OPTIONS_ENTRY_SIZE as usize;
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
/// The screen size the game lays its menus out on, and a 4096-step sine table (i16 every 4 bytes).
const SCREEN_W: u32 = 0x00f5_b320;
const SCREEN_H: u32 = 0x00f5_b324;
const SINE: u32 = 0x0058_6a30;

// --- Our texts ---------------------------------------------------------------------------------

const KEY_ENTRY: &str = "OPT_GFXCONFIG";
const KEY_RESOLUTION: &str = "OPT_GFXRES";

/// Our texts by key, in the game's languages (GER, ENG, FRA, ITA, SPA, NL).
const TEXTS: &[(&str, [&str; 6])] = &[
    (
        KEY_ENTRY,
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
];

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
    /// The pause menu with our entry.
    menu: u32,
    /// Our keys, then a text buffer per key, then the resolution's text.
    strings: u32,
}

const STR_KEYS: u32 = 0;
const STR_TEXTS: u32 = 64;
const STR_TEXT_SIZE: u32 = 64;
const STR_VALUE: u32 = STR_TEXTS + STR_TEXT_SIZE * TEXTS.len() as u32;
const STR_BYTES: u32 = STR_VALUE + 32;

#[derive(Default)]
struct State {
    blocks: Option<Blocks>,
    /// The options panel's entries, title and selection while ours are in their place.
    saved: Option<(Vec<u8>, u32, u32)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    blocks: None,
    saved: None,
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

fn text(c: &mut Cpu) {
    let m = c.mem();
    let key = arg(c, 0);
    if key != 0 {
        let k = m.cstr(key, 32);
        if let Some(t) = text_for(&k, m.ld32(LANGUAGE)) {
            let b = blocks(m);
            let i = TEXTS
                .iter()
                .position(|(n, _)| n.as_bytes() == k)
                .unwrap_or(0) as u32;
            let at = b.strings + STR_TEXTS + STR_TEXT_SIZE * i;
            ret_cdecl(c, put_str(m, at, STR_TEXT_SIZE, &ansi(t)));
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

/// Our menu's entry `i` → the pause menu's (None: ours).
fn original_index(i: u32) -> Option<u32> {
    match i {
        _ if i < OUR_INDEX => Some(i),
        OUR_INDEX => None,
        _ => Some(i - 1),
    }
}

/// Copy the entries' state (all but the key) between the pause menu and ours.
fn sync(m: Mem, to_original: bool) {
    let Some(b) = STATE.lock().expect("patch state").blocks else {
        return;
    };
    for i in 0..=pause_len(m) {
        let ours = b.menu + MENU_ENTRY * if i < OUR_INDEX { i } else { i + 1 };
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
                // Like "Configure input devices": an action that closes the menu.
                let like = PAUSE_TABLE + MENU_ENTRY * 2;
                m.st32(to, key_addr(b, KEY_ENTRY));
                m.st32(to + 4, m.ld32(like + 4));
                m.st32(to + 8, 0);
                m.st32(to + 12, 1);
                to += MENU_ENTRY;
            }
            let from = PAUSE_TABLE + MENU_ENTRY * i;
            for w in 0..4 {
                m.st32(to + 4 * w, m.ld32(from + 4 * w));
            }
            to += MENU_ENTRY;
        }
        m.st32(c.r[ESP] + 4, b.menu);
        call_original(c, MENU_OPEN);
        // Its box was sized for the game's entries: one line more for ours.
        let line = u32::from(m.ld8(m.ld32(MENU_FONT) + FONT_LINE));
        m.st32(MENU_BOX_HEIGHT, m.ld32(MENU_BOX_HEIGHT) + line);
        return;
    }
    call_original(c, MENU_OPEN);
}

/// A pause-menu callback with an entry index first: ours, or the original's with its index.
fn pause_callback(c: &mut Cpu, f: u32, ours: fn(&mut Cpu)) {
    let open = STATE.lock().expect("patch state").blocks.map(|b| b.menu);
    if open.is_none() || c.mem().ld32(MENU_TABLE) != open.unwrap_or(0) {
        call_original(c, f); // not our menu (a build lifted without the hooks)
        return;
    }
    let i = arg(c, 0);
    match original_index(i) {
        None => ours(c),
        Some(j) => {
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
    pause_callback(c, PAUSE_VALUE, |c| ret_cdecl(c, 0));
}

fn pause_chosen(c: &mut Cpu) {
    pause_callback(c, PAUSE_CHOSEN, |c| {
        show_graphics(c);
        let m = c.mem();
        // As "Options" does: the panel slides in, the menu waits for it.
        m.st32(PAUSE_SEEN, u32::MAX);
        m.st32(PAUSE_STATE, (m.ld32(PAUSE_STATE) & !0x800) | 2);
        m.st32(MENU_FLAGS, m.ld32(MENU_FLAGS) | 2);
        ret_cdecl(c, 0);
    });
}

// --- The options panel as the graphics panel ------------------------------------------------------

/// Put our entry and title into the options panel (its own are kept until it is hidden).
fn show_graphics(c: &mut Cpu) {
    let m = c.mem();
    let b = blocks(m);
    {
        let mut s = STATE.lock().expect("patch state");
        if s.saved.is_none() {
            let mut entries = vec![0u8; OPTIONS_BYTES];
            m.read(OPTIONS_TABLE, &mut entries);
            s.saved = Some((entries, m.ld32(OPTIONS_TITLE), m.ld32(OPTIONS_SELECTED)));
        }
    }
    // Laid out as the first volume line; not selectable, no slider (drawn in options_entry).
    let e = OPTIONS_TABLE;
    let like = m.ld32(e + 4);
    m.write(e, &[0u8; 2 * OPTIONS_ENTRY_SIZE as usize]);
    m.st32(e, key_addr(b, KEY_RESOLUTION));
    m.st32(e + 4, like);
    m.st16(e + 8, (-60i16) as u16);
    m.st16(e + 0xc, 2);
    m.st16(e + 0x20, 0xffff);
    let label = call_guest(c, TEXT, &[key_addr(b, KEY_RESOLUTION)]);
    m.st32(e + 0x18, label);
    let title = call_guest(c, TEXT, &[key_addr(b, KEY_ENTRY)]);
    m.st32(OPTIONS_TITLE, title);
    m.st32(OPTIONS_SELECTED, e);
}

/// The options panel's own entries again.
fn restore_options(m: Mem) {
    let saved = STATE.lock().expect("patch state").saved.take();
    if let Some((entries, title, selected)) = saved {
        m.write(OPTIONS_TABLE, &entries);
        m.st32(OPTIONS_TITLE, title);
        m.st32(OPTIONS_SELECTED, selected);
    }
}

fn showing_graphics() -> bool {
    STATE.lock().expect("patch state").saved.is_some()
}

fn options_setup(c: &mut Cpu) {
    restore_options(c.mem()); // a level loads: the panel is set up from its own entries
    call_original(c, OPTIONS_SETUP);
}

fn options_frame(c: &mut Cpu) {
    call_original(c, OPTIONS_FRAME);
    let m = c.mem();
    if showing_graphics() && m.ld32(PAUSE_STATE) & 2 == 0 && m.ld32(OPTIONS_SLIDE) == 0 {
        restore_options(m); // slid out
    }
}

/// One entry of the options panel; ours is a line of text: the label, and the resolution where
/// a slider's number goes. Placed as the game places its entries (sliding in from the side).
fn options_entry(c: &mut Cpu) {
    let e = arg(c, 0);
    if e != OPTIONS_TABLE || !showing_graphics() {
        call_original(c, OPTIONS_ENTRY);
        return;
    }
    let m = c.mem();
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
    let value_x = w * x / 1000 - i32::from(m.ld8(metrics + 9));
    let label_x = i16_at(e + 8) * w / 1000 + value_x;
    let y = y * h / 1000;
    let (font, colour) = (m.ld32(OPTIONS_FONT), 0x80);

    let b = blocks(m);
    let (mw, mh, _) = rc_hle::ddraw::display_mode();
    let value = put_str(
        m,
        b.strings + STR_VALUE,
        32,
        format!("{mw} x {mh}").as_bytes(),
    );
    let label = m.ld32(e + 0x18);
    // The game aligns both to the right: the label before the number, the number where its
    // slider begins. The resolution is longer than a number: it goes where the slider is.
    for (t, at, align) in [
        (label, label_x, ALIGN_RIGHT),
        (value, value_x + w / 50, ALIGN_LEFT),
    ] {
        call_guest(
            c,
            DRAW_TEXT,
            &[
                font, t, at as u32, y as u32, 0x12, colour, colour, colour, 2, align,
            ],
        );
    }
    ret_cdecl(c, c.r[EAX]);
}

/// Install the hooks (the addresses `game.toml` routes through the dispatcher).
pub fn register() {
    hook(TEXT, text);
    hook(MENU_OPEN, menu_open);
    hook(PAUSE_VALUE, pause_value);
    hook(PAUSE_CHOSEN, pause_chosen);
    hook(OPTIONS_SETUP, options_setup);
    hook(OPTIONS_FRAME, options_frame);
    hook(OPTIONS_ENTRY, options_entry);
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
            }
        }
        assert_eq!(text_for(b"OPT_GFXCONFIG", 9), Some("Configure graphics"));
        assert_eq!(text_for(b"OPT_OPTIONS", 0), None);
    }

    #[test]
    fn indices_around_ours() {
        assert_eq!(original_index(2), Some(2));
        assert_eq!(original_index(OUR_INDEX), None);
        assert_eq!(original_index(4), Some(3));
    }
}
