// SPDX-License-Identifier: MIT
//! Includes the lifted code when `generated/technomage/set.rs` exists (`recomp lift`), so the
//! shell also builds without it (CI, a fresh clone).

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../generated/technomage");
    let set = root.join("set.rs");
    println!("cargo:rerun-if-changed={}", set.display());
    println!("cargo:rustc-check-cfg=cfg(rc_have_generated)");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("generated.rs");
    let text = if set.is_file() {
        println!("cargo:rustc-cfg=rc_have_generated");
        let set = set.canonicalize().expect("set.rs");
        format!("#[path = {:?}]\npub mod set;\n", set.display().to_string())
    } else {
        String::new()
    };
    std::fs::write(out, text).expect("generated.rs");
}
