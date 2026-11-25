# Development

## Requirements

- Rust stable (`rust-toolchain.toml` installs `rustfmt`, `clippy` and the `wasm32-unknown-unknown` target).
- Nothing else: no C compiler, no SDL. CMake (3.25) and Ninja only for the optional IDE front end below. On Linux Bevy needs the usual system libraries (Vulkan or GL driver,
  X11 or Wayland, ALSA or PulseAudio/PipeWire, udev for gamepads) and their development packages to build.
- For the difftests (M3): the `i686-unknown-linux-gnu` target.
- For the browser build (M8): `wasm-bindgen-cli` matching the crate version.

## Inputs

Put the media into `orig/` (git-ignored), or let the recompiler download them (from M1):

```sh
cargo run -p rc-cli --release -- download --dest orig technomage de   # archive.org, checked by SHA-256
cargo run -p rc-cli --release -- extract orig/2004-03.BIN               # → orig/technomage/cd-de/
```

## Build and run

```sh
cargo run -p rc-cli --release -- lift technomage cd-de   # → generated/technomage/ (16 shard crates)
cargo build --release -p technomage                      # about 7 minutes; dev builds about 5
target/release/technomage --game-dir orig/technomage/cd-de
```

The English variant runs the same way from `orig/technomage/cd-en` (`recomp extract` puts the German
executable and `TECHMAGE.CFG` with `LANGUAGE=1` there). A build without lifted code (`RC_NO_GENERATED=1`) is
the template the converter puts its Cranelift code beside; it runs a package's `data/code-<target>-….bin`.

### Options of the program (environment)

| Variable | Effect |
|---|---|
| `RC_GAME_DIR` | the installed game (same as `--game-dir`) |
| `RC_USER_DIR` | where saves and `TechnoMage.ini` go (default: the platform's data directory) |
| `RC_HEADLESS=1` | no window (same as `--headless`); frames only for screenshots |
| `RC_RENDERER=soft` | Direct3D in the software rasterizer instead of wgpu |
| `RC_FRAMES=n` | end after `n` presented frames |
| `RC_SCREENSHOT=file.ppm`, `RC_SCREENSHOT_FRAME=n` | write frame `n` (default: the first) |
| `RC_INPUT="frame:key …"` | scripted input: `esc enter space ctrl z x up down left right click@x,y` |
| `RC_CLOCK=virtual` | the guest's clock advances by frames (reproducible runs) |
| `RC_TRACE=imports,files,fps,debug` | trace import calls, file opens, frame rate and where its time goes, `OutputDebugString` |

A scripted run to the first room of a new game:

```sh
RC_HEADLESS=1 RC_INPUT="20:esc 60:esc 100:esc 140:esc 180:esc 220:esc 300:enter 360:esc 420:esc 480:esc 540:esc 600:esc 900:ctrl 960:ctrl" \
  RC_FRAMES=1100 RC_SCREENSHOT=room.ppm RC_SCREENSHOT_FRAME=1090 RC_TRACE=fps target/release/technomage --game-dir orig/technomage/cd-de
```

## The project's changes to the game

`games/technomage/src/patches.rs` (also compiled into the browser player) wraps functions of the game that
`[binary.patch] hooks` in `game.toml` lists: the lifter routes calls to them through the dispatcher, where
`patches::register` installs the wrappers, and a wrapper runs the game's own function with
`dispatch::call_original`. Changing the list needs a new lift. What the hooks touch is in RESEARCH.md ("The pause
menu, the options panel and the texts"). So far two entries in the pause menu (its box grows by their lines), each
opening the options panel with lines of its own: "Configure graphics" (the render resolution and the "native
resolution" switch, kept in `TechnoMageRecomp.cfg` in the user directory; `rc_platform::render`, the HLE's shadows
of the screen's surfaces in `d3d.rs`) and "Show information" (this program's version, build time and commit, the
platform, the game's version and language). Their texts are in a table in the game's six languages. The build time
and commit come from `games/technomage/build_info.rs`, which both build scripts include (`SOURCE_DATE_EPOCH` fixes
the time; `-dirty` marks uncommitted changes). With a window the mouse pointer is a hardware cursor (RESEARCH.md,
"The mouse pointer", "The ordering table"; `rc_platform::cursor`): `RC_TRACE=cursor` counts its images, and
`RC_SCREENSHOT` also writes the cursor shown then as `<file>.cursor.pam` (RGBA).

## Convert (what players run)

```sh
RC_NO_GENERATED=1 CARGO_TARGET_DIR=target/template cargo build --release -p technomage
cargo run -p rc-cli --release -- template --game technomage --system linux \
  --shell target/template/release/technomage -o templates/TechnoMage-linux.zip
cargo run -p rc-cli --release -- convert orig/2004-03.BIN --templates templates --system linux
```

`convert` reads the medium (and its supplements beside it), lifts and compiles the code with Cranelift, and
writes a zip that runs on its own: the program, `data/game/` (the installed files and the CD's folders) and
the code.

## The browser (converter page and player)

```sh
tools/build-web out/site templates            # rc-web and technomage-web for wasm32, the page; templates optional
python3 -m http.server -d out/site 8000       # then http://localhost:8000/
```

The page converts in a worker and stores the game in the browser (OPFS); `play.html` plays it. Before it starts,
`play.html` has the worker make the game's code again from the stored files when the site's converter makes
different code (`code_version`; a new version, new hooks). The game's mouse pointer is the canvas's cursor there
(`cursor_js`). Settings in the player's address are the runtime's (`?RC_TRACE=fps`, `?RC_RENDERER=soft`).

Without a browser, the player runs in Node (software rendering) or in headless Firefox (WebGL2) on a game
converted for the browser (a directory with `game/…` and `code.wasm`):

```sh
cargo build --release --target wasm32-unknown-unknown -p technomage-web
RC_FRAMES=1100 RC_INPUT="…" RC_SCREENSHOT=room.ppm RC_SCREENSHOT_FRAME=1090 \
  node --experimental-wasm-jspi tools/play-node.mjs \
  target/wasm32-unknown-unknown/release/technomage_web.wasm GAME_DIR
tools/web-test/run GAME_DIR out "RC_FRAMES=1100&RC_TRACE=fps&shots=1090&RC_INPUT=…"
```

Node needs version 24 (JSPI behind `--experimental-wasm-jspi`), Firefox 139 or later. `tools/web-test/flow.html`
runs the page's own flow from a built site: the release from `orig/` converted by `worker.js` into the browser's
storage, then `play.html` started in a frame (serve the site with `tools/web-test/serve.py SITE GAME OUT PORT`).

## VS Code (CMake)

`CMakeLists.txt` is a front end over cargo for IDEs; it compiles nothing itself (`LANGUAGES NONE`) and the
commands above stay the reference. With the CMake Tools and C/C++ extensions (`.vscode/extensions.json`),
VS Code picks a preset from `CMakePresets.json`, and `.vscode/` has the tasks and debug configurations:

| Preset | Cargo profile | For |
|---|---|---|
| `release` | `release` (optimised, line tables) | playing, profiling, stepping by line |
| `dev` | `dev` (full debug information; the lifted shards unoptimised) | debugging the runtime and HLE |
| `release-en` | `release` | the English game (`orig/technomage/cd-en`) |
| `template` | `release`, `RC_NO_GENERATED=1`, `target/template/` | the converter's shell template |

| Target | Does |
|---|---|
| `technomage` (default) | `cargo build -p technomage`; lifts first when `generated/` has no lifted code yet |
| `lift` | `recomp lift RC_GAME_DIR` again (writes only the files that changed) |
| `recomp` | the recompiler, always `--release` |
| `inputs` | `recomp download` and `recomp extract` for `RC_LANGUAGE` into `orig/` |
| `run`, `web` | play `RC_GAME_DIR`; `tools/build-web out/site` |
| `audit`, `fmt`, `clippy`, `cargo-test` | the checks, also CTest tests (`ctest --preset release`) |

The builds land in cargo's `target/` as usual and are linked (copied on Windows) into `build/<preset>/bin/`, which
the debug configurations start: the game in German or English, the scripted run to the first room, `recomp
lift`, and attaching to a running game. They use gdb through `rust-gdb` on Linux (from rustup; VS Code must find
it on `PATH`) and the Visual Studio debugger on Windows. From a terminal:

```sh
cmake --preset release
cmake --build --preset release                 # lift if needed, build the game
cmake --build --preset release --target lift   # lift again after changing the recompiler
```

## Checks

```sh
tools/audit.sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```
