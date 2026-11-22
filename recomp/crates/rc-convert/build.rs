// SPDX-License-Identifier: MIT
//! Builds the games' manifests (games/*/game.toml) into the crate, for the converter in a browser,
//! which has no games/ directory to read.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let games = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../games");
    println!("cargo:rerun-if-changed={}", games.display());
    let mut list: Vec<PathBuf> = std::fs::read_dir(&games)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path().join("game.toml"))
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    let mut out = String::from("[");
    for p in &list {
        println!("cargo:rerun-if-changed={}", p.display());
        let game = p
            .parent()
            .and_then(|d| d.file_name())
            .unwrap_or_default()
            .to_string_lossy();
        let _ = write!(
            out,
            "(\"games/{game}/game.toml\", include_str!({:?})),",
            p.display().to_string()
        );
    }
    out.push(']');
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("manifests.rs");
    std::fs::write(dest, out).expect("manifests.rs");
}
