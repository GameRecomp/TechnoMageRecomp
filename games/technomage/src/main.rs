// SPDX-License-Identifier: MIT
//! TechnoMage shell: finds the install (`--game-dir`, `$RC_GAME_DIR`), checks the executable
//! against the manifest's hashes, and runs the lifted code on the runtime. Until code is lifted
//! (M3) it only reports what it would run.

fn main() {
    let game_dir = std::env::args()
        .skip_while(|a| a != "--game-dir")
        .nth(1)
        .or_else(|| std::env::var("RC_GAME_DIR").ok());
    match game_dir {
        Some(dir) => eprintln!("technomage: game directory {dir}; no lifted code built yet (M3)"),
        None => eprintln!("usage: technomage --game-dir DIR (see docs/DEVELOPMENT.md)"),
    }
    let _ = rc_hle::platform::Backend::default();
    std::process::exit(2);
}
