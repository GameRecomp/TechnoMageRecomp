// SPDX-License-Identifier: MIT
//! `recomp list` and `recomp extract`.

use rc_manifest::{Manifest, Treatment};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

pub fn list(paths: &[PathBuf]) -> Result<ExitCode, String> {
    let [media] = paths else {
        return Err("list needs exactly one MEDIA".into());
    };
    let (kind, ar) = rc_media::open(media).map_err(|e| format!("{}: {e}", media.display()))?;
    let mut total = 0u64;
    for e in ar.entries() {
        println!("{:>12}  {}", e.size, e.path);
        total += e.size;
    }
    println!("{kind}: {} files, {total} bytes", ar.entries().len());
    Ok(ExitCode::SUCCESS)
}

/// The manifest variant a medium belongs to, by SHA-256.
pub(crate) fn find_variant<'a>(ms: &'a [Manifest], sha: &str) -> Option<(&'a Manifest, &'a str)> {
    ms.iter().find_map(|m| {
        m.variants
            .iter()
            .find(|v| v.media.iter().any(|md| md.sha256 == sha))
            .map(|v| (m, v.id.as_str()))
    })
}

pub(crate) fn is_empty_dir(p: &Path) -> bool {
    fs::read_dir(p)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true)
}

/// Find `name` (ASCII case-insensitive) directly in `dir` or one level below.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        for e in fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 1 {
                stack.push((p, depth + 1));
            } else if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
                return Some(p);
            }
        }
    }
    None
}

/// What the installer writes into the registry (GOG: settings, Cultures 2's serial), as
/// `registry-install.txt` in the unpacked folder, where the runtime reads it before the player's
/// own registry.
fn write_registry(ar: &dyn rc_media::Archive, dest: &Path) -> Result<(), String> {
    let reg = rc_media::registry_file(ar.registry());
    if !reg.is_empty() {
        let path = dest.join("registry-install.txt");
        fs::write(&path, reg).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("installer registry entries → {}", path.display());
    }
    Ok(())
}

/// The same for a folder unpacked before (only the setup header is read).
/// `recomp-variant.txt`: which release this is (several share an executable), for the runtime's
/// language and code page, as the converter writes it.
fn write_variant(dest: &Path, variant: &str) -> Result<(), String> {
    let path = dest.join(rc_convert::install::VARIANT_FILE);
    fs::write(&path, variant).map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn write_installer_registry(media: &Path, dest: &Path) -> Result<(), String> {
    if dest.join("registry-install.txt").exists() {
        return Ok(());
    }
    let (_, ar) = rc_media::open(media).map_err(|e| format!("{}: {e}", media.display()))?;
    write_registry(ar.as_ref(), dest)
}

/// `recomp extract --game ID FOLDER`: unpack game `id` from one of its media in `folder` (the
/// variants in manifest order), unless `orig/<id>/` already holds an unpacked variant. Files are
/// matched by size first, so only likely candidates are hashed.
pub fn extract_game(
    games: &Path,
    id: &str,
    force: bool,
    paths: &[PathBuf],
) -> Result<ExitCode, String> {
    let [folder] = paths else {
        return Err("extract --game needs exactly one FOLDER".into());
    };
    let ms = rc_manifest::load_dir(games).map_err(|e| e.to_string())?;
    let m = ms
        .iter()
        .find(|m| m.game.id == id)
        .ok_or_else(|| format!("no game {id:?} in {}", games.display()))?;
    let orig = games
        .parent()
        .unwrap_or(Path::new("."))
        .join("orig")
        .join(id);
    if !force {
        if let Some(v) = m
            .variants
            .iter()
            .find(|v| orig.join(&v.id).is_dir() && !is_empty_dir(&orig.join(&v.id)))
        {
            println!("{id}: already unpacked in {}", orig.join(&v.id).display());
            return Ok(ExitCode::SUCCESS);
        }
    }
    let mut files: Vec<(PathBuf, u64)> = fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| (e.path(), meta.len()))
        })
        .collect();
    files.sort();
    for v in &m.variants {
        // The medium that is opened: the only one, or the installer beside its data slices
        // (which `extract` finds next to it).
        let opened: Vec<_> = v
            .media
            .iter()
            .filter(|m| m.kind != rc_manifest::MediaKind::Slice)
            .collect();
        let [medium] = opened.as_slice() else {
            continue;
        };
        for (path, _) in files.iter().filter(|(_, size)| *size == medium.size) {
            let sha =
                rc_manifest::sha256_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if sha == medium.sha256 {
                return extract(games, None, true, std::slice::from_ref(path));
            }
        }
    }
    Err(format!(
        "{}: none of the media of {} ({id})",
        folder.display(),
        m.game.title
    ))
}

pub fn extract(
    games: &Path,
    dest: Option<&Path>,
    force: bool,
    paths: &[PathBuf],
) -> Result<ExitCode, String> {
    let [media] = paths else {
        return Err("extract needs exactly one MEDIA".into());
    };
    let ms = rc_manifest::load_dir(games).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let sha = rc_manifest::sha256_file(media).map_err(|e| format!("{}: {e}", media.display()))?;
    let variant = find_variant(&ms, &sha);

    let dest = match (dest, variant) {
        (Some(d), _) => d.to_path_buf(),
        (None, Some((m, v))) => games
            .parent()
            .unwrap_or(Path::new("."))
            .join("orig")
            .join(&m.game.id)
            .join(v),
        (None, None) => {
            return Err(format!(
                "{} is not in any manifest (unsupported build); pass --dest to unpack it anyway",
                media.display()
            ))
        }
    };
    if !force && dest.exists() && !is_empty_dir(&dest) {
        // Unpacked before: add what newer versions write besides the files (the installer's
        // registry entries, e.g. Cultures 2's serial), but leave the files alone.
        let had = dest.join("registry-install.txt").exists();
        write_installer_registry(media, &dest)?;
        if let Some((_, v)) = variant {
            write_variant(&dest, v)?;
        }
        if had || !dest.join("registry-install.txt").exists() {
            println!(
                "{}: already unpacked, nothing to add (--force unpacks again)",
                dest.display()
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;

    match variant {
        Some((m, v)) => println!("{}: {}/{v} ({})", media.display(), m.game.id, m.game.title),
        None => println!("{}: not in any manifest", media.display()),
    }
    let (kind, mut ar) = rc_media::open(media).map_err(|e| format!("{}: {e}", media.display()))?;
    println!("{kind}, {} files → {}", ar.entries().len(), dest.display());
    let n = ar
        .extract(&dest, &|_| true)
        .map_err(|e| format!("extracting {}: {e}", media.display()))?;
    println!("{n} files written in {:.1}s", t0.elapsed().as_secs_f64());
    write_registry(ar.as_ref(), &dest)?;
    if let Some((_, v)) = variant {
        write_variant(&dest, v)?;
    }

    let Some((m, v)) = variant else {
        return Ok(ExitCode::SUCCESS);
    };
    // Verify every binary the manifest pins for this variant.
    let mut bad = 0;
    for b in m
        .binaries
        .iter()
        .filter(|b| b.variants.iter().any(|x| x == v))
    {
        let status = match find_file(&dest, &b.name) {
            None if b.treatment == Treatment::Ignore => continue,
            None => {
                bad += 1;
                "MISSING".to_string()
            }
            Some(p) => match rc_manifest::sha256_file(&p) {
                Ok(h) if h == b.sha256 => "ok".to_string(),
                Ok(h) => {
                    bad += 1;
                    format!("MISMATCH ({})", &h[..16])
                }
                Err(e) => {
                    bad += 1;
                    format!("unreadable: {e}")
                }
            },
        };
        println!(
            "  {:<20} {:<12} {status}",
            b.name,
            format!("{:?}", b.role).to_lowercase()
        );
    }
    if bad > 0 {
        eprintln!("{bad} pinned binaries missing or different");
        return Ok(ExitCode::from(1));
    }
    println!("all pinned binaries verified");
    Ok(ExitCode::SUCCESS)
}
