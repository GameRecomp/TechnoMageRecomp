// SPDX-License-Identifier: MIT
//! Includes the lifted code when `generated/technomage/set.rs` exists (`recomp lift`), so the
//! shell also builds without it (CI, a fresh clone).

use std::path::PathBuf;

include!("build_info.rs");

fn main() {
    build_info(&["src"]);
    // Android: the library's calls to functions it defines itself bind to them, not to a
    // system library's of the same name (rc_platform::android takes the gamepads' events in
    // its own AInputQueue_getEvent, which android-activity calls).
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "android") {
        println!("cargo:rustc-link-arg=-Wl,-Bsymbolic-functions");
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../generated/technomage");
    let set = root.join("set.rs");
    println!("cargo:rerun-if-changed={}", set.display());
    // A converter shell template (RC_NO_GENERATED): no lifted code, the packaged native code
    // instead.
    println!("cargo:rerun-if-env-changed=RC_NO_GENERATED");
    let skip = std::env::var_os("RC_NO_GENERATED").is_some();
    println!("cargo:rustc-check-cfg=cfg(rc_have_generated)");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("generated.rs");
    let text = if set.is_file() && !skip {
        println!("cargo:rustc-cfg=rc_have_generated");
        let set = set.canonicalize().expect("set.rs");
        format!("#[path = {:?}]\npub mod set;\n", set.display().to_string())
    } else {
        String::new()
    };
    std::fs::write(out, text).expect("generated.rs");
}
