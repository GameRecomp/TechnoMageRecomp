// SPDX-License-Identifier: MIT
//! `recomp discover` and `recomp codediff`: code discovery on one binary, and a function-level
//! comparison of two builds.

use rc_manifest::Manifest;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub struct DiscoverOpts<'a> {
    pub games: &'a Path,
    pub out: Option<&'a Path>,
    pub base: Option<u32>,
    pub write: bool,
    pub verbose: bool,
    pub no_relocs: bool,
}

pub(crate) use rc_convert::lift::{Known, Loaded};

/// A binary as the manifests know it; of several games the one whose id names a directory of the
/// file's path (`orig/<game>/<variant>/`).
fn find(manifests: &[Manifest], sha256: &str, file: &Path) -> Option<Known> {
    rc_convert::lift::find(manifests, sha256, &|game| {
        file.ancestors()
            .any(|a| a.file_name().is_some_and(|n| n.eq_ignore_ascii_case(game)))
    })
}

pub(crate) fn load(
    file: &Path,
    games: &Path,
    base: Option<u32>,
    use_relocs: bool,
) -> Result<Loaded, String> {
    let data = fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let sha256 = rc_manifest::sha256_hex(&data);
    let manifests = rc_manifest::load_dir(games).map_err(|e| e.to_string())?;
    let known = find(&manifests, &sha256, file);
    let name = file
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    rc_convert::lift::load(&data, &name, known, base, use_relocs)
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// `generated/<game>/<binary>-<variant>/` next to the `games/` directory.
pub(crate) fn default_out(games: &Path, l: &Loaded) -> PathBuf {
    let root = games.parent().unwrap_or(Path::new(".")).join("generated");
    match &l.known {
        Some(k) => root.join(&k.game).join(k.binary.output_dir_name()),
        None => root.join("unknown").join(format!(
            "{}-{}",
            l.map.binary.name.to_ascii_lowercase(),
            &l.sha256[..8]
        )),
    }
}

pub fn discover(o: &DiscoverOpts, files: &[PathBuf]) -> Result<ExitCode, String> {
    if files.is_empty() {
        return Err("discover needs at least one FILE".into());
    }
    if o.out.is_some() && files.len() > 1 {
        return Err("--out works with one FILE only".into());
    }
    for (n, file) in files.iter().enumerate() {
        if n > 0 {
            println!();
        }
        let l = load(file, o.games, o.base, !o.no_relocs)?;
        match &l.known {
            Some(k) => println!(
                "manifest    {}/{} {}",
                k.game,
                k.binary.variants.join(","),
                k.binary.name
            ),
            None => println!("manifest    not listed (no hints)"),
        }
        print!("{}", l.map.report(o.verbose));
        if o.write {
            let dir = o
                .out
                .map_or_else(|| default_out(o.games, &l), Path::to_path_buf);
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = dir.join("codemap.json");
            let json = serde_json::to_string_pretty(&l.map).map_err(|e| e.to_string())?;
            fs::write(&path, json + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
            println!("code map    {}", path.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn codediff(games: &Path, verbose: bool, files: &[PathBuf]) -> Result<ExitCode, String> {
    let [a, b] = files else {
        return Err("codediff needs exactly two FILEs".into());
    };
    let la = load(a, games, None, true)?;
    let lb = load(b, games, None, true)?;
    let fa = rc_discover::fingerprints(&la.image, &la.map);
    let fb = rc_discover::fingerprints(&lb.image, &lb.map);
    let d = rc_discover::diff(&fa, &fb);
    let pct = |x: u64, y: u64| {
        if y == 0 {
            0.0
        } else {
            x as f64 * 100.0 / y as f64
        }
    };
    println!(
        "A           {} ({} functions, {} bytes)",
        a.display(),
        d.a_functions,
        d.a_bytes
    );
    println!(
        "B           {} ({} functions, {} bytes)",
        b.display(),
        d.b_functions,
        d.b_bytes
    );
    println!(
        "identical   {} functions ({:.1} % of A), {} bytes ({:.1} % of A's code)",
        d.identical,
        pct(d.identical as u64, d.a_functions as u64),
        d.identical_bytes,
        pct(d.identical_bytes, d.a_bytes)
    );
    let bytes = |v: &[(u32, u32)]| v.iter().map(|x| u64::from(x.1)).sum::<u64>();
    println!(
        "only in A   {} functions, {} bytes",
        d.a_only.len(),
        bytes(&d.a_only)
    );
    println!(
        "only in B   {} functions, {} bytes",
        d.b_only.len(),
        bytes(&d.b_only)
    );
    if verbose {
        for (va, size) in &d.a_only {
            println!("  A only  {va:#010x} ({size} bytes)");
        }
        for (va, size) in &d.b_only {
            println!("  B only  {va:#010x} ({size} bytes)");
        }
    }
    Ok(ExitCode::SUCCESS)
}
