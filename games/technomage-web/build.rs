// SPDX-License-Identifier: MIT
//! Link options of the player module: the page reads the function table (the game's module
//! goes into it), the shadow stack pointer (each guest thread has its own) and grows memory up
//! to 4 GiB (guest memory is 768 MiB of it).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_FAMILY").is_ok_and(|f| f == "wasm") {
        for arg in [
            "--export-table",
            "--growable-table",
            "--export=__stack_pointer",
            "--max-memory=4294967296",
            "-zstack-size=8388608",
        ] {
            println!("cargo:rustc-cdylib-link-arg={arg}");
        }
    }
}
