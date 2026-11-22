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

## Build (from M3)

```sh
cargo run -p rc-cli --release -- lift technomage cd-de   # → generated/technomage/…
cargo build --release -p technomage
target/release/technomage --game-dir orig/technomage/cd-de
```

## Checks

```sh
tools/audit.sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```
