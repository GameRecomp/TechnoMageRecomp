// SPDX-License-Identifier: MIT
//! The TechnoMage player in a browser: the runtime and the HLE layer for
//! `wasm32-unknown-unknown`, driven by web/player.js.
//!
//! The page instantiates this module, then the game's code (the converter's WebAssembly
//! module, `code.wasm`), which shares this module's memory and function table and calls the
//! runtime through the helper table ([`rc_helpers`]). It hands the code's tables over
//! ([`rc_load_code`]) and runs [`rc_main`] as a JSPI `promising` export. The game's files are
//! the page's (`/game`, the user's writes in `/user`), see `rc_runtime::host`.
//!
//! Exports, all `extern "C"`:
//!
//! | export | what |
//! |---|---|
//! | `rc_alloc(n) -> ptr` | `n` bytes for the page to copy into |
//! | `rc_helpers() -> ptr` | the 11 helper table slots (u32 each) |
//! | `rc_load_code(ptr, len, table_base) -> i32` | the code module's `rc.code` section; 0 or an error |
//! | `rc_main() -> i32` | run the game (suspends); its exit code, or 2 on an error |
//! | `rc_thread_main(id)` | guest thread `id`'s body (suspends) |
//! | `rc_key(scan, vk, down)`, `rc_mouse(dx, dy)`, `rc_button(b, down)`, `rc_wheel(dz)`, `rc_cursor(x, y)`, `rc_pad(buttons, x, y)` | input |
//! | `rc_rumble()` | the gamepads' vibration (thousandths) |
//! | `rc_mix(ptr, n)` | `n` interleaved stereo f32 samples at 44.1 kHz into `ptr` |

/// The game project's changes, as in the native shell.
#[cfg(target_family = "wasm")]
#[path = "../../technomage/src/patches.rs"]
mod patches;

#[cfg(target_family = "wasm")]
#[path = "../../technomage/src/controls.rs"]
mod controls;

#[cfg(target_family = "wasm")]
#[path = "../../technomage/src/icons.rs"]
mod icons;

#[cfg(target_family = "wasm")]
mod web {
    use rc_platform::Input;
    use rc_runtime::host;
    use std::path::Path;

    #[link(wasm_import_module = "rc_js")]
    unsafe extern "C" {
        /// A frame: RGBA8, `width * height * 4` bytes at `ptr`.
        fn present_js(ptr: *const u8, width: u32, height: u32);
        /// A frame drawn on the player's WebGL canvas (`width × height`).
        fn present_gl_js(width: u32, height: u32);
        /// The mouse cursor: RGBA8 (straight alpha) at `ptr` with its hotspot, how many of its
        /// pixels make one of the frame's (thousandths: the image may be enlarged, the frame
        /// drawn larger than the game's screen) and a key that is the same for the same image
        /// (the page keeps what it made of one), or none (`ptr` 0).
        fn cursor_js(
            ptr: *const u8,
            width: u32,
            height: u32,
            hot_x: u32,
            hot_y: u32,
            scale_milli: u32,
            key: u32,
        );
    }

    /// What the page was last told of the cursor: its generation and whether one is shown.
    static CURSOR: std::sync::Mutex<(u64, bool)> = std::sync::Mutex::new((0, false));

    /// The last frame's size and the game's screen it shows: the page gives positions in the
    /// frame's pixels (larger with native resolution), the game takes them in its own.
    static SHOWN: std::sync::Mutex<((u32, u32), (u32, u32))> =
        std::sync::Mutex::new(((0, 0), (0, 0)));

    fn to_game(x: i32, y: i32) -> (i32, i32) {
        let ((w, h), (gw, gh)) = *SHOWN.lock().expect("shown");
        if w == 0 || h == 0 || (w, h) == (gw, gh) {
            return (x, y);
        }
        (
            (i64::from(x) * i64::from(gw) / i64::from(w)) as i32,
            (i64::from(y) * i64::from(gh) / i64::from(h)) as i32,
        )
    }

    fn present(f: &rc_platform::Frame) {
        *SHOWN.lock().expect("shown") = ((f.width, f.height), f.game);
        if f.gpu.is_some() {
            // SAFETY: a plain call.
            unsafe { present_gl_js(f.width, f.height) }
        } else {
            // SAFETY: the frame's pixels are valid for the call.
            unsafe { present_js(f.rgba.as_ptr(), f.width, f.height) }
        }
        let (generation, pointer) = rc_platform::cursor::current_at(f.number);
        let mut told = CURSOR.lock().expect("cursor");
        if *told == (generation, pointer.is_some()) {
            return;
        }
        *told = (generation, pointer.is_some());
        match pointer {
            // SAFETY: the image is valid for the call.
            Some(c) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                c.hash(&mut h);
                let key = h.finish();
                // SAFETY: the image is valid for the call.
                unsafe {
                    cursor_js(
                        c.rgba.as_ptr(),
                        c.width,
                        c.height,
                        c.hotspot.0,
                        c.hotspot.1,
                        (u64::from(c.scale.max(1)) * 1000 * u64::from(f.game.0.max(1))
                            / u64::from(f.width.max(1))) as u32,
                        (key ^ key >> 32) as u32,
                    )
                }
            }
            // SAFETY: a null image asks for none.
            None => unsafe { cursor_js(std::ptr::null(), 0, 0, 0, 0, 1000, 0) },
        }
    }

    /// The page shows the game's pointer as the mouse cursor (`on` not 0): the game stops
    /// drawing it into the frame (the project's patches). Without it (Node) the game draws it.
    #[no_mangle]
    pub extern "C" fn rc_hardware_cursor(on: u32) {
        rc_platform::cursor::set_enabled(on != 0);
    }

    #[no_mangle]
    pub extern "C" fn rc_alloc(n: usize) -> *mut u8 {
        Box::leak(vec![0u8; n].into_boxed_slice()).as_mut_ptr()
    }

    #[no_mangle]
    pub extern "C" fn rc_helpers() -> *const *const () {
        rc_runtime::native::helper_table().as_ptr()
    }

    /// # Safety
    ///
    /// `ptr` must hold `len` bytes (from [`rc_alloc`]).
    #[no_mangle]
    pub unsafe extern "C" fn rc_load_code(ptr: *const u8, len: usize, table_base: u32) -> i32 {
        // SAFETY: the caller's promise.
        let b = unsafe { std::slice::from_raw_parts(ptr, len) };
        match rc_runtime::native::load_wasm(b, table_base) {
            Ok(l) => {
                rc_runtime::log!(
                    "technomage: {} functions as WebAssembly (modules {})",
                    l.functions,
                    l.modules.join(", ")
                );
                0
            }
            Err(e) => {
                rc_runtime::log!("technomage: the game's code: {e}");
                1
            }
        }
    }

    /// The file in `dir` whose name matches `name` case-insensitively.
    fn find(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
        host::read_dir(dir)?
            .into_iter()
            .find(|n| n.eq_ignore_ascii_case(name))
            .map(|n| dir.join(n))
    }

    fn run() -> Result<i32, String> {
        let dir = Path::new("/game");
        let exe_path = find(dir, "TechnoMage.exe").ok_or("no TechnoMage.exe in the game")?;
        let exe = host::read(&exe_path).map_err(|e| format!("TechnoMage.exe: {e}"))?;
        rc_runtime::clock::init_from_env();
        rc_runtime::vfs::init(dir, Some("/user".into()));
        rc_hle::register_all();
        crate::patches::register();
        rc_platform::set_present_hook(present);
        if host::env("RC_TRACE").is_some_and(|v| v.contains("imports")) {
            rc_runtime::imports::set_trace(true);
        }
        let mem = rc_runtime::mem::GuestMemory::get(u64::from(rc_runtime::mem::GUEST_TOP)).mem();
        let mut dlls = Vec::new();
        if let Some(path) = find(dir, "binkw32.dll") {
            let data = host::read(&path).map_err(|e| format!("binkw32.dll: {e}"))?;
            let m = rc_runtime::process::map(mem, "binkw32.dll", &data, None, &[])
                .map_err(|e| e.to_string())?;
            if rc_runtime::dispatch::is_known(m.entry) {
                rc_runtime::process::add_module(m.clone());
                rc_runtime::log!("technomage: binkw32.dll at {:#010x}", m.base);
                dlls.push(m);
            }
        }
        let image = rc_runtime::process::image_of(&exe).map_err(|e| e.to_string())?;
        let module = rc_runtime::process::map(mem, "TechnoMage.exe", &image, None, &dlls)
            .map_err(|e| e.to_string())?;
        rc_runtime::vm::set_floor(module.base + module.size);
        rc_hle::kernel32::set_image_base(module.base);
        rc_runtime::process::add_module(module.clone());
        rc_runtime::threads::init_main();
        let mut cpu = rc_runtime::Cpu::new(mem);
        rc_runtime::process::main_thread(&mut cpu, module.base);
        for d in &dlls {
            let ok = rc_runtime::dispatch::call_guest(&mut cpu, d.entry, &[d.base, 1, 0]);
            if ok == 0 {
                return Err(format!("{} did not initialise", d.name));
            }
        }
        rc_runtime::dispatch::dispatch(&mut cpu, module.entry);
        Ok(cpu.r[rc_runtime::cpu::EAX] as i32)
    }

    #[no_mangle]
    pub extern "C" fn rc_main() -> i32 {
        std::panic::set_hook(Box::new(|p| host::log(&format!("technomage: {p}"))));
        match run() {
            Ok(c) => c,
            Err(e) => {
                rc_runtime::log!("technomage: {e}");
                2
            }
        }
    }

    #[no_mangle]
    pub extern "C" fn rc_thread_main(id: u32) {
        rc_runtime::threads::thread_main(id);
    }

    #[no_mangle]
    pub extern "C" fn rc_key(scan: u32, vk: u32, down: u32) {
        rc_platform::input(Input::Key {
            scan: scan as u8,
            vk,
            down: down != 0,
        });
    }

    #[no_mangle]
    pub extern "C" fn rc_mouse(dx: i32, dy: i32) {
        rc_platform::input(Input::MouseMotion { dx, dy });
    }

    #[no_mangle]
    pub extern "C" fn rc_button(button: u32, down: u32) {
        rc_platform::input(Input::MouseButton {
            button: button as u8,
            down: down != 0,
        });
    }

    #[no_mangle]
    pub extern "C" fn rc_wheel(dz: i32) {
        rc_platform::input(Input::Wheel { dz });
    }

    /// The size the page shows the picture in (device pixels): widescreen takes its proportions,
    /// native resolution its size.
    #[no_mangle]
    pub extern "C" fn rc_output(width: u32, height: u32) {
        rc_platform::render::set_output(width, height);
    }

    #[no_mangle]
    pub extern "C" fn rc_cursor(x: i32, y: i32) {
        let (x, y) = to_game(x, y);
        rc_platform::input(Input::Cursor { x, y });
    }

    /// Whose buttons the gamepad used has: 0 PlayStation, 1 Xbox, 2 Nintendo.
    #[no_mangle]
    pub extern "C" fn rc_pad_style(style: u32) {
        use rc_platform::pad::{set_style, Style};
        set_style(match style {
            0 => Style::PlayStation,
            2 => Style::Nintendo,
            _ => Style::Xbox,
        });
    }

    /// The gamepads' vibration the game asks for now, in thousandths (its force feedback).
    #[no_mangle]
    pub extern "C" fn rc_rumble() -> u32 {
        (rc_platform::rumble::strength() * 1000.0) as u32
    }

    /// The gamepads as the PlayStation pad: `rc_platform::pad`'s buttons, the direction
    /// (-32768 to 32767, y down).
    #[no_mangle]
    pub extern "C" fn rc_pad(buttons: u32, x: i32, y: i32) {
        rc_platform::input(Input::Pad {
            buttons: buttons as u16,
            x: x.clamp(-32768, 32767) as i16,
            y: y.clamp(-32768, 32767) as i16,
        });
    }

    /// # Safety
    ///
    /// `ptr` must hold `n` f32 (from [`rc_alloc`]).
    #[no_mangle]
    pub unsafe extern "C" fn rc_mix(ptr: *mut f32, n: usize) {
        // SAFETY: the caller's promise.
        let out = unsafe { std::slice::from_raw_parts_mut(ptr, n) };
        rc_platform::audio::mix(out);
    }
}
