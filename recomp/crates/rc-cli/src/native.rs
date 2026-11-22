// SPDX-License-Identifier: MIT
//! `recomp lift --native TARGETS`: an install's lifted binaries compiled to native code (Cranelift,
//! rc-emit-native) instead of C, as one blob per executable that the runtime loads (rc/native.h).
//! No C compiler is needed to run it: the converter's packages are built from these.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rc_emit_native::{Compiled, Compiler, Options, Target};

use crate::discover::{default_out, load};

/// A target by name: `host`, or one of rc-emit-native's (`x86_64-sysv`, `x86_64-windows`,
/// `arm64-linux`, `arm64-apple`).
pub fn target(name: &str) -> Result<Target, String> {
    if name == "host" {
        return Target::host().ok_or_else(|| "no native code for this machine".to_string());
    }
    [
        Target::X64SysV,
        Target::X64Windows,
        Target::Arm64Linux,
        Target::Arm64Apple,
    ]
    .into_iter()
    .find(|t| t.name() == name)
    .ok_or_else(|| format!("unknown target {name:?}"))
}

pub use rc_convert::lift::Program;

/// Compile `p` for `target` into a blob, on as many threads as the machine has.
pub fn compile(target: Target, p: &Program) -> Result<Vec<u8>, String> {
    let compiler = Compiler::new(target, Options::default()).map_err(|e| e.to_string())?;
    let entries: BTreeSet<u32> = p.functions.iter().map(|f| f.addr).collect();
    let n = p.functions.len();
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get());
    let chunk = n.div_ceil(threads).max(1);
    let parts: Vec<Result<Vec<Compiled>, String>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|a| {
                let (compiler, entries, funcs) = (&compiler, &entries, &p.functions);
                s.spawn(move || {
                    funcs[a..(a + chunk).min(n)]
                        .iter()
                        .map(|f| compiler.compile(f, entries).map_err(|e| e.to_string()))
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("a worker panicked".into())))
            .collect()
    });
    let mut all = Vec::with_capacity(n);
    for part in parts {
        all.extend(part?);
    }
    rc_emit_native::link(target, &all, &p.modules).map_err(|e| e.to_string())
}

/// The programs of `files` (an install's lifted binaries): one per executable that is not a
/// multiplayer build, each with all the DLLs lifted with it, beside the executable's loaded file.
pub fn programs(
    games: &Path,
    files: &[PathBuf],
) -> Result<Vec<(crate::discover::Loaded, Program)>, String> {
    let loaded = files
        .iter()
        .map(|f| load(f, games, None, true))
        .collect::<Result<Vec<_>, _>>()?;
    let progs = rc_convert::lift::programs(&loaded)?;
    let mut loaded: Vec<Option<crate::discover::Loaded>> = loaded.into_iter().map(Some).collect();
    progs
        .into_iter()
        .map(|p| {
            let l = loaded
                .iter_mut()
                .find(|l| {
                    l.as_ref()
                        .and_then(|l| l.known.as_ref())
                        .is_some_and(|k| k.binary.name == p.exe)
                })
                .and_then(Option::take)
                .ok_or_else(|| format!("{}: not loaded", p.exe))?;
            Ok((l, p))
        })
        .collect()
}

/// Compile `p` to the web shell's WebAssembly module (rc-emit-wasm), on as many threads as the
/// machine has.
pub fn compile_wasm(p: &Program) -> Result<Vec<u8>, String> {
    let addrs: BTreeSet<u32> = p.functions.iter().map(|f| f.addr).collect();
    let compiler = rc_emit_wasm::Compiler::new(&addrs, rc_emit_wasm::Options::default());
    let n = p.functions.len();
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get());
    let chunk = n.div_ceil(threads).max(1);
    let parts: Vec<Result<Vec<rc_emit_wasm::Compiled>, String>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|a| {
                let (compiler, funcs) = (&compiler, &p.functions);
                s.spawn(move || {
                    funcs[a..(a + chunk).min(n)]
                        .iter()
                        .map(|f| compiler.compile(f).map_err(|e| e.to_string()))
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("a worker panicked".into())))
            .collect()
    });
    let mut all = Vec::with_capacity(n);
    for part in parts {
        all.extend(part?);
    }
    rc_emit_wasm::link(&all, &p.modules).map_err(|e| e.to_string())
}

pub fn lift_native(
    games: &Path,
    paths: &[PathBuf],
    targets: &[String],
    out: Option<&Path>,
) -> Result<ExitCode, String> {
    // `wasm`: the web shell's module, code-wasm.wasm.
    let wasm = targets.iter().any(|t| t == "wasm");
    let targets: Vec<Target> = targets
        .iter()
        .filter(|t| *t != "wasm")
        .map(|t| target(t))
        .collect::<Result<_, _>>()?;
    let files = crate::lift::expand(games, paths)?;
    for (l, p) in programs(games, &files)? {
        let l = &l;
        let k = l.known.as_ref().expect("known");
        let dir = match out {
            Some(d) => d.to_path_buf(),
            None => {
                let d = default_out(games, l);
                d.parent()
                    .unwrap_or(Path::new("."))
                    .join("native")
                    .join(d.file_name().unwrap_or_default())
            }
        };
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if wasm {
            let module = compile_wasm(&p)?;
            let path = dir.join("code-wasm.wasm");
            fs::write(&path, &module).map_err(|e| format!("{}: {e}", path.display()))?;
            println!(
                "wasm        {} + {} DLLs, {} functions → {} ({:.1} MB)",
                k.binary.name,
                p.modules.len(),
                p.functions.len(),
                path.display(),
                module.len() as f64 / 1e6
            );
        }
        for &t in &targets {
            let blob = compile(t, &p)?;
            let path = dir.join(format!("code-{}.bin", t.name()));
            fs::write(&path, &blob).map_err(|e| format!("{}: {e}", path.display()))?;
            println!(
                "native      {} + {} DLLs, {} functions → {} ({:.1} MB)",
                k.binary.name,
                p.modules.len(),
                p.functions.len(),
                path.display(),
                blob.len() as f64 / 1e6
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
