// SPDX-License-Identifier: MIT
//! `recomp template` and `recomp package`: packages that run without a compiler (rc-convert), as
//! TropicoRecomp's converter makes them.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use crate::native::{compile, programs};

/// `recomp template --game ID --system SYSTEM --shell PROGRAM -o OUT.zip`.
pub fn template(game: &str, system: &str, shell: &Path, out: &Path) -> Result<ExitCode, String> {
    let bytes = fs::read(shell).map_err(|e| format!("{}: {e}", shell.display()))?;
    let name = shell
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("--shell needs a file")?;
    let zip = rc_convert::make_template(game, system, name, &bytes)?;
    fs::write(out, zip).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("template    {game} for {system}: {}", out.display());
    Ok(ExitCode::SUCCESS)
}

/// `recomp package --template T.zip [-o OUT.zip] INSTALL`: the install's programs compiled for the
/// template's targets, the template, and the install's files, as one zip.
pub fn package(
    games: &Path,
    template: &Path,
    out: Option<&Path>,
    install: &Path,
) -> Result<ExitCode, String> {
    let ms = rc_manifest::load_dir(games).map_err(|e| e.to_string())?;
    let (manifest, variant) = crate::lift::identify_dir(&ms, install)
        .ok_or_else(|| format!("{}: no known executable in it", install.display()))?;
    let id = manifest.game.id.clone();
    let tbytes = fs::read(template).map_err(|e| format!("{}: {e}", template.display()))?;
    let tpl = rc_convert::template(&tbytes)?;
    let files = crate::lift::liftable_in(&ms, install)?;
    let mut blobs = Vec::new();
    for (l, p) in programs(games, &files)? {
        let exe = l
            .known
            .as_ref()
            .map_or_else(String::new, |k| k.binary.name.to_ascii_lowercase());
        for &t in &tpl.targets {
            let blob = compile(t, &p)?;
            println!(
                "native      {exe} for {}: {:.1} MB",
                t.name(),
                blob.len() as f64 / 1e6
            );
            blobs.push((t, exe.clone(), blob));
        }
    }
    let out = match out {
        Some(o) => o.to_path_buf(),
        None => {
            let d = games
                .parent()
                .unwrap_or(Path::new("."))
                .join("out")
                .join(&id);
            fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
            d.join(format!(
                "{}-{variant}-{}.{}",
                rc_convert::stem(&id),
                tpl.targets[0].name(),
                if tpl.apk { "apk" } else { "zip" }
            ))
        }
    };
    let f = fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let w = std::io::BufWriter::new(f);
    let mut n = 0;
    rc_convert::package(
        w,
        &tpl,
        &blobs,
        &mut |sink| rc_convert::folder_files(install, sink),
        None,
        &mut |_| n += 1,
    )?;
    println!(
        "package     {id}/{variant}, {n} game files → {}",
        out.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// The systems a converter makes builds for.
const SYSTEMS: [&str; 4] = ["windows", "linux", "macos", "android"];

/// The templates folder beside the running program (a converter download's).
fn templates_beside_program() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("where recomp is: {e}"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    Ok(exe
        .parent()
        .ok_or("where recomp is: no folder")?
        .join("templates"))
}

/// `recomp convert MEDIA --system SYSTEM [--templates DIR] [--out FILE]` (or `--template T.zip`):
/// a GOG installer or the Saga's zip into a build for a system, without a compiler, as the
/// converter page does: the game's code compiled to native code into the shell template of the
/// game for that system (from DIR, by default the templates folder beside `recomp`), with the
/// game's files, as a zip or, for Android, an APK.
pub fn convert_package(
    media: &Path,
    system: Option<&str>,
    templates: Option<&Path>,
    template: Option<&Path>,
    out: Option<&Path>,
) -> Result<ExitCode, String> {
    use std::io::Write as _;
    let t0 = std::time::Instant::now();
    if let Some(s) = system.filter(|s| !SYSTEMS.contains(s)) {
        return Err(format!("unknown system {s}: one of {}", SYSTEMS.join(", ")));
    }
    let ms = rc_convert::lift::manifests()?;
    let file = fs::File::open(media).map_err(|e| format!("{}: {e}", media.display()))?;
    let reader = std::io::BufReader::with_capacity(1 << 20, file);
    // An installer's external data slices beside it (setup_x.exe: setup_x-1.bin, -2.bin, …).
    let mut slices: Vec<Box<dyn rc_convert::rc_media::ReadSeek>> = Vec::new();
    for n in 1.. {
        let p = media.with_file_name(format!(
            "{}-{n}.bin",
            media.file_stem().unwrap_or_default().to_string_lossy()
        ));
        let Ok(f) = fs::File::open(&p) else { break };
        slices.push(Box::new(std::io::BufReader::with_capacity(1 << 20, f)));
    }
    let mut inst = rc_convert::install::Install::open_with(Box::new(reader), slices, &ms)
        .map_err(|e| format!("{}: {e}", media.display()))?;
    let (_, files) = inst.plan();
    let game_bytes: u64 = files.iter().map(|f| f.1).sum();
    eprintln!(
        "{} ({}): {} files, {:.0} MiB",
        inst.manifest.game.title,
        inst.variant,
        files.len(),
        game_bytes as f64 / (1u64 << 20) as f64
    );
    let stem = rc_convert::stem(&inst.manifest.game.id);
    let (tpath, default_out) = match (template, system) {
        (Some(t), _) => (t.to_path_buf(), None),
        (None, Some(s)) => {
            let dir = match templates {
                Some(d) => d.to_path_buf(),
                None => templates_beside_program()?,
            };
            let t = dir.join(format!("{stem}-{s}.zip"));
            if !t.is_file() {
                return Err(format!(
                    "no template for {} on {s}: {} is missing",
                    inst.manifest.game.title,
                    t.display()
                ));
            }
            let ext = if s == "android" { "apk" } else { "zip" };
            (
                t,
                Some(std::path::PathBuf::from(format!("{stem}-{s}.{ext}"))),
            )
        }
        (None, None) => return Err("convert needs --system SYSTEM or --template FILE".into()),
    };
    let out = match (out, &default_out) {
        (Some(o), _) => o.to_path_buf(),
        (None, Some(o)) => o.clone(),
        (None, None) => return Err("convert --template needs --out FILE".into()),
    };
    let tbytes = fs::read(&tpath).map_err(|e| format!("{}: {e}", tpath.display()))?;
    let tpl = rc_convert::template(&tbytes)?;
    let progs = inst.programs(&ms)?;
    eprintln!(
        "lifted {} programs ({:.1} s)",
        progs.len(),
        t0.elapsed().as_secs_f64()
    );
    let mut blobs = Vec::new();
    for p in &progs {
        for &t in &tpl.targets {
            blobs.push((t, p.exe.to_ascii_lowercase(), compile(t, p)?));
            eprintln!(
                "compiled {} for {} ({:.1} s)",
                p.exe,
                t.name(),
                t0.elapsed().as_secs_f64()
            );
        }
    }
    let f = fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let total = game_bytes.max(1);
    let (mut done, mut shown) = (0u64, 0u64);
    let w = rc_convert::package(
        std::io::BufWriter::with_capacity(1 << 20, f),
        &tpl,
        &blobs,
        &mut |sink| inst.files(sink),
        None,
        &mut |n| {
            done += n;
            let pct = done * 100 / total;
            if pct != shown {
                shown = pct;
                eprint!("\rpackaging {pct}%");
            }
        },
    )?;
    w.into_inner()
        .map_err(|e| e.to_string())?
        .flush()
        .map_err(|e| e.to_string())?;
    eprintln!(
        "\rwrote {} ({:.0} MiB) in {:.1} s",
        out.display(),
        fs::metadata(&out).map(|m| m.len()).unwrap_or(0) as f64 / (1u64 << 20) as f64,
        t0.elapsed().as_secs_f64()
    );
    Ok(ExitCode::SUCCESS)
}
