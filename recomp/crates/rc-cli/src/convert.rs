// SPDX-License-Identifier: MIT
//! `recomp convert`: the user's installer, CD image, zip or installed folder to a program, in
//! one step: identify, unpack (to `orig/<game>/<variant>/`), lift every binary the manifest
//! lifts, build the game's shell with CMake, and put it with a launcher into `out/<game>/`.
//! The program contains code derived from the user's copy: it stays on their machine.

use crate::lift::{self, identify_dir};
use crate::media;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub struct ConvertOpts<'a> {
    pub games: &'a Path,
    pub preset: &'a str,
    pub force: bool,
}

/// Compile jobs for the build: `CMAKE_BUILD_PARALLEL_LEVEL` if set, else one per CPU but no more than
/// one per 1.5 GB of available memory. (`--parallel` without a number lets Make start every file of the
/// generated code at once, which takes tens of gigabytes.)
fn build_jobs() -> usize {
    if let Some(n) = std::env::var("CMAKE_BUILD_PARALLEL_LEVEL")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        return n;
    }
    let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
    let by_memory = available_memory_mb().map_or(cpus, |mb| (mb / 1536) as usize);
    cpus.min(by_memory).max(1)
}

/// MemAvailable from /proc/meminfo, in megabytes (Linux only; None elsewhere).
fn available_memory_mb() -> Option<u64> {
    let info = fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

fn run(cmd: &mut Command) -> Result<(), String> {
    println!("+ {cmd:?}");
    let st = cmd
        .status()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({st})", cmd.get_program()))
    }
}

pub fn convert(o: &ConvertOpts, paths: &[PathBuf]) -> Result<ExitCode, String> {
    let [input] = paths else {
        return Err(
            "convert needs exactly one installer, CD image, zip or installed folder".into(),
        );
    };
    let ms = rc_manifest::load_dir(o.games).map_err(|e| e.to_string())?;
    let root = o.games.parent().unwrap_or(Path::new(".")).to_path_buf();

    // 1. Which game and variant, and its files.
    let (manifest, variant, dir) = if input.is_dir() {
        let (m, v) = identify_dir(&ms, input)
            .ok_or_else(|| format!("{}: no known executable in it", input.display()))?;
        (m, v, input.clone())
    } else {
        let sha =
            rc_manifest::sha256_file(input).map_err(|e| format!("{}: {e}", input.display()))?;
        let (m, v) = media::find_variant(&ms, &sha).ok_or_else(|| {
            format!(
                "{} is not a supported installer, CD or zip",
                input.display()
            )
        })?;
        let dir = root.join("orig").join(&m.game.id).join(v);
        if o.force || !dir.exists() || media::is_empty_dir(&dir) {
            let _ = media::extract(o.games, None, true, std::slice::from_ref(input))?;
        } else {
            println!("{}: already unpacked", dir.display());
            media::write_installer_registry(input, &dir)?;
        }
        (m, v.to_string(), dir)
    };
    let id = &manifest.game.id;
    println!("game        {} ({id}/{variant})", manifest.game.title);

    // 2. Lift what the manifest lifts and the folder has.
    let _ = lift::lift(
        &lift::LiftOpts {
            games: o.games,
            out: None,
            set: None,
            base: None,
            comments: false,
            verbose: false,
        },
        std::slice::from_ref(&dir),
    )?;

    // 3. Build the game's program.
    let target = format!("sacked-{id}");
    // The build lifts again when the lifter or the manifest changes (cmake/lift.cmake): from
    // this install, not another variant in orig/. (`absolute`, not `canonicalize`: no `\\?\`
    // prefix on Windows, which CMake does not take.)
    let game_dir = std::path::absolute(&dir).unwrap_or_else(|_| dir.clone());
    let dirs_var = format!("-D{}_GAME_DIRS={}", id.to_uppercase(), game_dir.display());
    run(Command::new("cmake")
        .current_dir(&root)
        .args(["--preset", o.preset, &dirs_var]))?;
    let jobs = build_jobs().to_string();
    run(Command::new("cmake").current_dir(&root).args([
        "--build",
        "--preset",
        o.preset,
        "--parallel",
        &jobs,
        "--target",
        &target,
    ]))?;
    let built = root
        .join("build")
        .join(o.preset)
        .join("games")
        .join(id)
        .join(&target);
    let built = if built.exists() {
        built
    } else {
        built.with_extension("exe")
    };

    // 4. The program and a launcher that points it at the game's files.
    let out = root.join("out").join(id);
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let dest = out.join(built.file_name().unwrap_or_default());
    fs::copy(&built, &dest).map_err(|e| format!("{}: {e}", built.display()))?;
    if cfg!(windows) {
        let bat = out.join("play.bat");
        let text = format!(
            "@echo off\r\n\"%~dp0{}\" --game-dir \"{}\" %*\r\n",
            target,
            game_dir.display()
        );
        fs::write(&bat, text).map_err(|e| e.to_string())?;
    } else {
        let sh = out.join("play.sh");
        let text = format!(
            "#!/bin/sh\n# Runs {} with the game's files from where they were unpacked.\nexec \"$(dirname \"$0\")/{}\" --game-dir \"{}\" \"$@\"\n",
            manifest.game.title,
            target,
            game_dir.display()
        );
        fs::write(&sh, text).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&sh, fs::Permissions::from_mode(0o755));
        }
    }
    println!(
        "ready       {} (start it with {})",
        dest.display(),
        out.join(if cfg!(windows) { "play.bat" } else { "play.sh" })
            .display()
    );
    Ok(ExitCode::SUCCESS)
}
