// SPDX-License-Identifier: MIT
//! Checks against the user's own media in `../orig/` (not available in CI):
//! `cargo test -p rc-cli -- --ignored`.
//!
//! Every medium in a manifest must open, list every pinned binary of its variant with the
//! pinned size, and (for the fast readers) extract the main executable with the pinned hash.

use rc_manifest::{Role, Treatment};
use rc_media::MediaType;
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."))
}

#[test]
#[ignore = "needs the game media in orig/"]
fn every_medium_in_orig_matches_its_manifest() {
    let ms = rc_manifest::load_dir(&repo().join("games")).expect("manifests");
    let tmp = std::env::temp_dir().join(format!("rc-orig-media-{}", std::process::id()));
    let mut checked = 0;
    for m in &ms {
        for v in &m.variants {
            for md in &v.media {
                if md.kind == rc_manifest::MediaKind::Slice {
                    continue; // read through its installer
                }
                let path = repo().join("orig").join(&md.file);
                if !path.is_file() {
                    eprintln!("skip {} (not present)", md.file);
                    continue;
                }
                let (kind, mut ar) = rc_media::open(&path).expect("open medium");
                for b in m.binaries.iter().filter(|b| b.variants.contains(&v.id)) {
                    if b.treatment == Treatment::Ignore {
                        continue;
                    }
                    let e = ar
                        .entries()
                        .iter()
                        .find(|e| {
                            Path::new(&e.path)
                                .file_name()
                                .is_some_and(|n| n.eq_ignore_ascii_case(b.name.as_str()))
                        })
                        .unwrap_or_else(|| panic!("{}: {} not listed", md.file, b.name));
                    assert_eq!(e.size, b.size, "{}: size of {}", md.file, b.name);
                }
                if kind != MediaType::Inno {
                    // Fast readers: extract the main executable and compare its hash.
                    let main = m
                        .binaries
                        .iter()
                        .find(|b| b.role == Role::Main && b.variants.contains(&v.id))
                        .expect("main binary");
                    let dest = tmp.join(&m.game.id).join(&v.id);
                    let name = main.name.clone();
                    let n = ar
                        .extract(&dest, &|e| e.path.eq_ignore_ascii_case(&name))
                        .expect("extract");
                    assert_eq!(n, 1);
                    let got = rc_manifest::sha256_file(&dest.join(&main.name)).expect("hash");
                    assert_eq!(got, main.sha256, "{}: {}", md.file, main.name);
                }
                checked += 1;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(checked > 0, "no media found in orig/");
    eprintln!("{checked} media checked");
}
