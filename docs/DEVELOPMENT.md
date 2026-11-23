# Development

## Requirements

- Rust stable (`rust-toolchain.toml` installs `rustfmt`, `clippy` and the `wasm32-unknown-unknown` target).
- Nothing else: no C compiler, no CMake, no SDL. On Linux Bevy needs the usual system libraries (Vulkan or GL driver,
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
| `RC_SCREENSHOT=file.ppm`, `RC_SCREENSHOT_FRAME=n` | write frame `n` (default: the last) |
| `RC_INPUT="frame:key …"` | scripted input: `esc enter space ctrl z x up down left right click@x,y` |
| `RC_CLOCK=virtual` | the guest's clock advances by frames (reproducible runs) |
| `RC_TRACE=imports,files,fps,debug` | trace import calls, file opens, frame rate and where its time goes, `OutputDebugString` |

A scripted run to the first room of a new game:

```sh
RC_HEADLESS=1 RC_INPUT="20:esc 60:esc 100:esc 140:esc 180:esc 220:esc 300:enter 360:esc 420:esc 480:esc 540:esc 600:esc 900:ctrl 960:ctrl" \
  RC_FRAMES=1100 RC_SCREENSHOT=room.ppm RC_TRACE=fps target/release/technomage --game-dir orig/technomage/cd-de
```

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

## Checks

```sh
tools/audit.sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```
