// SPDX-License-Identifier: MIT
//! From a game's binaries (their bytes) to its lifted programs, without touching the file system:
//! what `recomp lift --native` and the converter in the browser share.
//!
//! - [`manifests`]: the games' manifests, built into the crate.
//! - [`find`] and [`load`]: a binary as the manifests know it, discovered.
//! - [`lift_map`] and [`apply_patches`]: its functions in the lifter's IR, patched.
//! - [`programs`]: one program per executable with the DLLs lifted with it.

use std::collections::{BTreeMap, BTreeSet};

use rc_discover::{CodeMap, Function, Image, Options};
use rc_ir::Stmt;
use rc_manifest::{Binary, Manifest, Role, Treatment};

use crate::Error;

/// The games' manifests (`games/*/game.toml`), as this crate was built with them.
pub fn manifests() -> Result<Vec<Manifest>, Error> {
    include!(concat!(env!("OUT_DIR"), "/manifests.rs"))
        .iter()
        .map(|(origin, text)| {
            rc_manifest::parse_str(text, std::path::Path::new(origin)).map_err(|e| e.to_string())
        })
        .collect()
}

/// A binary as a manifest knows it.
#[derive(Clone, Debug)]
pub struct Known {
    pub game: String,
    pub binary: Binary,
}

/// The binary with this hash. One build can belong to several games (Cultures 2 ships the German
/// Cultures 1 `binkw32.dll`): then the game `prefer` says wins (the one the path names, or the
/// one being converted), else the first.
pub fn find(manifests: &[Manifest], sha256: &str, prefer: &dyn Fn(&str) -> bool) -> Option<Known> {
    let hits: Vec<Known> = manifests
        .iter()
        .flat_map(|m| {
            m.binaries
                .iter()
                .filter(|b| b.sha256 == sha256)
                .map(|b| Known {
                    game: m.game.id.clone(),
                    binary: b.clone(),
                })
        })
        .collect();
    let pick = hits.iter().position(|k| prefer(&k.game)).unwrap_or(0);
    hits.into_iter().nth(pick)
}

/// Discovery's options from the manifest's hints.
pub fn options(known: Option<&Known>) -> Options {
    let mut o = Options::default();
    if let Some(h) = known.and_then(|k| k.binary.discover.as_ref()) {
        o.seeds = h.seeds.clone();
        o.no_return = h.no_return.clone();
        o.returns = h.returns.clone();
        o.data = h.data.iter().map(|r| (r.start, r.end)).collect();
        o.jump_tables = h
            .jump_tables
            .iter()
            .map(|t| (t.at, t.table, t.count))
            .collect();
    }
    o
}

/// A binary loaded (rebased, patched) and discovered.
pub struct Loaded {
    pub image: Image,
    pub map: CodeMap,
    pub known: Option<Known>,
    pub sha256: String,
}

/// Load and discover `data` (the file `name`, its manifest entry `known` if any; `base` rebases
/// it, else the manifest's base does).
pub fn load(
    data: &[u8],
    name: &str,
    known: Option<Known>,
    base: Option<u32>,
    use_relocs: bool,
) -> Result<Loaded, Error> {
    let sha256 = rc_manifest::sha256_hex(data);
    let pe = rc_pe::parse(data).map_err(|e| format!("{name}: {e}"))?;
    let base = base.or(known.as_ref().and_then(|k| k.binary.base));
    let mut image = Image::load(data, &pe, base).map_err(|e| format!("{name}: {e}"))?;
    // The game project's code patches change the bytes before anything looks at them.
    if let Some(k) = &known {
        for c in &k.binary.patch.code {
            let (from, to) = c.bytes()?;
            image.patch(c.at, &from, &to)?;
        }
    }
    let name = known
        .as_ref()
        .map_or_else(|| name.to_string(), |k| k.binary.name.clone());
    let mut opts = options(known.as_ref());
    opts.use_relocs = use_relocs;
    let map = rc_discover::discover(&image, &opts, &name, &sha256);
    Ok(Loaded {
        image,
        map,
        known,
        sha256,
    })
}

/// The labels other code jumps to inside a function (not switch cases): each gets an entry of
/// its own.
pub fn label_entries(map: &CodeMap) -> Vec<(u32, &Function)> {
    let cases: BTreeSet<u32> = map
        .jump_tables
        .iter()
        .flat_map(|t| t.targets.iter().map(|v| v.0))
        .collect();
    let mut owner: BTreeMap<u32, &Function> = BTreeMap::new();
    for f in &map.functions {
        for &a in &f.body {
            owner.insert(a, f);
        }
    }
    map.labels
        .iter()
        .map(|l| l.va.0)
        .filter(|va| !cases.contains(va))
        .filter_map(|va| owner.get(&va).map(|f| (va, *f)))
        .collect()
}

/// The `__except` blocks of the binary: the handlers of SEH scope-table entries that have a
/// filter (an entry without one is a `__finally` block, which returns). `__except_handler3`
/// calls such a block, and the block continues in its own function's frame: entered during an
/// exception, it unwinds the lifted functions to that frame first (rc_unwind_begin).
pub fn except_blocks(l: &Loaded) -> BTreeMap<u32, u32> {
    let mut out = BTreeMap::new();
    for t in &l.map.seh_scope_tables {
        for i in 0..t.entries {
            let e = t.table.0 + 12 * i;
            let (filter, handler) = (l.image.dword(e + 4), l.image.dword(e + 8));
            if let (Some(f @ 1..), Some(h @ 1..)) = (filter, handler) {
                let _ = f;
                out.insert(h, t.frame.0);
            }
        }
    }
    out
}

/// Where an exception may continue inside a function's frame: the `__except` blocks of its SEH
/// frame, and the continuations its C++ catch blocks return (a catch funclet ends by loading the
/// address to continue at into eax). Each place with the address of the instruction that set up
/// the frame (the SEH `push -1`, or the `push` of the C++ frame handler stub), by which the
/// lifted function that owns the frame, and every entry into its body, finds it.
pub fn continuations(l: &Loaded) -> BTreeMap<u32, u32> {
    let img = &l.image;
    let d = |a: u32| img.dword(a).unwrap_or(0);
    let mut out: BTreeMap<u32, u32> = except_blocks(l);
    let funcinfos: BTreeSet<u32> = l.map.eh_funcinfo.iter().map(|v| v.0).collect();
    let by_entry: BTreeMap<u32, &Function> =
        l.map.functions.iter().map(|f| (f.entry.0, f)).collect();
    // FuncInfo -> the continuations of its catch blocks.
    let mut of_funcinfo: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &fi in &funcinfos {
        let (ntry, tries) = (d(fi + 12).min(1024), d(fi + 16));
        let mut conts = Vec::new();
        for t in 0..ntry {
            let e = tries + 20 * t;
            let (nc, hs) = (d(e + 12).min(256), d(e + 16));
            for c in 0..nc {
                let Some(funclet) = by_entry.get(&d(hs + 16 * c + 12)) else {
                    continue;
                };
                for &a in &funclet.body {
                    // mov eax, imm32 with a code address
                    if img.byte(a) == Some(0xb8) && img.is_exec(d(a + 1)) {
                        conts.push(d(a + 1));
                    }
                }
            }
        }
        of_funcinfo.insert(fi, conts);
    }
    // The functions that push a frame handler stub (mov eax, FuncInfo; jmp __CxxFrameHandler).
    for f in &l.map.functions {
        for &a in &f.body {
            if img.byte(a) != Some(0x68) {
                continue;
            }
            let stub = d(a + 1);
            if img.byte(stub) != Some(0xb8) || img.byte(stub + 5) != Some(0xe9) {
                continue;
            }
            if let Some(conts) = of_funcinfo.get(&d(stub + 1)) {
                for &c in conts {
                    out.entry(c).or_insert(a);
                }
            }
        }
    }
    out
}

pub struct Lifted {
    pub functions: Vec<rc_ir::Function>,
    pub label_entries: usize,
    /// Trap reason (without its address) → count.
    pub traps: BTreeMap<String, u32>,
    pub instructions: usize,
}

/// Every function and label entry of a loaded binary in the lifter's IR, by address.
pub fn lift_map(l: &Loaded) -> Lifted {
    let mut functions: Vec<rc_ir::Function> = l
        .map
        .functions
        .iter()
        .map(|f| rc_x86::lift_function(&l.image, &l.map, f))
        .collect();
    let labels = label_entries(&l.map);
    for (va, f) in &labels {
        functions.push(rc_x86::lift_entry(&l.image, &l.map, f, *va));
    }
    functions.sort_by_key(|f| f.addr);
    // Exceptions: which functions are __except blocks, and where exceptions continue in each
    // function's frame (by the instruction that set the frame up, which every entry into the
    // function's body contains).
    let excepts = except_blocks(l);
    let conts = continuations(l);
    for f in &mut functions {
        f.except_block = excepts.contains_key(&f.addr);
        let own: BTreeSet<u32> = f.insns.iter().map(|i| i.addr).collect();
        f.continuations = conts
            .iter()
            .filter(|(_, frame)| own.contains(frame))
            .map(|(c, _)| *c)
            .collect();
    }
    let mut traps: BTreeMap<String, u32> = BTreeMap::new();
    let mut instructions = 0;
    for f in &functions {
        instructions += f.insns.len();
        for i in &f.insns {
            // The first trap of an instruction is the one that stops it.
            if let Some(t) = i.stmts.iter().find_map(|s| match s {
                Stmt::Trap(t) => Some(t),
                _ => None,
            }) {
                let reason = t.split(" at 0x").next().unwrap_or(t).to_string();
                *traps.entry(reason).or_insert(0) += 1;
            }
        }
    }
    Lifted {
        functions,
        label_entries: labels.len(),
        traps,
        instructions,
    }
}

/// The game project's patches on the lifted program (`[binary.patch]` hooks, points and
/// operands; see rc_manifest::Patch), as TropicoRecomp applies them.
pub fn apply_patches(
    functions: &mut [rc_ir::Function],
    p: &rc_manifest::Patch,
) -> Result<(), Error> {
    let hooks: BTreeSet<u32> = p.hooks.iter().copied().collect();
    for h in &hooks {
        if functions.binary_search_by_key(h, |f| f.addr).is_err() {
            return Err(format!("[patch] hook {h:#010x} is not a function entry"));
        }
    }
    if !hooks.is_empty() {
        for f in functions.iter_mut() {
            if let Some(at) = rc_ir::route_through_dispatch(f, &hooks).first() {
                return Err(format!(
                    "[patch] conditional jump to a hooked function at {at:#010x}"
                ));
            }
        }
    }
    for &at in &p.points {
        let n = functions
            .iter_mut()
            .filter(|f| f.insns.first().is_some_and(|i| i.addr <= at))
            .map(|f| usize::from(rc_ir::insert_point(f, at)))
            .sum::<usize>();
        if n == 0 {
            return Err(format!(
                "[patch] point {at:#010x} is not a lifted instruction"
            ));
        }
    }
    for o in &p.operands {
        let n: usize = functions
            .iter_mut()
            .map(|f| rc_ir::load_operand(f, o.at, o.value, o.load))
            .sum();
        if n == 0 {
            return Err(format!(
                "[patch] operand {:#x} not found in the instruction at {:#010x}",
                o.value, o.at
            ));
        }
    }
    Ok(())
}

/// The functions of one executable and the DLLs lifted with it, patched; the DLLs by name, with
/// `@base` when the manifest rebases them (the runtime's loader puts them there).
pub struct Program {
    /// The executable's name as the manifest lists it.
    pub exe: String,
    pub functions: Vec<rc_ir::Function>,
    pub modules: Vec<String>,
}

/// The programs of an install's lifted binaries (`loaded`, all known to the manifests): one per
/// executable that is not a multiplayer build, each with all the DLLs lifted with it.
pub fn programs(loaded: &[Loaded]) -> Result<Vec<Program>, Error> {
    let mut exes = Vec::new();
    let mut dlls = Vec::new();
    for l in loaded {
        let Some(k) = l.known.as_ref() else {
            return Err(format!("{}: not in any manifest", l.map.binary.name));
        };
        if k.binary.treatment == Treatment::Ignore || matches!(k.binary.role, Role::Multiplayer) {
            continue;
        }
        if matches!(k.binary.role, Role::Library) {
            dlls.push((l, k));
        } else {
            exes.push((l, k));
        }
    }
    let mut dll_functions = Vec::new();
    let mut modules = Vec::new();
    for (l, k) in &dlls {
        let mut lifted = lift_map(l);
        apply_patches(&mut lifted.functions, &k.binary.patch)?;
        dll_functions.extend(lifted.functions);
        modules.push(match k.binary.base {
            Some(b) => format!("{}@{b:x}", k.binary.name),
            None => k.binary.name.clone(),
        });
    }
    let mut out = Vec::new();
    for (l, k) in exes {
        let mut lifted = lift_map(l);
        apply_patches(&mut lifted.functions, &k.binary.patch)?;
        let mut functions = lifted.functions;
        functions.extend(dll_functions.iter().cloned());
        functions.sort_by_key(|f| f.addr);
        functions.dedup_by_key(|f| f.addr);
        out.push(Program {
            exe: k.binary.name.clone(),
            functions,
            modules: modules.clone(),
        });
    }
    Ok(out)
}

/// The game and variant of an install, by the hash of an executable (the main one first; DLLs
/// only as a last resort: the same `binkw32.dll` ships with several games). `file(name)` returns
/// a file of the install (any case) or None.
pub fn identify<'a>(
    ms: &'a [Manifest],
    file: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Option<(&'a Manifest, String)> {
    let rank = |r: Role| match r {
        Role::Main => 0,
        Role::Library => 2,
        _ => 1,
    };
    for pass in 0..3 {
        for m in ms {
            for b in m.binaries.iter().filter(|b| rank(b.role) == pass) {
                let Some(data) = file(&b.name) else {
                    continue;
                };
                if rc_manifest::sha256_hex(&data) == b.sha256
                    && let Some(v) = b.variants.first()
                {
                    return Some((m, v.clone()));
                }
            }
        }
    }
    None
}

/// The lifted programs of an install (`file` as for [`identify`]): its game, variant and
/// programs.
pub fn lift_install(
    ms: &[Manifest],
    file: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Result<(String, String, Vec<Program>), Error> {
    let (m, variant) = identify(ms, file).ok_or("no known executable in it")?;
    let mut loaded = Vec::new();
    let mut seen = BTreeSet::new();
    for b in m
        .binaries
        .iter()
        .filter(|b| b.treatment == Treatment::Lift && b.variants.contains(&variant))
    {
        if !seen.insert(b.name.to_ascii_lowercase()) {
            continue;
        }
        let Some(data) = file(&b.name) else {
            continue;
        };
        let sha = rc_manifest::sha256_hex(&data);
        let known = find(ms, &sha, &|g| g == m.game.id);
        loaded.push(load(&data, &b.name, known, None, true)?);
    }
    if loaded.is_empty() {
        return Err("nothing to lift".into());
    }
    Ok((m.game.id.clone(), variant, programs(&loaded)?))
}

/// Compile a program to the web shell's WebAssembly module (rc-emit-wasm); `progress` hears
/// each function compiled.
pub fn compile_wasm(p: &Program, progress: &mut dyn FnMut(usize)) -> Result<Vec<u8>, Error> {
    let addrs: BTreeSet<u32> = p.functions.iter().map(|f| f.addr).collect();
    let compiler = rc_emit_wasm::Compiler::new(&addrs, rc_emit_wasm::Options::default());
    let mut all = Vec::with_capacity(p.functions.len());
    for f in &p.functions {
        all.push(compiler.compile(f).map_err(|e| e.to_string())?);
        progress(1);
    }
    rc_emit_wasm::link(&all, &p.modules).map_err(|e| e.to_string())
}

/// Compile a program to native code for `target` (rc-emit-native) on one thread; `progress`
/// hears each function compiled.
pub fn compile_native(
    target: rc_emit_native::Target,
    p: &Program,
    progress: &mut dyn FnMut(usize),
) -> Result<Vec<u8>, Error> {
    let compiler = rc_emit_native::Compiler::new(target, rc_emit_native::Options::default())
        .map_err(|e| e.to_string())?;
    let entries: BTreeSet<u32> = p.functions.iter().map(|f| f.addr).collect();
    let mut all = Vec::with_capacity(p.functions.len());
    for f in &p.functions {
        all.push(compiler.compile(f, &entries).map_err(|e| e.to_string())?);
        progress(1);
    }
    rc_emit_native::link(target, &all, &p.modules).map_err(|e| e.to_string())
}
