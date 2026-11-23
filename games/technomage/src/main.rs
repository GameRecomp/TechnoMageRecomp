// SPDX-License-Identifier: MIT
//! TechnoMage shell: finds the install (`--game-dir`, `$RC_GAME_DIR`), checks the executable
//! against the lifted builds' hashes, maps it (restored from its UPX packing) into guest memory
//! and runs its entry point on the runtime.
//!
//! `--check` lists the imports the HLE layer does not implement yet, without running the game.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

// The shards hold the lifted functions; naming them links them in.
use technomage_shard_00 as _;
use technomage_shard_01 as _;
use technomage_shard_02 as _;
use technomage_shard_03 as _;
use technomage_shard_04 as _;
use technomage_shard_05 as _;
use technomage_shard_06 as _;
use technomage_shard_07 as _;
use technomage_shard_08 as _;
use technomage_shard_09 as _;
use technomage_shard_10 as _;
use technomage_shard_11 as _;
use technomage_shard_12 as _;
use technomage_shard_13 as _;
use technomage_shard_14 as _;
use technomage_shard_15 as _;

#[allow(clippy::all)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

struct Args {
    game_dir: Option<PathBuf>,
    check: bool,
    headless: bool,
}

fn args() -> Args {
    let mut a = Args {
        game_dir: std::env::var_os("RC_GAME_DIR").map(PathBuf::from),
        check: false,
        headless: std::env::var_os("RC_HEADLESS").is_some(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--game-dir" => a.game_dir = it.next().map(PathBuf::from),
            "--check" => a.check = true,
            "--headless" => a.headless = true,
            _ => {}
        }
    }
    a
}

/// The file in `dir` whose name matches `name` case-insensitively.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
                .then(|| e.path())
        })
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(rc_have_generated)]
fn lifted() -> &'static [&'static rc_runtime::lifted::GeneratedBinary] {
    generated::set::SET.binaries
}

#[cfg(not(rc_have_generated))]
fn lifted() -> &'static [&'static rc_runtime::lifted::GeneratedBinary] {
    &[]
}

/// The directory of this program (where a package keeps `data/`).
fn program_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Where the lifted code comes from: compiled in (Rust) or a converter package's native blob.
enum Code {
    Rust(&'static rc_runtime::lifted::GeneratedBinary),
    Native,
}

fn run(a: &Args) -> Result<ExitCode, String> {
    // In a converter package the game's files are in data/game beside the program.
    let packaged = program_dir()
        .map(|d| d.join("data/game"))
        .filter(|d| d.is_dir());
    let Some(dir) = a.game_dir.clone().or(packaged) else {
        return Err(
            "usage: technomage --game-dir DIR [--check] [--headless] (see docs/DEVELOPMENT.md)"
                .into(),
        );
    };
    let dir = &dir;
    let exe_path = find(dir, "TechnoMage.exe")
        .ok_or_else(|| format!("{}: no TechnoMage.exe", dir.display()))?;
    let exe = std::fs::read(&exe_path).map_err(|e| format!("{}: {e}", exe_path.display()))?;
    let sha = sha256_hex(&exe);
    let code = match lifted().iter().find(|b| b.sha256 == sha) {
        Some(b) => Code::Rust(b),
        None => {
            // A package: data/code-<target>-technomage.exe.bin beside the program.
            let target =
                rc_runtime::native::target_name().ok_or("no native code for this machine")?;
            let blob_path = program_dir()
                .map(|d| d.join(format!("data/code-{target}-technomage.exe.bin")))
                .filter(|p| p.is_file())
                .ok_or_else(|| {
                    format!(
                        "{}: no lifted code for this build ({}): run `recomp lift`, or use a converter package",
                        exe_path.display(),
                        &sha[..16]
                    )
                })?;
            let blob =
                std::fs::read(&blob_path).map_err(|e| format!("{}: {e}", blob_path.display()))?;
            let l = rc_runtime::native::load(&blob)
                .map_err(|e| format!("{}: {e}", blob_path.display()))?;
            eprintln!(
                "technomage: native code, {} functions (modules {})",
                l.functions,
                l.modules.join(", ")
            );
            Code::Native
        }
    };
    if let Code::Rust(build) = &code {
        eprintln!(
            "technomage: {} ({}), {} lifted functions",
            build.name,
            build.variant,
            build.table.len()
        );
    }

    rc_runtime::clock::init_from_env();
    rc_runtime::vfs::init(dir, user_dir());
    rc_hle::register_all();
    if std::env::var("RC_TRACE").is_ok_and(|v| v.contains("imports")) {
        rc_runtime::imports::set_trace(true);
    }
    let mem = rc_runtime::mem::GuestMemory::get(1 << 30).mem();
    // The lifted DLLs beside the executable first (binkw32.dll): matched by hash (Rust), or the
    // ones a native blob holds, at their preferred base.
    let mut dlls = Vec::new();
    let dll_names: Vec<(
        String,
        Option<u32>,
        Option<&'static rc_runtime::lifted::GeneratedBinary>,
    )> = match &code {
        Code::Rust(_) => lifted()
            .iter()
            .filter(|b| !b.name.to_ascii_lowercase().ends_with(".exe"))
            .map(|b| (b.name.to_string(), Some(b.base), Some(*b)))
            .collect(),
        Code::Native => vec![("binkw32.dll".to_string(), None, None)],
    };
    for (name, base, b) in dll_names {
        let Some(path) = find(dir, &name) else {
            continue;
        };
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(b) = b {
            if sha256_hex(&data) != b.sha256 {
                continue;
            }
        }
        let m =
            rc_runtime::process::map(mem, &name, &data, base, &[]).map_err(|e| e.to_string())?;
        if let Some(b) = b {
            rc_runtime::dispatch::add_lifted(b.table);
        } else if !rc_runtime::dispatch::is_known(m.entry) {
            continue; // the blob does not hold this DLL's code
        }
        rc_runtime::process::add_module(m.clone());
        eprintln!("technomage: {name} at {:#010x}", m.base);
        dlls.push(m);
    }
    let image = rc_runtime::process::image_of(&exe).map_err(|e| e.to_string())?;
    let module = rc_runtime::process::map(mem, "TechnoMage.exe", &image, None, &dlls)
        .map_err(|e| e.to_string())?;
    rc_runtime::vm::set_floor(module.base + module.size);
    rc_hle::kernel32::set_image_base(module.base);
    rc_runtime::process::add_module(module.clone());
    if let Code::Rust(build) = &code {
        rc_runtime::dispatch::add_lifted(build.table);
    }

    if a.check {
        let missing = rc_runtime::imports::missing();
        for m in &missing {
            println!("missing {m}");
        }
        println!("{} imports not implemented", missing.len());
        return Ok(if missing.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
    }

    rc_runtime::threads::init_main();
    let mut cpu = rc_runtime::Cpu::new(mem);
    rc_runtime::process::main_thread(&mut cpu, module.base);
    // DllMain(module, DLL_PROCESS_ATTACH, 0) of each lifted DLL, before the program starts.
    for d in &dlls {
        let ok = rc_runtime::dispatch::call_guest(&mut cpu, d.entry, &[d.base, 1, 0]);
        if ok == 0 {
            return Err(format!("{} did not initialise", d.name));
        }
    }
    rc_runtime::dispatch::dispatch(&mut cpu, module.entry);
    Ok(ExitCode::from(cpu.r[rc_runtime::cpu::EAX] as u8))
}

/// Where the game's settings and saves go: `$XDG_DATA_HOME/technomagerecomp` or the
/// platform's equivalent.
fn user_dir() -> Option<PathBuf> {
    let base = std::env::var_os("RC_USER_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            if cfg!(windows) {
                std::env::var_os("APPDATA").map(PathBuf::from)
            } else {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
                    })
            }
        })?;
    let d = base.join("technomagerecomp");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn main() -> ExitCode {
    let a = args();
    #[cfg(feature = "window")]
    if !a.headless && !a.check {
        let a = Args {
            game_dir: a.game_dir.clone(),
            check: false,
            headless: false,
        };
        rc_platform::bevy_app::run("TechnoMage", move || {
            if let Err(e) = run(&a) {
                eprintln!("technomage: {e}");
                std::process::exit(2);
            }
        });
    }
    match run(&a) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("technomage: {e}");
            ExitCode::from(2)
        }
    }
}
