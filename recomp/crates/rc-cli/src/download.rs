// SPDX-License-Identifier: MIT
//! `recomp download GAME LANGUAGE`: the game's medium for that language from where the manifest
//! says it is archived (archive.org: the games are abandonware), checked by size and SHA-256.
//! The transfer is curl's, which Linux, macOS and Windows 10 and later have.

use rc_convert::rc_manifest::{self, Manifest};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The manifests: from the games folder when there is one (a checkout), else the converter's own.
fn manifests(games: &Path) -> Result<Vec<Manifest>, String> {
    match rc_manifest::load_dir(games) {
        Ok(ms) if !ms.is_empty() => Ok(ms),
        _ => rc_convert::lift::manifests(),
    }
}

/// The languages a game is offered in, for messages.
fn languages(m: &Manifest) -> String {
    m.variants
        .iter()
        .map(|v| v.language.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Download `game`'s medium for `language` into `dest` (a folder); its path. A file already there
/// with the right hash is kept.
pub fn download(games: &Path, game: &str, language: &str, dest: &Path) -> Result<PathBuf, String> {
    let ms = manifests(games)?;
    let m = ms.iter().find(|m| m.game.id == game).ok_or_else(|| {
        let ids: Vec<_> = ms.iter().map(|m| m.game.id.as_str()).collect();
        format!("unknown game {game:?} (known: {})", ids.join(", "))
    })?;
    let v = m
        .variants
        .iter()
        .find(|v| v.language == language || v.id == language)
        .ok_or_else(|| {
            format!(
                "{} is not offered in {language:?} (languages: {})",
                m.game.title,
                languages(m)
            )
        })?;
    let [media] = v.media.as_slice() else {
        return Err(format!("{game}/{}: more than one medium", v.id));
    };
    let url = media
        .url
        .as_deref()
        .ok_or_else(|| format!("{game}/{}: no download address", v.id))?;
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let path = dest.join(&media.file);
    if path.is_file()
        && rc_manifest::sha256_file(&path).ok().as_deref() == Some(media.sha256.as_str())
    {
        println!("{}: already there", path.display());
        return Ok(path);
    }
    let part = dest.join(format!("{}.part", media.file));
    println!(
        "download    {} ({}, {} MiB) from {url}",
        m.game.title,
        v.language,
        media.size >> 20
    );
    let st = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--retry",
            "3",
            "--progress-bar",
            "--continue-at",
            "-",
            "--output",
        ])
        .arg(&part)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e} (curl is needed to download)"))?;
    if !st.success() {
        return Err(format!("downloading {url} failed ({st})"));
    }
    let size = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let sha = rc_manifest::sha256_file(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    if size != media.size || sha != media.sha256 {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "{url}: {size} bytes, SHA-256 {sha}; expected {} bytes, {} (the archived file changed?)",
            media.size, media.sha256
        ));
    }
    fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("verified    {}", path.display());
    Ok(path)
}
