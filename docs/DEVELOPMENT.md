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

The other languages the same way (`en`, `fr`, `it`, `es`); `extract` takes their medium (`TECHNOMAGE.mdf`,
`TECHNOMAGE.ISO`, `tecnomage.img`, `TechnoMage_PC_CD_2000_ES.tar.001` with its `.002` beside it) and the cover CD
beside it for the executable. The Spanish CD is a 7z inside a tar in two parts: its image is unpacked into memory
first (about a minute).

## Build and run

```sh
cargo run -p rc-cli --release -- lift orig/technomage/cd-de   # → generated/technomage/ (16 shard crates)
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
| `RC_INPUT="frame:key …"` | scripted input: `esc enter space ctrl alt z x m i s q tab f11 f12 up down left right click@x,y move@x,y` (`a-b:key` holds a key from frame a to b), the gamepad held four frames: `pad-square pad-triangle pad-cross pad-circle pad-l1 pad-l2 pad-r1 pad-r2 pad-select pad-start pad-left pad-right pad-up pad-down`, an on-screen button at `tap@x,y` |
| `RC_PAD_STYLE=playstation\|xbox\|nintendo`, `RC_TOUCH=1` | scripted runs: the gamepad's buttons as that maker's; touch as the input in use (the on-screen buttons shown) |
| `RC_RENDER_OUTPUT=WxH` | without a window: the window size native resolution and widescreen follow (scripted runs) |
| `RC_CLOCK=virtual` | the guest's clock advances by frames (reproducible runs; smooth textures and fonts are enlarged at once, not in the background) |
| `RC_TRACE=imports,files,fps,debug,rumble` | trace import calls, file opens, frame rate and where its time goes, `OutputDebugString`, the force feedback's strength |
| `RC_TRACE=profile` (`RC_PROFILE=file.csv`) | a frame profiler: per frame the time of each section (game code, Direct3D recording, the GPU batch's read, upload, wait and write-back, the picture with native resolution, presenting, copies, sleeps), one CSV line a frame (`hle::profile`) |
| `RC_GPU_RESIDENT=0` | every Direct3D batch read back into guest memory and the frame shown from there, as before the frame stayed on the GPU (to compare) |
| `RC_GPU_SYNC=1` | wait for the GPU at each frame's end (measuring its work with `RC_TRACE=profile`) |
| `RC_WINDOW_SHOT=frame:file.ppm` | the window's picture as Bevy rendered it, once that frame has been shown (under `xvfb-run` too, where a screen capture shows no Vulkan window) |
| `RC_FRAME_HASHES=file` | a line per frame shown: its size and a hash of its pixels; two scripted runs (virtual clock) before and after a change compared frame by frame show any change in the picture |

A scripted run to the first room of a new game:

```sh
RC_HEADLESS=1 RC_INPUT="20:esc 60:esc 100:esc 140:esc 180:esc 220:esc 300:enter 360:esc 420:esc 480:esc 540:esc 600:esc 900:ctrl 960:ctrl" \
  RC_FRAMES=1100 RC_SCREENSHOT=room.ppm RC_SCREENSHOT_FRAME=1090 RC_TRACE=fps target/release/technomage --game-dir orig/technomage/cd-de
```

## The project's changes to the game

`games/technomage/src/patches.rs` (also compiled into the browser player) wraps functions of the game that
`[binary.patch] hooks` in `game.toml` lists: the lifter routes calls to them through the dispatcher, where
`patches::register` installs the wrappers, and a wrapper runs the game's own function with
`dispatch::call_original`. Changing the list needs a new lift: the lift writes what it applied
(`lift-inputs.txt` beside the lifted code), a cargo build stops when that is not what `game.toml` has
(`rc_manifest::stale_lifts`), and the CMake build lifts again by itself. What the hooks touch is in RESEARCH.md ("The pause
menu, the options panel and the texts"). So far two entries in the pause menu (its box grows by their lines), each
opening the options panel with lines of its own: "Configure graphics" (the render resolution and the switches
"native resolution", "widescreen", "antialiasing" and "show frame rate", kept in `TechnoMageRecomp.cfg` in the
user directory; `rc_platform::render`, the HLE's shadows of the screen's surfaces in `d3d.rs`, the margins of the
screen's surfaces in `ddraw.rs` with the patches' world on the wider screen, multisampling in the GPU renderer,
`rc_platform::fps`) and "Show information" (this program's version, build time and commit, the
platform, the game's version and language). Their texts are in a table in the game's six languages. The build time
and commit come from `games/technomage/build_info.rs`, which both build scripts include (`SOURCE_DATE_EPOCH` fixes
the time; `-dirty` marks uncommitted changes). With a window the mouse pointer is a hardware cursor (RESEARCH.md,
"The mouse pointer", "The ordering table"; `rc_platform::cursor`): `RC_TRACE=cursor` counts its images (and how far the
nearest one made before differed), and
`RC_SCREENSHOT` also writes the cursor shown then as `<file>.cursor.pam` (RGBA).

## Saved games: `tmsave`

`tmsave` (`games/technomage-save`) reads, checks and changes saved games, every byte of them (the format:
RESEARCH.md, "Saved games"), for tests that need a game in a given state:

```sh
cargo build --release -p technomage-save
target/release/tmsave show 1                     # slot 1 of the user directory (RC_USER_DIR), or a file
target/release/tmsave set 1 items.GOLDCOIN=5000 bag[0]=HEAL_POTION items.HEAL_POTION=5 character.level=5 character.xp=51000
target/release/tmsave check 1                    # the game's rules: errors (exit 1) and warnings
target/release/tmsave export 1 -o s.json         # every field, lossless; import -o FILE [--base SAVE] back
target/release/tmsave diff 1 2                   # the fields that differ (also the level bitfields' bits)
target/release/tmsave schema character.          # offset, type, path, meaning
```

Item, world and level names come from the game's own `TechnoMage.exe` (`--exe`, `RC_GAME_DIR` or
`orig/technomage/cd-de`); nothing of the game is in the editor. Paths name the schema's fields (`items[51]` or
`items.GOLDCOIN`, `slots[4].door[1]`, `world.x66`); `set` takes numbers, the names a field knows and hex for byte
fields, writes in place (or `-o`) and warns of the rules the result breaks. A scripted run can load a slot
through the book: `… 300:down 330:enter 420:left 440:left 460:left 480:enter 520:enter 560:left 580:enter` after
the intro keys loads slot 1.

## Convert (what players run)

```sh
RC_NO_GENERATED=1 CARGO_TARGET_DIR=target/template cargo build --release -p technomage
curl -fsSL -o target/appimage-runtime \
  https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$(uname -m)
cargo run -p rc-cli --release -- template --game technomage --system linux \
  --shell target/template/release/technomage --runtime target/appimage-runtime \
  --icon tools/icons/technomage.svg -o templates/TechnoMage-linux.zip
cargo run -p rc-cli --release -- convert orig/2004-03.BIN --templates templates --system linux
```

`convert` reads the medium (and its supplements beside it), lifts and compiles the code with Cranelift, and
writes a package that runs on its own: the program, `data/game/` (the installed files and the CD's folders) and
the code; a zip, for Android an APK, and for Linux one AppImage holding everything (the converter page makes
the same). The AppImage is the AppImage runtime (type2-runtime, static, MIT; the template carries the one for
its machine) followed by a SquashFS image that `rc_convert::squashfs` writes itself, uncompressed (the game's
files are mostly compressed already, and a browser needs no compressor): `AppRun` (the shell, which finds
`data/` beside it), `technomage.desktop`, `technomage.svg`, `data/code-*.bin` and `data/game/`. The image's
tables are laid out from the file list first, so the files stream straight in. It needs FUSE to start;
without it `--appimage-extract-and-run`. A template made without `--runtime` gives a zip as before.

The systems are `windows`, `linux` (x86-64), `windows-arm64`, `linux-arm64`, `macos` and `android`. A template is
the shell built on that machine (CI builds the ARM64 ones on GitHub's `ubuntu-22.04-arm` and `windows-11-arm`
runners); the converter itself compiles the code for any of them on any machine (Cranelift's targets
`x86_64-sysv`, `x86_64-windows`, `arm64-linux`, `arm64-windows`, `arm64-apple`).

**Linux builds on the oldest Ubuntu runner.** A Linux program needs the glibc it was linked against or a
newer one: built on `ubuntu-latest` it refuses older systems ("GLIBC_2.xx not found"). The Pages and release
workflows build the Linux shells and the converter on the oldest Ubuntu GitHub still offers (`ubuntu-22.04`,
`ubuntu-22.04-arm`, glibc 2.35); when GitHub retires it, move to the next oldest, never to `ubuntu-latest`.

**Caches.** Every Rust job restores `~/.cargo` and `target/` with `Swatinem/rust-cache` (keyed by the toolchain,
the host and `Cargo.lock`). Only runs on `main` save them, so branches and pull requests do not crowd out the
10 GB the repository gets. The Pages and release builds of the shells share a key (`dist`), as do their
Android jobs (`android`): a version tag restores what the last Pages run on `main` built and saves nothing.

**Superseded runs.** A new CI run cancels the one still running for the same branch or pull request
(`concurrency`, keyed by the workflow and the ref); a new Pages run cancels the one before. Release runs are
never cancelled.

### Android

```sh
export ANDROID_HOME=~/Android/Sdk ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
tools/build-android build/TechnoMage-shell.apk          # arm64-v8a and x86_64, unsigned
cargo run -p rc-cli --release -- template --game technomage --system android \
  --shell build/TechnoMage-shell.apk -o templates/TechnoMage-android.zip
cargo run -p rc-cli --release -- convert orig/2004-03.BIN --templates templates --system android
```

The shell is the same program built as `libtechnomage.so` (`cargo rustc --crate-type cdylib`, without lifted
code) in Android's `NativeActivity`; `tools/build-android` needs the SDK's `platforms/android-*` and
`build-tools/*` (aapt2) and the NDK (its clang links), no JDK. `convert` writes `TechnoMage.apk`: the shell,
the code for arm64 and x86-64 and the game's files under `assets/data/`, signed with the converter's built-in
key. On its first start the app unpacks them into its storage (`rc_platform::android`), and again when an
update brings a different `files.txt`. The program's log (stderr) goes to `log.txt` in
`Android/data/io.github.gamerecomp.technomagerecomp/files/`; `adb logcat` has Bevy's. Settings and saves are
in the app's private storage.

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

With `cfg=smooth_fonts%3D1%3Bwidescreen%3D1` the test page starts with those lines in the project's settings file.
With `hashes=1` the test page writes a hash of every frame to `out/hashes` (a line a frame); a run with
`RC_GPU_RESIDENT=0&RC_CLOCK=virtual` beside one without compares the WebGL2 path kept on the GPU with the read-back one.

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
| `technomage` (default) | `cargo build -p technomage`; lifts first when the executables, `game.toml` or the recompiler changed (only the changed files are written, so only those shards are compiled again) |
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
cmake --build --preset release --target lift   # lift again
```

## Checks

```sh
tools/audit.sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```
