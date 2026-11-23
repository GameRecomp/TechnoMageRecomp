// SPDX-License-Identifier: MIT
//! Build script of every shard crate (`shardNN/build.rs` includes this file): finds the shard's
//! part of each lifted binary, `generated/technomage/<binary>-<variant>/shard_NN.rs`, and
//! includes them as modules. Without generated code the shard is empty.

use std::path::PathBuf;

fn main() {
    let name = std::env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME");
    let n: usize = name
        .rsplit('-')
        .next()
        .and_then(|s| s.parse().ok())
        .expect("a shard number at the end of the package name");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../generated/technomage");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-env-changed=RC_NO_GENERATED");
    if std::env::var_os("RC_NO_GENERATED").is_some() {
        let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("shard.rs");
        std::fs::write(out, "").expect("shard.rs");
        return;
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    let mut text = String::new();
    for (i, d) in dirs.iter().enumerate() {
        let f = d.join(format!("shard_{n:02}.rs"));
        if f.is_file() {
            println!("cargo:rerun-if-changed={}", f.display());
            let f = f.canonicalize().expect("shard file");
            text.push_str(&format!(
                "#[path = {:?}]\nmod b{i};\n",
                f.display().to_string()
            ));
        }
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("shard.rs");
    std::fs::write(out, text).expect("shard.rs");
}
