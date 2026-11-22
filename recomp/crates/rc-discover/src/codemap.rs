// SPDX-License-Identifier: MIT
//! The code map: what discovery found, as the lifter reads it (`codemap.json`), plus the
//! coverage report. It holds addresses and sizes only, never code bytes.

use crate::engine::{
    decode_at, Engine, Flow as EFlow, Partition, Source, BODY, DATA, INSN, PAD, UNK,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

pub const SCHEMA: u32 = 1;

/// How control leaves an instruction: what the lifter needs from discovery. Import slots and
/// jump tables are already resolved. (The same shape as TropicoRecomp's, so the lifter is
/// shared.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Next,
    Jump(u32),
    Cond(u32),
    Call(u32),
    CallIndirect,
    /// `call [slot]` through the import address table.
    CallImport(u32),
    Ret,
    /// `jmp reg` or `jmp [mem]` that is not a known table or import: dispatched at run time.
    JumpIndirect,
    /// `jmp [idx*4 + table]`; the table is the [`JumpTable`] whose `at` is this instruction.
    JumpTable,
    /// `jmp [slot]` through the import address table.
    JumpImport(u32),
    /// `int3`, `hlt`, `ud2`, `int n`: execution does not continue.
    Stop,
}

impl From<EFlow> for Flow {
    fn from(f: EFlow) -> Flow {
        match f {
            EFlow::Next => Flow::Next,
            EFlow::Jcc(t) => Flow::Cond(t),
            EFlow::Jmp(t) => Flow::Jump(t),
            EFlow::Call(t) => Flow::Call(t),
            EFlow::CallImp { slot, .. } => Flow::CallImport(slot),
            EFlow::JmpImp(slot) => Flow::JumpImport(slot),
            EFlow::CallInd => Flow::CallIndirect,
            EFlow::JmpInd => Flow::JumpIndirect,
            EFlow::Table(_) => Flow::JumpTable,
            EFlow::Ret => Flow::Ret,
            EFlow::Stop => Flow::Stop,
        }
    }
}

/// One decoded instruction (kept in memory for the lifter, not written to JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insn {
    pub len: u8,
    pub flow: Flow,
}

/// A virtual address, written as `"0x00401000"` in JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Va(pub u32);

impl Serialize for Va {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:#010x}", self.0))
    }
}

impl<'de> Deserialize<'de> for Va {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let h = s
            .strip_prefix("0x")
            .ok_or_else(|| serde::de::Error::custom("expected 0x…"))?;
        u32::from_str_radix(h, 16)
            .map(Va)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeMap {
    pub schema: u32,
    pub binary: BinaryInfo,
    pub coverage: Vec<SectionCoverage>,
    pub stats: Stats,
    pub functions: Vec<Function>,
    /// Addresses taken as values (code or data constants) that lie inside a function body.
    pub labels: Vec<Label>,
    pub jump_tables: Vec<JumpTable>,
    pub vtables: Vec<Vtable>,
    pub seh_scope_tables: Vec<ScopeTable>,
    pub eh_funcinfo: Vec<Va>,
    /// Data inside code sections.
    pub data: Vec<DataRange>,
    pub padding: Vec<[Va; 2]>,
    /// Unreachable code after calls that do not return (the caller's stack clean-up).
    pub dead_code: Vec<[Va; 2]>,
    pub unknown: Vec<[Va; 2]>,
    /// `dll!name` → number of `call [IAT]` / `jmp [IAT]` sites.
    pub imports_called: BTreeMap<String, u32>,
    pub indirect_jumps: Vec<Va>,
    /// `[callee, call site]`: calls that fall straight into another function. Candidates for
    /// `no_return` hints; check them by hand (see `Engine::no_return_evidence`).
    pub no_return_candidates: Vec<[Va; 2]>,
    pub cpu_features: BTreeMap<String, u32>,
    pub warnings: Vec<Warning>,
    /// Candidates that failed trial decoding, with the reason.
    pub rejected: Vec<Warning>,
    /// DLLs only: the result checked against the base relocations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reloc_check: Option<RelocCheck>,
    /// Every instruction discovery decoded, by address (in memory only).
    #[serde(skip)]
    pub insns: BTreeMap<u32, Insn>,
}

impl CodeMap {
    /// The function whose entry is `va`.
    pub fn function(&self, va: u32) -> Option<&Function> {
        self.functions
            .binary_search_by_key(&va, |f| f.entry.0)
            .ok()
            .map(|i| &self.functions[i])
    }

    /// The jump table used by the `jmp` at `va`.
    pub fn jump_table(&self, va: u32) -> Option<&JumpTable> {
        self.jump_tables.iter().find(|t| t.at.0 == va)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelocCheck {
    /// Whether discovery used the relocations (then the check is partly circular).
    pub used_relocs: bool,
    pub sites_in_code: u32,
    /// Fixup sites in code that are neither inside an instruction operand nor data.
    pub bad_sites: Vec<Va>,
    pub targets_in_code: u32,
    /// `[site, value]`: fixed-up pointers into code that hit no instruction start or data.
    pub bad_targets: Vec<[Va; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryInfo {
    pub name: String,
    pub sha256: String,
    pub base: Va,
    pub preferred_base: Va,
    pub entry: Va,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SectionCoverage {
    pub section: String,
    pub va: Va,
    pub size: u32,
    pub code: u32,
    pub data: u32,
    pub padding: u32,
    pub unknown: u32,
}

impl SectionCoverage {
    /// Classified share (code + data + padding) in percent.
    pub fn classified_percent(&self) -> f64 {
        if self.size == 0 {
            return 100.0;
        }
        f64::from(self.size - self.unknown) * 100.0 / f64::from(self.size)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    /// Discovery runs until the no-return set and the cross jumps were stable.
    pub iterations: u32,
    /// Instructions that more than one function reaches (should be zero after the joins).
    pub shared_instructions: u32,
    /// Decoded instructions that no function reaches (dead code after no-return calls).
    pub orphan_instructions: u32,
    pub instructions: u32,
    pub rejected_candidates: u32,
    pub no_return_functions: u32,
    pub indirect_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Function {
    pub entry: Va,
    /// `normal`, `thunk` (a single `jmp` to another function) or `import_thunk`
    /// (a single `jmp [IAT]`).
    pub kind: String,
    /// Contiguous byte ranges of the body, `[start, end)`.
    pub chunks: Vec<[Va; 2]>,
    pub size: u32,
    pub instructions: u32,
    /// Basic block starts, ascending (the entry, branch targets, fall-through after
    /// conditional branches and calls).
    pub blocks: Vec<Va>,
    /// Other functions this one transfers to without a call (jump or fall-through).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tail_calls: Vec<Va>,
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_return: bool,
    /// Addresses of the instructions this function owns, ascending (in memory only).
    #[serde(skip)]
    pub body: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub va: Va,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JumpTable {
    /// The `jmp [idx*4 + table]` instruction.
    pub at: Va,
    /// Address of the first entry in `targets`.
    pub table: Va,
    pub count: u32,
    /// Entry `k` of `targets` is used for index `first_index + k`: the slot at
    /// `disp + 4 * (first_index + k)`, with `disp` the jump's displacement. Negative for the
    /// CRT's `memcpy` tables, 1 for a switch indexed from 1.
    pub first_index: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_table: Option<IndexTable>,
    pub bound: String,
    pub targets: Vec<Va>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexTable {
    pub va: Va,
    pub len: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vtable {
    pub va: Va,
    /// The instruction that stores the vtable pointer (typically in a constructor).
    pub stored_at: Va,
    pub entries: Vec<Va>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeTable {
    /// The `push -1` that starts the SEH frame set-up.
    pub frame: Va,
    pub table: Va,
    pub handler: Va,
    pub entries: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataRange {
    pub start: Va,
    pub end: Va,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub va: Va,
    pub message: String,
}

fn runs(st: &[u8], base: u32, want: u8) -> Vec<[Va; 2]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < st.len() {
        if st[i] != want {
            i += 1;
            continue;
        }
        let j = st[i..]
            .iter()
            .position(|&b| b != want)
            .map_or(st.len(), |k| i + k);
        out.push([Va(base + i as u32), Va(base + j as u32)]);
        i = j;
    }
    out
}

pub(crate) fn build(
    e: &Engine,
    part: &Partition,
    name: &str,
    sha256: &str,
    iterations: u32,
    used_relocs: bool,
) -> CodeMap {
    let img = e.img;
    let no_return = &part.no_return;
    let mut owned = HashSet::new();
    let mut functions = Vec::with_capacity(e.funcs.len());
    for (&entry, &bits) in &e.funcs {
        let b = &part.bodies[&entry];
        let mut chunks: Vec<[Va; 2]> = Vec::new();
        let mut size = 0u32;
        for &va in &b.insns {
            owned.insert(va);
            let len = u32::from(e.ins[&va].len);
            size += len;
            match chunks.last_mut() {
                Some(c) if c[1].0 == va => c[1] = Va(va + len),
                _ => chunks.push([Va(va), Va(va + len)]),
            }
        }
        let kind = match b.insns.iter().next().map(|a| e.ins[a].flow) {
            Some(EFlow::Jmp(t)) if b.insns.len() == 1 && t != entry && e.funcs.contains_key(&t) => {
                "thunk"
            }
            Some(EFlow::JmpImp(_)) if b.insns.len() == 1 => "import_thunk",
            _ => "normal",
        };
        functions.push(Function {
            entry: Va(entry),
            kind: kind.into(),
            chunks,
            size,
            instructions: b.insns.len() as u32,
            blocks: b.blocks.iter().map(|&v| Va(v)).collect(),
            tail_calls: b.tails.iter().map(|&v| Va(v)).collect(),
            sources: Source::names(bits).into_iter().map(String::from).collect(),
            no_return: no_return.contains(&entry),
            body: b.insns.iter().copied().collect(),
        });
    }

    let mut coverage = Vec::new();
    let mut padding = Vec::new();
    let mut unknown = Vec::new();
    for (va, st) in e.ranges() {
        let sec = img.section_of(va).map_or(String::new(), |s| s.name.clone());
        let count = |want: &[u8]| st.iter().filter(|b| want.contains(b)).count() as u32;
        coverage.push(SectionCoverage {
            section: sec,
            va: Va(va),
            size: st.len() as u32,
            code: count(&[INSN, BODY]),
            data: count(&[DATA]),
            padding: count(&[PAD]),
            unknown: count(&[UNK]),
        });
        padding.extend(runs(st, va, PAD));
        unknown.extend(runs(st, va, UNK));
    }

    let mut imports_called = BTreeMap::new();
    let mut indirect_jumps = Vec::new();
    let mut indirect_calls = 0u32;
    let mut cpu_features: BTreeMap<String, u32> = BTreeMap::new();
    let mut addrs: Vec<u32> = e.ins.keys().copied().collect();
    addrs.sort_unstable();
    for &va in &addrs {
        match e.ins[&va].flow {
            EFlow::CallImp { slot, .. } | EFlow::JmpImp(slot) => {
                *imports_called
                    .entry(img.imports[&slot].clone())
                    .or_insert(0) += 1;
            }
            EFlow::JmpInd => indirect_jumps.push(Va(va)),
            EFlow::CallInd => indirect_calls += 1,
            _ => {}
        }
        if let Some(i) = decode_at(img, va) {
            for f in i.cpuid_features() {
                *cpu_features.entry(format!("{f:?}")).or_insert(0) += 1;
            }
        }
    }

    let mut data: Vec<DataRange> = e
        .data
        .iter()
        .map(|&(s, end, k)| DataRange {
            start: Va(s),
            end: Va(end),
            kind: k.name().into(),
        })
        .collect();
    data.sort_by_key(|d| (d.start, d.end));
    // Merge touching or overlapping ranges of the same kind (tables are marked per entry).
    let mut merged: Vec<DataRange> = Vec::with_capacity(data.len());
    for d in data {
        match merged.last_mut() {
            Some(l) if l.kind == d.kind && d.start <= l.end => l.end = l.end.max(d.end),
            _ => merged.push(d),
        }
    }
    let data = merged;

    let mut warnings: Vec<Warning> = e
        .warnings
        .iter()
        .map(|(va, m)| Warning {
            va: Va(*va),
            message: m.clone(),
        })
        .collect();
    warnings.sort_by_key(|w| w.va);
    warnings.dedup_by(|a, b| a.va == b.va && a.message == b.message);

    let mut eh_funcinfo: Vec<Va> = e.eh_funcinfos.iter().map(|&v| Va(v)).collect();
    eh_funcinfo.sort();

    CodeMap {
        schema: SCHEMA,
        binary: BinaryInfo {
            name: name.into(),
            sha256: sha256.into(),
            base: Va(img.base),
            preferred_base: Va(img.preferred_base),
            entry: Va(img.entry),
        },
        coverage,
        stats: Stats {
            iterations,
            shared_instructions: part.shared.len() as u32,
            orphan_instructions: e.ins.keys().filter(|a| !owned.contains(a)).count() as u32,
            instructions: e.ins.len() as u32,
            rejected_candidates: e.rejected.len() as u32,
            no_return_functions: no_return.len() as u32,
            indirect_calls,
        },
        functions,
        labels: e
            .labels
            .iter()
            .filter(|(va, _)| !e.funcs.contains_key(va))
            .map(|(&va, &bits)| Label {
                va: Va(va),
                sources: Source::names(bits).into_iter().map(String::from).collect(),
            })
            .collect(),
        jump_tables: {
            let mut t: Vec<JumpTable> = e
                .tables
                .iter()
                .map(|t| JumpTable {
                    at: Va(t.at),
                    table: Va(t.table),
                    count: t.count,
                    first_index: (t.table.wrapping_sub(t.disp) as i32) / 4,
                    index_table: t
                        .index_table
                        .map(|(va, len)| IndexTable { va: Va(va), len }),
                    bound: t.bound.name().into(),
                    targets: t.targets.iter().map(|&v| Va(v)).collect(),
                })
                .collect();
            t.sort_by_key(|t| t.at);
            t
        },
        vtables: e
            .vtables
            .iter()
            .map(|(&va, v)| Vtable {
                va: Va(va),
                stored_at: Va(v.from),
                entries: v.entries.iter().map(|&x| Va(x)).collect(),
            })
            .collect(),
        seh_scope_tables: {
            let mut s: Vec<ScopeTable> = e
                .scope_tables
                .iter()
                .map(|s| ScopeTable {
                    frame: Va(s.frame),
                    table: Va(s.table),
                    handler: Va(s.handler),
                    entries: s.entries,
                })
                .collect();
            s.sort_by_key(|s| s.frame);
            s
        },
        eh_funcinfo,
        data,
        padding,
        dead_code: {
            let mut d: Vec<[Va; 2]> = e.dead.iter().map(|&(a, b)| [Va(a), Va(b)]).collect();
            d.sort();
            d
        },
        unknown,
        imports_called,
        indirect_jumps,
        no_return_candidates: e
            .no_return_evidence()
            .into_iter()
            .map(|(t, va)| [Va(t), Va(va)])
            .collect(),
        cpu_features,
        warnings,
        rejected: e
            .rejected
            .iter()
            .map(|(va, src, why)| Warning {
                va: Va(*va),
                message: format!("{}: {why}", src.name()),
            })
            .collect(),
        reloc_check: e.check_relocs().map(|c| RelocCheck {
            used_relocs,
            sites_in_code: c.sites,
            bad_sites: c.bad_sites.iter().map(|&v| Va(v)).collect(),
            targets_in_code: c.targets,
            bad_targets: c.bad_targets.iter().map(|&(a, b)| [Va(a), Va(b)]).collect(),
        }),
        insns: e
            .ins
            .iter()
            .map(|(&va, i)| {
                (
                    va,
                    Insn {
                        len: i.len,
                        flow: i.flow.into(),
                    },
                )
            })
            .collect(),
    }
}

impl CodeMap {
    /// Code + data + padding over all code sections, in percent.
    pub fn classified_percent(&self) -> f64 {
        let size: u64 = self.coverage.iter().map(|c| u64::from(c.size)).sum();
        let unk: u64 = self.coverage.iter().map(|c| u64::from(c.unknown)).sum();
        if size == 0 {
            100.0
        } else {
            (size - unk) as f64 * 100.0 / size as f64
        }
    }

    /// Human-readable coverage report (addresses and counts only).
    pub fn report(&self, verbose: bool) -> String {
        let mut o = String::new();
        let b = &self.binary;
        let _ = writeln!(
            o,
            "binary      {} ({})",
            b.name,
            &b.sha256[..b.sha256.len().min(16)]
        );
        if b.base != b.preferred_base {
            let _ = writeln!(
                o,
                "base        {:#010x} (rebased from {:#010x})",
                b.base.0, b.preferred_base.0
            );
        } else {
            let _ = writeln!(o, "base        {:#010x}", b.base.0);
        }
        let _ = writeln!(o, "entry       {:#010x}", b.entry.0);
        let _ = writeln!(o, "coverage");
        for c in &self.coverage {
            let pct = |n: u32| f64::from(n) * 100.0 / f64::from(c.size.max(1));
            let _ = writeln!(
                o,
                "  {:8} {:#010x} {:>8} bytes: code {:6.2} %  data {:5.2} %  padding {:5.2} %  unknown {:5.2} %  → classified {:6.2} %",
                c.section,
                c.va.0,
                c.size,
                pct(c.code),
                pct(c.data),
                pct(c.padding),
                pct(c.unknown),
                c.classified_percent()
            );
        }
        let count = |src: &str| {
            self.functions
                .iter()
                .filter(|f| f.sources.iter().any(|s| s == src))
                .count()
        };
        let _ = writeln!(
            o,
            "functions   {} ({} no-return, {} thunks); by source: call {}, code pointer {}, data pointer {}, reloc {}, vtable {}, seh {}, eh {}, export {}, gap {}, cross jump {}",
            self.functions.len(),
            self.stats.no_return_functions,
            self.functions.iter().filter(|f| f.kind != "normal").count(),
            count("call"),
            count("code_pointer"),
            count("data_pointer"),
            count("reloc"),
            count("vtable"),
            count("seh_filter") + count("seh_handler"),
            count("eh_unwind") + count("eh_catch"),
            count("export"),
            count("gap"),
            count("cross_jump"),
        );
        let bounds = |k: &str| self.jump_tables.iter().filter(|t| t.bound == k).count();
        let _ =
            writeln!(
            o,
            "jump tables {} (compare {}, mask {}, reloc {}, scan {}, hint {}; {} with index table)",
            self.jump_tables.len(),
            bounds("compare"),
            bounds("mask"),
            bounds("reloc"),
            bounds("scan"),
            bounds("hint"),
            self.jump_tables.iter().filter(|t| t.index_table.is_some()).count()
        );
        let _ = writeln!(
            o,
            "vtables     {} ({} slots); SEH scope tables {}; C++ EH FuncInfo {}; labels {}; dead code blocks {}",
            self.vtables.len(),
            self.vtables.iter().map(|v| v.entries.len()).sum::<usize>(),
            self.seh_scope_tables.len(),
            self.eh_funcinfo.len(),
            self.labels.len(),
            self.dead_code.len()
        );
        let _ = writeln!(
            o,
            "indirect    {} calls, {} jumps unresolved; {} imports called",
            self.stats.indirect_calls,
            self.indirect_jumps.len(),
            self.imports_called.len()
        );
        let feats: Vec<String> = self
            .cpu_features
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        let _ = writeln!(o, "cpu         {}", feats.join(", "));
        let _ = writeln!(
            o,
            "analysis    {} instructions ({} shared, {} in no function), {} rounds, {} rejected candidates, {} warnings",
            self.stats.instructions,
            self.stats.shared_instructions,
            self.stats.orphan_instructions,
            self.stats.iterations,
            self.stats.rejected_candidates,
            self.warnings.len()
        );
        if let Some(c) = &self.reloc_check {
            let _ = writeln!(
                o,
                "relocs      {} sites in code: {} bad; {} pointers into code: {} bad{}",
                c.sites_in_code,
                c.bad_sites.len(),
                c.targets_in_code,
                c.bad_targets.len(),
                if c.used_relocs {
                    ""
                } else {
                    " (discovery did not use them)"
                }
            );
        }
        let _ = writeln!(o, "classified  {:.2} %", self.classified_percent());
        if verbose {
            if let Some(c) = &self.reloc_check {
                for s in &c.bad_sites {
                    let _ = writeln!(o, "  bad reloc site    {:#010x}", s.0);
                }
                for t in &c.bad_targets {
                    let _ = writeln!(
                        o,
                        "  bad reloc target  {:#010x} -> {:#010x}",
                        t[0].0, t[1].0
                    );
                }
            }
            for u in &self.unknown {
                let _ = writeln!(
                    o,
                    "  unknown  {:#010x}..{:#010x} ({} bytes)",
                    u[0].0,
                    u[1].0,
                    u[1].0 - u[0].0
                );
            }
            for w in &self.warnings {
                let _ = writeln!(o, "  warning  {:#010x} {}", w.va.0, w.message);
            }
            for w in &self.rejected {
                let _ = writeln!(o, "  rejected {:#010x} {}", w.va.0, w.message);
            }
            for c in &self.no_return_candidates {
                let _ = writeln!(
                    o,
                    "  no-return? {:#010x} (call at {:#010x} falls into a function)",
                    c[0].0, c[1].0
                );
            }
            for j in &self.indirect_jumps {
                let _ = writeln!(o, "  indirect jump at {:#010x}", j.0);
            }
        }
        o
    }
}
