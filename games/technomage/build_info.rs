// SPDX-License-Identifier: MIT
// Build information for the "Show information" page (src/patches.rs), included by the build
// scripts of the shell and the browser player: `RC_BUILD_TIME` (UTC, or `SOURCE_DATE_EPOCH` for
// reproducible builds) and `RC_COMMIT` (short hash, `-dirty` with uncommitted changes).

/// Emit the build information; `sources` are the directories whose changes make a new build.
fn build_info(sources: &[&str]) {
    use std::path::Path;
    use std::process::Command;
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let git = |args: &[&str]| -> Option<String> {
        let o = Command::new("git").args(args).current_dir(&dir).output().ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(hash) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|s| !s.is_empty());
            format!("{hash}{}", if dirty { "-dirty" } else { "" })
        }
        None => "unknown".to_string(),
    };
    println!("cargo:rustc-env=RC_COMMIT={commit}");
    if let Some(g) = git(&["rev-parse", "--absolute-git-dir"]) {
        let g = Path::new(&g);
        for f in ["HEAD", "index", "packed-refs"] {
            println!("cargo:rerun-if-changed={}", g.join(f).display());
        }
        if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", g.join(r).display());
        }
    }
    for s in sources {
        println!("cargo:rerun-if-changed={s}");
    }

    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
    // The civil date of a day count (Howard Hinnant's days-to-civil).
    let z = (secs / 86_400) as i64 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let t = secs % 86_400;
    println!(
        "cargo:rustc-env=RC_BUILD_TIME={year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        t / 3600,
        t % 3600 / 60
    );
}
