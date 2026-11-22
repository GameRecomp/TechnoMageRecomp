// SPDX-License-Identifier: MIT
//! The discovery engine: recursive descent from trusted roots, then candidate pointers
//! (code immediates, data constants, vtables, SEH and C++ EH tables) verified by trial
//! decoding, then gap filling. No-return functions are found by a fixed point over
//! whole discovery runs (see [`crate::discover`]).

use crate::image::Image;
use iced_x86::{
    ConditionCode, Decoder, DecoderOptions, FlowControl, Instruction, MemorySize, Mnemonic, OpKind,
    Register, RflagsBits,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Byte states in executable sections.
pub(crate) const UNK: u8 = 0;
pub(crate) const INSN: u8 = 1;
pub(crate) const BODY: u8 = 2;
pub(crate) const DATA: u8 = 3;
pub(crate) const PAD: u8 = 4;

/// Why an address is a function entry or label. Stored as a bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Entry = 0,
    Export,
    Seed,
    Call,
    CodePointer,
    DataPointer,
    Reloc,
    Vtable,
    SehFilter,
    SehHandler,
    EhUnwind,
    EhCatch,
    Gap,
    /// Code of one function runs into code of another here (shared code split off).
    CrossJump,
}

impl Source {
    pub const ALL: [Source; 14] = [
        Source::Entry,
        Source::Export,
        Source::Seed,
        Source::Call,
        Source::CodePointer,
        Source::DataPointer,
        Source::Reloc,
        Source::Vtable,
        Source::SehFilter,
        Source::SehHandler,
        Source::EhUnwind,
        Source::EhCatch,
        Source::Gap,
        Source::CrossJump,
    ];
    pub fn bit(self) -> u32 {
        1 << self as u32
    }
    pub fn name(self) -> &'static str {
        match self {
            Source::Entry => "entry",
            Source::Export => "export",
            Source::Seed => "seed",
            Source::Call => "call",
            Source::CodePointer => "code_pointer",
            Source::DataPointer => "data_pointer",
            Source::Reloc => "reloc",
            Source::Vtable => "vtable",
            Source::SehFilter => "seh_filter",
            Source::SehHandler => "seh_handler",
            Source::EhUnwind => "eh_unwind",
            Source::EhCatch => "eh_catch",
            Source::Gap => "gap",
            Source::CrossJump => "cross_jump",
        }
    }
    pub fn names(bits: u32) -> Vec<&'static str> {
        Source::ALL
            .iter()
            .filter(|s| bits & s.bit() != 0)
            .map(|s| s.name())
            .collect()
    }
}

/// Data kinds inside executable sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    JumpTable,
    IndexTable,
    Reloc,
    Hint,
    DataRef,
    String,
}

impl DataKind {
    pub fn name(self) -> &'static str {
        match self {
            DataKind::JumpTable => "jump_table",
            DataKind::IndexTable => "index_table",
            DataKind::Reloc => "reloc",
            DataKind::Hint => "hint",
            DataKind::DataRef => "data_ref",
            DataKind::String => "string",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Next,
    Jcc(u32),
    Jmp(u32),
    Call(u32),
    CallImp { slot: u32, noret: bool },
    JmpImp(u32),
    CallInd,
    JmpInd,
    Table(u32),
    Ret,
    Stop,
}

/// A function body (see [`Engine::body`]).
#[derive(Debug, Default)]
pub(crate) struct Body {
    pub insns: BTreeSet<u32>,
    pub edges: Vec<(u32, u32)>,
    pub blocks: BTreeSet<u32>,
    pub tails: BTreeSet<u32>,
    /// Leaves by `ret`, an unknown indirect jump or a jump to a returning import.
    pub exits: bool,
}

pub(crate) struct Partition {
    pub bodies: BTreeMap<u32, Body>,
    pub no_return: HashSet<u32>,
    pub joins: BTreeSet<u32>,
    pub shared: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Ins {
    pub len: u8,
    pub flow: Flow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// `cmp idx, n` + `ja`/`jae` before the jump.
    Compare,
    /// `and idx, mask`.
    Mask,
    /// From a manifest hint.
    Hint,
    /// No bound found: entries were read while they pointed into code.
    Scan,
    /// Bounded by the base relocations (DLLs).
    Reloc,
}

impl Bound {
    pub fn name(self) -> &'static str {
        match self {
            Bound::Compare => "compare",
            Bound::Mask => "mask",
            Bound::Hint => "hint",
            Bound::Scan => "scan",
            Bound::Reloc => "reloc",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Table {
    pub at: u32,
    pub table: u32,
    pub count: u32,
    pub index_table: Option<(u32, u32)>,
    pub bound: Bound,
    pub targets: Vec<u32>,
    /// The displacement in the jump: entry `k` of `targets` is at
    /// `disp + 4 * (first_index + k)`.
    pub disp: u32,
    /// No bound found yet: resolved by a scan once the rest of the code is known.
    pub pending: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Vtable {
    pub entries: Vec<u32>,
    pub from: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopeTable {
    pub frame: u32,
    pub table: u32,
    pub handler: u32,
    pub entries: u32,
}

/// Discovery options: the manifest hints plus switches.
#[derive(Debug, Clone)]
pub struct Options {
    pub seeds: Vec<u32>,
    pub no_return: Vec<u32>,
    /// Functions checked by hand to return although a call to them falls into another
    /// function (MSVC's x87 helpers): not reported as no-return candidates.
    pub returns: Vec<u32>,
    /// Data ranges in code sections (`end` exclusive).
    pub data: Vec<(u32, u32)>,
    /// `(jmp address, table address, count)`.
    pub jump_tables: Vec<(u32, u32, u32)>,
    /// Try to decode leftover gaps as functions (dead or only indirectly reachable code).
    pub gap_functions: bool,
    /// Use the base relocations (DLLs) as exact pointer information. Off only to validate
    /// the heuristics against them.
    pub use_relocs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            seeds: Vec::new(),
            no_return: Vec::new(),
            returns: Vec::new(),
            data: Vec::new(),
            jump_tables: Vec::new(),
            gap_functions: true,
            use_relocs: true,
        }
    }
}

/// Imports that never return.
const NORETURN_IMPORTS: &[&str] = &[
    "kernel32.dll!ExitProcess",
    "kernel32.dll!ExitThread",
    "kernel32.dll!FatalExit",
    "kernel32.dll!FatalAppExitA",
    "kernel32.dll!FatalAppExitW",
    "kernel32.dll!FreeLibraryAndExitThread",
    "msvcrt.dll!exit",
    "msvcrt.dll!_exit",
    "msvcrt.dll!abort",
    "msvcrt.dll!_CxxThrowException",
    "msvcrt.dll!?terminate@@YAXXZ",
    "msvcrt.dll!_amsg_exit",
    "msvcrt.dll!longjmp",
    "user32.dll!ExitWindowsEx",
];

/// Whether the import `dll!name` never returns. The C runtime's entries count for every
/// version of it (`msvcrt.dll`, `msvcr71.dll`, …).
fn is_noreturn_import(n: &str) -> bool {
    NORETURN_IMPORTS.iter().any(|x| x.eq_ignore_ascii_case(n))
        || n.split_once('!').is_some_and(|(dll, f)| {
            let dll = dll.to_ascii_lowercase();
            dll.starts_with("msvcr")
                && dll.ends_with(".dll")
                && NORETURN_IMPORTS
                    .iter()
                    .filter_map(|x| x.strip_prefix("msvcrt.dll!"))
                    .any(|x| x == f)
        })
}

/// The MSVC C++ exception code (`'msc' | 0xE0000000`).
const MSC_EXCEPTION: u32 = 0xE06D_7363;
/// `FuncInfo` magic of the MSVC 6 C++ EH tables.
const EH_MAGIC: u32 = 0x1993_0520;

struct Space {
    /// (va, size, offset into `state`)
    ranges: Vec<(u32, u32, usize)>,
    state: Vec<u8>,
}

impl Space {
    fn new(img: &Image) -> Self {
        let mut ranges = Vec::new();
        let mut off = 0usize;
        for s in img.sections.iter().filter(|s| s.exec) {
            ranges.push((s.va, s.size, off));
            off += s.size as usize;
        }
        Space {
            ranges,
            state: vec![UNK; off],
        }
    }
    fn idx(&self, va: u32) -> Option<usize> {
        self.ranges
            .iter()
            .find(|r| va >= r.0 && va - r.0 < r.1)
            .map(|r| r.2 + (va - r.0) as usize)
    }
    fn get(&self, va: u32) -> Option<u8> {
        self.idx(va).map(|i| self.state[i])
    }
}

#[derive(Default)]
struct TxLog {
    states: Vec<(usize, u8)>,
    ins: Vec<u32>,
    funcs: Vec<(u32, Option<u32>)>,
    labels: Vec<(u32, Option<u32>)>,
    tables: usize,
    data: usize,
    cands: usize,
    vt: usize,
    seh: usize,
    refs: usize,
    warnings: usize,
}

pub(crate) struct Engine<'a> {
    pub img: &'a Image,
    opts: &'a Options,
    sp: Space,
    pub ins: HashMap<u32, Ins>,
    pub funcs: BTreeMap<u32, u32>,
    pub labels: BTreeMap<u32, u32>,
    pub tables: Vec<Table>,
    pub data: Vec<(u32, u32, DataKind)>,
    pub vtables: BTreeMap<u32, Vtable>,
    pub scope_tables: Vec<ScopeTable>,
    pub eh_funcinfos: Vec<u32>,
    pub no_return: HashSet<u32>,
    pub warnings: Vec<(u32, String)>,
    pub rejected: Vec<(u32, Source, String)>,
    cands: Vec<(u32, Source, u32)>,
    /// The entry of the candidate being tried (0: none).
    cand_entry: u32,
    /// Pointer candidates a first pass found to block real code (see `discover`).
    deny: &'a HashSet<u32>,
    vt_cands: Vec<(u32, u32)>,
    seh_cands: Vec<(u32, u32, u32)>,
    data_refs: Vec<u32>,
    tried: HashSet<u32>,
    work: Vec<u32>,
    /// Addresses right after calls that do not return.
    after_noret: HashSet<u32>,
    /// Dead code after such calls, decoded but not a function: `(start, end)`.
    pub dead: Vec<(u32, u32)>,
    tx: Option<TxLog>,
    /// Gap candidates may not fall into a known function (they would be the tail of data
    /// placed right before it); referenced candidates may (a second entry point).
    strict: bool,
    /// The gap candidate being tried starts at a 16-byte boundary right after padding, where
    /// MSVC places functions: it may fall into a second entry of the next function (the CRT's
    /// unreferenced `strchr` runs into `__from_strstr_to_strchr`).
    aligned_gap: bool,
    /// Cross-jump entries from the previous round.
    joins: &'a BTreeSet<u32>,
    exec_lo: u32,
    exec_hi: u32,
}

/// Is `va` inside the executable sections?
fn exec_range(img: &Image) -> (u32, u32) {
    let lo = img
        .sections
        .iter()
        .filter(|s| s.exec)
        .map(|s| s.va)
        .min()
        .unwrap_or(0);
    let hi = img
        .sections
        .iter()
        .filter(|s| s.exec)
        .map(|s| s.va + s.size)
        .max()
        .unwrap_or(0);
    (lo, hi)
}

pub(crate) fn decode_at(img: &Image, va: u32) -> Option<Instruction> {
    let s = img.section_of(va)?;
    if !s.exec {
        return None;
    }
    let bytes = &s.data[(va - s.va) as usize..];
    let mut d = Decoder::with_ip(32, bytes, u64::from(va), DecoderOptions::NONE);
    let i = d.decode();
    (!i.is_invalid()).then_some(i)
}

/// Single-byte and multi-byte padding as MSVC emits it between and inside functions.
pub(crate) fn padding_len(b: &[u8]) -> usize {
    const SEQ: &[&[u8]] = &[
        &[0x8d, 0xa4, 0x24, 0, 0, 0, 0],
        &[0x8d, 0xb4, 0x26, 0, 0, 0, 0],
        &[0x8d, 0xbc, 0x27, 0, 0, 0, 0],
        &[0x8d, 0x9b, 0, 0, 0, 0],
        &[0x05, 0, 0, 0, 0],
        &[0x8d, 0xb6, 0, 0, 0, 0],
        &[0x8d, 0xbf, 0, 0, 0, 0],
        &[0x8d, 0x80, 0, 0, 0, 0],
        &[0x8d, 0x64, 0x24, 0],
        &[0x8d, 0x74, 0x26, 0],
        &[0x8d, 0x49, 0],
        &[0x8d, 0x40, 0],
        &[0x8d, 0x76, 0],
        &[0x8d, 0x7f, 0],
        &[0x8d, 0x1b],
        &[0x8b, 0xff],
        &[0x8b, 0xc0],
        &[0x8b, 0xc9],
        &[0x8b, 0xd2],
        &[0x8b, 0xdb],
        &[0x8b, 0xe4],
        &[0x8b, 0xed],
        &[0x8b, 0xf6],
        &[0x87, 0xdb],
    ];
    match b.first() {
        Some(0xcc | 0x90) => 1,
        Some(_) => SEQ.iter().find(|s| b.starts_with(s)).map_or(0, |s| s.len()),
        None => 0,
    }
}

fn falls_through(f: Flow) -> bool {
    match f {
        Flow::Next | Flow::Jcc(_) | Flow::Call(_) | Flow::CallInd => true,
        Flow::CallImp { noret, .. } => !noret,
        _ => false,
    }
}

/// Instructions that do not occur in MSVC 6 user-mode code: a candidate that decodes to one
/// of them is data.
fn suspicious(i: &Instruction) -> bool {
    if i.is_privileged() {
        return true;
    }
    if matches!(
        i.mnemonic(),
        Mnemonic::In
            | Mnemonic::Out
            | Mnemonic::Insb
            | Mnemonic::Insw
            | Mnemonic::Insd
            | Mnemonic::Outsb
            | Mnemonic::Outsw
            | Mnemonic::Outsd
            | Mnemonic::Arpl
            | Mnemonic::Bound
            | Mnemonic::Into
            | Mnemonic::Aaa
            | Mnemonic::Aas
            | Mnemonic::Daa
            | Mnemonic::Das
            | Mnemonic::Aam
            | Mnemonic::Aad
            | Mnemonic::Salc
            | Mnemonic::Lds
            | Mnemonic::Les
            | Mnemonic::Retf
            | Mnemonic::Iretd
            | Mnemonic::Iret
            | Mnemonic::Int1
            | Mnemonic::Hlt
    ) {
        return true;
    }
    if matches!(
        i.segment_prefix(),
        Register::ES | Register::CS | Register::SS | Register::DS | Register::GS
    ) {
        return true;
    }
    if i.has_xacquire_prefix() || i.has_xrelease_prefix() {
        return true;
    }
    if matches!(
        i.flow_control(),
        FlowControl::UnconditionalBranch | FlowControl::Call
    ) && matches!(i.op0_kind(), OpKind::FarBranch16 | OpKind::FarBranch32)
    {
        return true;
    }
    // `add [eax], al` (00 00): zero-filled data.
    if i.mnemonic() == Mnemonic::Add
        && i.len() == 2
        && i.op0_kind() == OpKind::Memory
        && i.memory_base() == Register::EAX
        && i.op1_register() == Register::AL
    {
        return true;
    }
    false
}

impl<'a> Engine<'a> {
    pub fn new(
        img: &'a Image,
        opts: &'a Options,
        no_return: HashSet<u32>,
        joins: &'a BTreeSet<u32>,
        deny: &'a HashSet<u32>,
    ) -> Self {
        let (exec_lo, exec_hi) = exec_range(img);
        Engine {
            img,
            opts,
            sp: Space::new(img),
            ins: HashMap::new(),
            funcs: BTreeMap::new(),
            labels: BTreeMap::new(),
            tables: Vec::new(),
            data: Vec::new(),
            vtables: BTreeMap::new(),
            scope_tables: Vec::new(),
            eh_funcinfos: Vec::new(),
            no_return,
            warnings: Vec::new(),
            rejected: Vec::new(),
            cands: Vec::new(),
            cand_entry: 0,
            deny,
            vt_cands: Vec::new(),
            seh_cands: Vec::new(),
            data_refs: Vec::new(),
            tried: HashSet::new(),
            work: Vec::new(),
            after_noret: HashSet::new(),
            dead: Vec::new(),
            tx: None,
            strict: false,
            aligned_gap: false,
            joins,
            exec_lo,
            exec_hi,
        }
    }

    pub fn is_exec(&self, va: u32) -> bool {
        va >= self.exec_lo && va < self.exec_hi && self.sp.idx(va).is_some()
    }

    /// Iterate the executable ranges as `(va, states)`.
    pub fn ranges(&self) -> impl Iterator<Item = (u32, &[u8])> {
        self.sp
            .ranges
            .iter()
            .map(|&(va, size, off)| (va, &self.sp.state[off..off + size as usize]))
    }

    fn set_state(&mut self, va: u32, s: u8) {
        if let Some(i) = self.sp.idx(va) {
            if let Some(tx) = &mut self.tx {
                tx.states.push((i, self.sp.state[i]));
            }
            self.sp.state[i] = s;
        }
    }

    fn warn(&mut self, va: u32, msg: impl Into<String>) {
        self.warnings.push((va, msg.into()));
    }

    pub fn add_func(&mut self, va: u32, src: Source) {
        let old = self.funcs.get(&va).copied();
        if let Some(tx) = &mut self.tx {
            tx.funcs.push((va, old));
        }
        self.funcs.insert(va, old.unwrap_or(0) | src.bit());
        if old.is_none() {
            self.work.push(va);
        }
    }

    fn add_label(&mut self, va: u32, src: Source) {
        let old = self.labels.get(&va).copied();
        if let Some(tx) = &mut self.tx {
            tx.labels.push((va, old));
        }
        self.labels.insert(va, old.unwrap_or(0) | src.bit());
    }

    fn begin(&mut self) {
        self.tx = Some(TxLog {
            tables: self.tables.len(),
            data: self.data.len(),
            cands: self.cands.len(),
            vt: self.vt_cands.len(),
            seh: self.seh_cands.len(),
            refs: self.data_refs.len(),
            warnings: self.warnings.len(),
            ..TxLog::default()
        });
    }

    fn commit(&mut self) {
        self.tx = None;
    }

    fn rollback(&mut self) {
        let Some(tx) = self.tx.take() else { return };
        for &(i, old) in tx.states.iter().rev() {
            self.sp.state[i] = old;
        }
        for va in tx.ins {
            self.ins.remove(&va);
        }
        for &(va, old) in tx.funcs.iter().rev() {
            match old {
                Some(b) => self.funcs.insert(va, b),
                None => self.funcs.remove(&va),
            };
        }
        for &(va, old) in tx.labels.iter().rev() {
            match old {
                Some(b) => self.labels.insert(va, b),
                None => self.labels.remove(&va),
            };
        }
        self.tables.truncate(tx.tables);
        self.data.truncate(tx.data);
        self.cands.truncate(tx.cands);
        self.vt_cands.truncate(tx.vt);
        self.seh_cands.truncate(tx.seh);
        self.data_refs.truncate(tx.refs);
        self.warnings.truncate(tx.warnings);
        self.work.clear();
    }

    /// A problem on a path: fatal for a candidate, a warning for trusted code.
    fn conflict(&mut self, va: u32, msg: &str, trusted: bool) -> Result<(), String> {
        if trusted {
            self.warn(va, msg);
            Ok(())
        } else {
            Err(format!("{msg} at {va:#x}"))
        }
    }

    fn drain(&mut self, trusted: bool) -> Result<(), String> {
        while let Some(va) = self.work.pop() {
            self.run(va, trusted)?;
        }
        Ok(())
    }

    fn mark_data(
        &mut self,
        start: u32,
        end: u32,
        kind: DataKind,
        trusted: bool,
    ) -> Result<(), String> {
        let mut va = start;
        while va < end {
            match self.sp.get(va) {
                Some(UNK) | Some(PAD) => self.set_state(va, DATA),
                Some(DATA) | None => {}
                Some(_) => {
                    self.conflict(va, "data overlaps code", trusted)?;
                    return Ok(());
                }
            }
            va += 1;
        }
        if self.sp.get(start).is_some() {
            self.data.push((start, end, kind));
        }
        Ok(())
    }

    /// Decode one linear run of instructions starting at `start`.
    fn run(&mut self, start: u32, trusted: bool) -> Result<(), String> {
        let mut va = start;
        let mut first = true;
        let mut msc = false;
        let mut after_call = false;
        loop {
            match self.sp.get(va) {
                None => return self.conflict(va, "path leaves the code sections", trusted),
                Some(INSN) => {
                    // A second entry point is fine, except for a gap: after a call the callee
                    // probably does not return, otherwise the gap is data that sits right
                    // before a function. Cross-jump entries are the middle of a function by
                    // construction. (TropicoRecomp also rejects candidates that run or call
                    // into the middle of known code; measured on the C1 DLLs without their
                    // relocations, that rule removed no junk and lost real functions.)
                    if !trusted
                        && self.strict
                        && !self.aligned_gap
                        && !first
                        && !after_call
                        && self
                            .funcs
                            .get(&va)
                            .is_some_and(|&b| b != Source::CrossJump.bit())
                    {
                        return Err(format!("falls into the function at {va:#x}"));
                    }
                    return Ok(());
                }
                Some(UNK) => {}
                Some(DATA) => return self.conflict(va, "path runs into data", trusted),
                Some(PAD) => return self.conflict(va, "path runs into padding", trusted),
                Some(_) => return self.conflict(va, "path enters an instruction", trusted),
            }
            let Some(insn) = decode_at(self.img, va) else {
                return self.conflict(va, "invalid instruction", trusted);
            };
            if !trusted && (suspicious(&insn) || (first && insn.mnemonic() == Mnemonic::Nop)) {
                return Err(format!("implausible instruction at {va:#x}"));
            }
            let len = insn.len() as u32;
            for k in 1..len {
                if self.sp.get(va + k) != Some(UNK) {
                    return self.conflict(va, "instruction overlaps known bytes", trusted);
                }
            }
            // Padding (int3) is no function entry; a branch to an int3 inside a function is
            // an assertion (`jcc` over or onto a breakpoint) and fine.
            if !trusted && first && va == self.cand_entry && insn.mnemonic() == Mnemonic::Int3 {
                return Err(format!("starts with int3 at {va:#x}"));
            }
            self.set_state(va, INSN);
            for k in 1..len {
                self.set_state(va + k, BODY);
            }
            self.refs(&insn, va, &mut msc);
            let flow = self.classify(&insn, va, msc, trusted)?;
            self.ins.insert(
                va,
                Ins {
                    len: len as u8,
                    flow,
                },
            );
            if let Some(tx) = &mut self.tx {
                tx.ins.push(va);
            }
            let noret_call = match flow {
                Flow::Call(t) => self.no_return.contains(&t),
                Flow::CallImp { noret, .. } => noret,
                _ => false,
            };
            if noret_call {
                // The compiler still emits the stack clean-up after such a call.
                self.after_noret.insert(va + len);
            }
            if !falls_through(flow) || noret_call {
                return Ok(());
            }
            after_call = matches!(flow, Flow::Call(_) | Flow::CallImp { .. } | Flow::CallInd);
            va += len;
            first = false;
        }
    }

    /// Collect pointer-like operands of an instruction.
    fn refs(&mut self, i: &Instruction, va: u32, msc: &mut bool) {
        for op in 0..i.op_count() {
            match i.op_kind(op) {
                OpKind::Immediate32 => {
                    let v = i.immediate32();
                    if self.is_exec(v) {
                        self.cands.push((v, Source::CodePointer, va));
                    } else if self.img.contains(v) {
                        if self.img.dword(v) == Some(MSC_EXCEPTION) {
                            *msc = true;
                        }
                        if i.mnemonic() == Mnemonic::Mov && i.op0_kind() == OpKind::Memory {
                            self.vt_cands.push((v, va));
                        }
                    }
                    if i.mnemonic() == Mnemonic::Push && v == MSC_EXCEPTION {
                        *msc = true;
                    }
                }
                OpKind::Memory => {
                    let d = i.memory_displacement32();
                    if i.memory_base() == Register::None && self.is_exec(d) {
                        if i.mnemonic() == Mnemonic::Lea {
                            self.cands.push((d, Source::CodePointer, va));
                        } else if i.flow_control() != FlowControl::IndirectBranch {
                            self.data_refs.push(d);
                        }
                    }
                }
                _ => {}
            }
        }
        // SEH3 frame: push -1; push scopetable; push handler; mov eax, fs:[0]
        if i.mnemonic() == Mnemonic::Push
            && i.op0_kind() == OpKind::Immediate8to32
            && i.immediate8to32() == -1
        {
            let b = |o: u32| self.img.byte(va + o);
            let pat = [
                (2, 0x68),
                (7, 0x68),
                (12, 0x64),
                (13, 0xa1),
                (14, 0),
                (15, 0),
                (16, 0),
                (17, 0),
            ];
            if pat.iter().all(|&(o, v)| b(o) == Some(v)) {
                if let (Some(st), Some(h)) = (self.img.dword(va + 3), self.img.dword(va + 8)) {
                    if self.img.contains(st) && !self.is_exec(st) && self.is_exec(h) {
                        self.seh_cands.push((va, st, h));
                    }
                }
            }
        }
    }

    fn push_target(&mut self, t: u32, va: u32, trusted: bool) -> Result<(), String> {
        if self.is_exec(t) {
            self.work.push(t);
            Ok(())
        } else {
            self.conflict(va, &format!("branch to {t:#x} outside the code"), trusted)
        }
    }

    fn import_at(&self, i: &Instruction) -> Option<u32> {
        (i.op0_kind() == OpKind::Memory
            && i.memory_base() == Register::None
            && i.memory_index() == Register::None)
            .then(|| i.memory_displacement32())
            .filter(|d| self.img.imports.contains_key(d))
    }

    pub fn import_noret(&self, slot: u32) -> bool {
        self.img
            .imports
            .get(&slot)
            .is_some_and(|n| is_noreturn_import(n))
    }

    fn classify(
        &mut self,
        i: &Instruction,
        va: u32,
        msc: bool,
        trusted: bool,
    ) -> Result<Flow, String> {
        Ok(match i.flow_control() {
            FlowControl::Next => Flow::Next,
            FlowControl::ConditionalBranch => {
                let t = i.near_branch_target() as u32;
                self.push_target(t, va, trusted)?;
                Flow::Jcc(t)
            }
            FlowControl::UnconditionalBranch => {
                if i.op0_kind() == OpKind::NearBranch32 || i.op0_kind() == OpKind::NearBranch16 {
                    let t = i.near_branch_target() as u32;
                    self.push_target(t, va, trusted)?;
                    Flow::Jmp(t)
                } else {
                    Flow::Stop
                }
            }
            FlowControl::Call => {
                let t = i.near_branch_target() as u32;
                if self.is_exec(t) {
                    self.add_func(t, Source::Call);
                } else {
                    self.conflict(va, &format!("call to {t:#x} outside the code"), trusted)?;
                }
                Flow::Call(t)
            }
            FlowControl::IndirectCall => match self.import_at(i) {
                Some(slot) => {
                    let name = &self.img.imports[&slot];
                    let noret = self.import_noret(slot)
                        || (msc && name.eq_ignore_ascii_case("kernel32.dll!RaiseException"));
                    Flow::CallImp { slot, noret }
                }
                None => Flow::CallInd,
            },
            FlowControl::IndirectBranch => {
                if let Some(slot) = self.import_at(i) {
                    Flow::JmpImp(slot)
                } else if i.op0_kind() == OpKind::Memory
                    && i.memory_index() != Register::None
                    && i.memory_index_scale() == 4
                    && i.memory_base() == Register::None
                {
                    self.jump_table(i, va, trusted)?
                } else {
                    Flow::JmpInd
                }
            }
            FlowControl::Return => Flow::Ret,
            _ => Flow::Stop,
        })
    }

    /// The instruction that falls through into `va`, if any.
    fn prev(&self, va: u32) -> Option<u32> {
        (1..=15u32).find_map(|back| {
            let p = va.checked_sub(back)?;
            self.ins
                .get(&p)
                .filter(|i| u32::from(i.len) == back && falls_through(i.flow))
                .map(|_| p)
        })
    }

    /// Find the index bound (and an index byte table) of `jmp [idx*4 + table]` at `va`.
    fn table_bound(&self, va: u32, idx: Register) -> (Option<(u32, Bound)>, Option<u32>) {
        let mut track = idx.full_register32();
        // Once the index came from a memory slot (`mov eax, [esp+4]`): that slot, as
        // (base, index, scale, displacement), which a `cmp [slot], n` then bounds.
        let mut slot: Option<(Register, Register, u32, u64)> = None;
        let mem_of = |i: &Instruction| {
            (
                i.memory_base(),
                i.memory_index(),
                i.memory_index_scale(),
                i.memory_displacement64(),
            )
        };
        let mut index_table = None;
        // Instructions walked so far, nearest to the jump first (they follow the current one).
        let mut later: Vec<Instruction> = Vec::new();
        let mut cur = va;
        for _ in 0..12 {
            let Some(p) = self.prev(cur) else { break };
            let Some(i) = decode_at(self.img, p) else {
                break;
            };
            let reg0 = i.op0_register().full_register32();
            let byte_load = i.op1_kind() == OpKind::Memory
                && i.memory_size() == MemorySize::UInt8
                && (i.memory_base() == Register::None) != (i.memory_index() == Register::None)
                && (i.memory_index() == Register::None || i.memory_index_scale() == 1);
            match i.mnemonic() {
                Mnemonic::Movzx | Mnemonic::Mov
                    if index_table.is_none()
                        && byte_load
                        && i.op0_kind() == OpKind::Register
                        && reg0 == track =>
                {
                    let s = if i.memory_base() != Register::None {
                        i.memory_base()
                    } else {
                        i.memory_index()
                    };
                    index_table = Some(i.memory_displacement32());
                    track = s.full_register32();
                }
                Mnemonic::Cmp
                    if ((slot.is_none() && i.op0_kind() == OpKind::Register && reg0 == track)
                        || (slot.is_some()
                            && i.op0_kind() == OpKind::Memory
                            && i.memory_size() == MemorySize::UInt32
                            && Some(mem_of(&i)) == slot))
                        && (matches!(
                            i.op1_kind(),
                            OpKind::Immediate32 | OpKind::Immediate8to32
                        ) || (i.op1_kind() == OpKind::Register
                            && self
                                .reg_const(i.op1_register().full_register32(), p)
                                .is_some())) =>
                {
                    // `cmp reg, n`, or `cmp reg, reg2` with a constant in reg2 (`push n; pop reg2`).
                    let n = if i.op1_kind() == OpKind::Register {
                        self.reg_const(i.op1_register().full_register32(), p)
                            .unwrap_or(0)
                    } else {
                        i.immediate(1) as u32
                    };
                    // The first conditional branch after the compare, with no flag writes
                    // in between (x87 instructions write only C0-C3, which branches do not read).
                    let arith = RflagsBits::OF
                        | RflagsBits::SF
                        | RflagsBits::ZF
                        | RflagsBits::AF
                        | RflagsBits::CF
                        | RflagsBits::PF;
                    let mut cc = None;
                    for a in later.iter().rev() {
                        if a.flow_control() == FlowControl::ConditionalBranch {
                            cc = Some(a.condition_code());
                            break;
                        }
                        if a.rflags_modified() & arith != 0 {
                            break;
                        }
                    }
                    let bound = match cc {
                        Some(ConditionCode::a) => n.checked_add(1),
                        Some(ConditionCode::ae) => Some(n),
                        _ => None,
                    };
                    return (bound.map(|b| (b, Bound::Compare)), index_table);
                }
                Mnemonic::And
                    if i.op0_kind() == OpKind::Register
                        && reg0 == track
                        && matches!(i.op1_kind(), OpKind::Immediate32 | OpKind::Immediate8to32) =>
                {
                    let m = i.immediate(1) as u32;
                    if m < 4096 {
                        return (Some((m + 1, Bound::Mask)), index_table);
                    }
                    break;
                }
                Mnemonic::Mov
                    if slot.is_none()
                        && i.op0_kind() == OpKind::Register
                        && i.op1_kind() == OpKind::Register
                        && reg0 == track =>
                {
                    track = i.op1_register().full_register32();
                }
                // The index loaded from a dword slot (a stack argument, a global): from here
                // on the slot is what a compare bounds. Only one such step.
                Mnemonic::Mov
                    if slot.is_none()
                        && i.op0_kind() == OpKind::Register
                        && i.op1_kind() == OpKind::Memory
                        && i.memory_size() == MemorySize::UInt32
                        && reg0 == track
                        && i.memory_index() == Register::None =>
                {
                    slot = Some(mem_of(&i));
                    track = Register::None;
                }
                _ => {
                    // Another write to the tracked register ends the search.
                    if i.op0_kind() == OpKind::Register
                        && reg0 == track
                        && !matches!(i.mnemonic(), Mnemonic::Cmp | Mnemonic::Test)
                    {
                        break;
                    }
                    if let Some(sl) = slot {
                        // A stack slot moves with esp; a store into the slot changes it.
                        let moves_esp = matches!(
                            i.mnemonic(),
                            Mnemonic::Push | Mnemonic::Pop | Mnemonic::Call
                        ) || (i.op0_kind() == OpKind::Register
                            && reg0 == Register::ESP);
                        let stores = i.op0_kind() == OpKind::Memory
                            && mem_of(&i) == sl
                            && !matches!(i.mnemonic(), Mnemonic::Cmp | Mnemonic::Test);
                        if (sl.0 == Register::ESP && moves_esp) || stores {
                            break;
                        }
                    }
                }
            }
            later.push(i);
            cur = p;
        }
        (None, index_table)
    }

    /// The constant a 32-bit register holds before the instruction at `at`: set by `mov reg, n`
    /// or by `push n; pop reg` shortly before, with no other write to it in between.
    fn reg_const(&self, reg: Register, at: u32) -> Option<u32> {
        let mut cur = at;
        for _ in 0..8 {
            let p = self.prev(cur)?;
            let i = decode_at(self.img, p)?;
            let writes =
                i.op0_kind() == OpKind::Register && i.op0_register().full_register32() == reg;
            match i.mnemonic() {
                Mnemonic::Mov
                    if writes
                        && matches!(i.op1_kind(), OpKind::Immediate32 | OpKind::Immediate8to32) =>
                {
                    return Some(i.immediate(1) as u32);
                }
                Mnemonic::Pop if writes => {
                    let q = self.prev(p)?;
                    let push = decode_at(self.img, q)?;
                    return (push.mnemonic() == Mnemonic::Push
                        && matches!(
                            push.op0_kind(),
                            OpKind::Immediate8to32 | OpKind::Immediate32
                        ))
                    .then(|| push.immediate(0) as u32);
                }
                Mnemonic::Cmp | Mnemonic::Test => {}
                _ if writes || i.flow_control() != FlowControl::Next => return None,
                _ => {}
            }
            cur = p;
        }
        None
    }

    fn jump_table(&mut self, i: &Instruction, va: u32, trusted: bool) -> Result<Flow, String> {
        let disp = i.memory_displacement32();
        let (mut bound, index_table) = self.table_bound(va, i.memory_index());
        let mut table = disp;
        if let Some(&(_, t, n)) = self.opts.jump_tables.iter().find(|h| h.0 == va) {
            bound = Some((n, Bound::Hint));
            table = t;
        }
        let mut idx_range = None;
        let (count, kind) = match bound {
            // (A hint's count bounds the index table when there is one.)
            Some((n, k)) if n > 0 && n <= 4096 => match index_table {
                Some(bt) => {
                    let mut max = 0u32;
                    for j in 0..n {
                        let Some(b) = self.img.byte(bt.wrapping_add(j)) else {
                            self.conflict(va, "index table outside the image", trusted)?;
                            return Ok(Flow::JmpInd);
                        };
                        max = max.max(u32::from(b));
                    }
                    idx_range = Some((bt, n));
                    (max + 1, k)
                }
                _ => (n, k),
            },
            _ if index_table.is_some() => {
                self.conflict(va, "index table without a bound", trusted)?;
                return Ok(Flow::JmpInd);
            }
            _ => {
                // No bound: the table is read once everything else is known (`resolve_scans`),
                // so that the scan stops at neighbouring code instead of guessing.
                let id = self.tables.len() as u32;
                self.tables.push(Table {
                    at: va,
                    table,
                    count: 0,
                    index_table: None,
                    bound: Bound::Scan,
                    targets: Vec::new(),
                    disp,
                    pending: true,
                });
                return Ok(Flow::Table(id));
            }
        };
        let id = self.tables.len() as u32;
        self.tables.push(Table {
            at: va,
            table,
            count,
            index_table: idx_range,
            bound: kind,
            targets: Vec::new(),
            disp,
            pending: false,
        });
        self.fill_table(id, trusted)?;
        Ok(Flow::Table(id))
    }

    /// Read the entries of a table whose location and count are known, mark them as data and
    /// queue the targets.
    fn fill_table(&mut self, id: u32, trusted: bool) -> Result<(), String> {
        let t = &self.tables[id as usize];
        let (va, table, count, kind, idx_range) = (t.at, t.table, t.count, t.bound, t.index_table);
        let mut targets = Vec::with_capacity(count as usize);
        let mut first = None;
        for k in 0..count {
            let loc = table.wrapping_add(4 * k);
            let Some(t) = self.img.dword(loc) else {
                self.conflict(va, "jump table outside the image", trusted)?;
                break;
            };
            let in_code = (0..4).any(|b| matches!(self.sp.get(loc + b), Some(INSN | BODY)));
            if in_code || !self.is_exec(t) {
                if first.is_none() {
                    // Entries the index cannot reach (a switch indexed from 1, MSVC's
                    // memcpy with `[idx*4 + table - 4]`) belong to whatever is there.
                    continue;
                }
                if kind != Bound::Mask {
                    self.conflict(va, &format!("jump table entry {k} is not code"), trusted)?;
                }
                break;
            }
            first.get_or_insert(k);
            targets.push(t);
            self.mark_data(loc, loc + 4, DataKind::JumpTable, trusted)?;
        }
        let first = first.unwrap_or(0);
        let tb = &mut self.tables[id as usize];
        tb.table = table.wrapping_add(4 * first);
        tb.count = targets.len() as u32;
        if let Some((bt, n)) = idx_range {
            self.mark_data(bt, bt + n, DataKind::IndexTable, trusted)?;
        }
        for &t in &targets {
            self.push_target(t, va, trusted)?;
        }
        self.tables[id as usize].targets = targets;
        Ok(())
    }

    /// Resolve the tables without a bound: read entries while they point into code and do
    /// not overlap known code. `neg idx` right before the jump, or no entry at the table
    /// address, means negative indices (MSVC's memcpy/memmove), so the entries lie below.
    fn resolve_scans(&mut self) -> bool {
        let pending: Vec<u32> = (0..self.tables.len() as u32)
            .filter(|&id| self.tables[id as usize].pending)
            .collect();
        if pending.is_empty() {
            return false;
        }
        for id in pending {
            let (va, disp) = (self.tables[id as usize].at, self.tables[id as usize].disp);
            self.tables[id as usize].pending = false;
            let Some(i) = decode_at(self.img, va) else {
                continue;
            };
            let negated = self
                .prev(va)
                .and_then(|p| decode_at(self.img, p))
                .is_some_and(|p| {
                    p.mnemonic() == Mnemonic::Neg
                        && p.op0_kind() == OpKind::Register
                        && p.op0_register().full_register32() == i.memory_index().full_register32()
                });
            let mut table = disp;
            let (mut n, k) = self.scan_table(table);
            if negated || n == 0 {
                let below = self.scan_table_down(table);
                if below > 0 {
                    table = table.wrapping_sub(4 * below);
                    n = below + u32::from(negated && n > 0);
                }
            }
            if n == 0 {
                self.warn(va, "jump table without valid entries");
                continue;
            }
            let t = &mut self.tables[id as usize];
            t.table = table;
            t.count = n;
            t.bound = k;
            let _ = self.fill_table(id, true);
        }
        let _ = self.drain(true);
        true
    }

    /// Read table entries while they point into code and do not run into known code.
    fn scan_table(&self, table: u32) -> (u32, Bound) {
        let reloc = self.opts.use_relocs && !self.img.relocs.is_empty();
        let mut n = 0u32;
        while n < 1024 {
            let at = table.wrapping_add(4 * n);
            if reloc && !self.img.relocs.contains(&at) {
                break;
            }
            if matches!(self.sp.get(at), Some(INSN | BODY)) {
                break;
            }
            match self.img.dword(at) {
                Some(t) if self.is_exec(t) && !matches!(self.sp.get(t), Some(BODY | DATA)) => {
                    n += 1
                }
                _ => break,
            }
        }
        (n, if reloc { Bound::Reloc } else { Bound::Scan })
    }

    /// Count the code pointers directly below `table` (for negative indices).
    fn scan_table_down(&self, table: u32) -> u32 {
        let reloc = self.opts.use_relocs && !self.img.relocs.is_empty();
        let mut n = 0u32;
        while n < 64 {
            let at = table.wrapping_sub(4 * (n + 1));
            if reloc && !self.img.relocs.contains(&at) {
                break;
            }
            if (0..4).any(|b| matches!(self.sp.get(at + b), Some(INSN | BODY))) {
                break;
            }
            match self.img.dword(at) {
                Some(t) if self.is_exec(t) && !matches!(self.sp.get(t), Some(BODY | DATA)) => {
                    n += 1
                }
                _ => break,
            }
        }
        n
    }

    /// Try one candidate code address. Returns true if it added code.
    fn try_candidate(&mut self, t: u32, src: Source, from: u32) -> bool {
        let _ = from;
        match self.sp.get(t) {
            Some(INSN) => {
                if self.funcs.contains_key(&t) {
                    let bits = self.funcs[&t] | src.bit();
                    self.funcs.insert(t, bits);
                } else {
                    self.add_label(t, src);
                }
                false
            }
            Some(UNK)
                if self.deny.contains(&t)
                    && matches!(src, Source::DataPointer | Source::CodePointer)
                    && !self.tried.contains(&t) =>
            {
                self.tried.insert(t);
                self.rejected.push((
                    t,
                    src,
                    "pointer that blocked real code in a first pass".into(),
                ));
                false
            }
            Some(UNK) if !self.tried.contains(&t) => {
                self.tried.insert(t);
                self.begin();
                self.add_func(t, src);
                self.cand_entry = t;
                self.strict = src == Source::Gap;
                self.aligned_gap = self.strict && self.after_padding(t);
                let r = self.drain(false);
                self.strict = false;
                self.aligned_gap = false;
                self.cand_entry = 0;
                match r {
                    Ok(()) => {
                        self.commit();
                        true
                    }
                    Err(why) => {
                        self.rollback();
                        self.rejected.push((t, src, why));
                        false
                    }
                }
            }
            _ => false,
        }
    }

    /// Functions found only through a pointer (data or a code immediate) whose bytes made other
    /// candidates fail ("… at X" in a rejection, X inside them): their entry is likely a false
    /// pointer into the middle of real code.
    pub fn blocking_pointers(&self) -> HashSet<u32> {
        let weak = Source::DataPointer.bit() | Source::CodePointer.bit();
        let mut out = HashSet::new();
        for (_, src, why) in &self.rejected {
            if !(why.contains("overlaps known bytes") || why.contains("enters an instruction")) {
                continue;
            }
            // A gap candidate is a guess at unreferenced code; that it fails against a pointer's
            // code is no evidence against the pointer (Heroes IV: alignment fill decoded across
            // the start of a CRT initialiser that only its table points to).
            if *src == Source::Gap {
                continue;
            }
            let Some(x) = why
                .rsplit("at 0x")
                .next()
                .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
            else {
                continue;
            };
            // `x` is where the failing instruction starts. A pointer-only function that begins
            // inside that instruction (within 15 bytes), or whose code covers `x`.
            for (&f, &bits) in self
                .funcs
                .range(x.saturating_sub(0x1000)..=x.saturating_add(15))
            {
                if bits & !weak != 0 {
                    continue;
                }
                if (f > x) || self.reaches(f, x) {
                    out.insert(f);
                }
            }
        }
        out
    }

    /// The code reached from `f` without leaving it includes the byte `x`.
    fn reaches(&self, f: u32, x: u32) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![f];
        while let Some(a) = stack.pop() {
            if !seen.insert(a) {
                continue;
            }
            let Some(ins) = self.ins.get(&a) else {
                continue;
            };
            if (a..a + u32::from(ins.len)).contains(&x) {
                return true;
            }
            let next = a + u32::from(ins.len);
            match ins.flow {
                Flow::Jcc(t) => {
                    stack.push(t);
                    stack.push(next);
                }
                Flow::Jmp(t) => stack.push(t),
                Flow::Next | Flow::Call(_) | Flow::CallInd | Flow::CallImp { .. } => {
                    stack.push(next)
                }
                _ => {}
            }
        }
        false
    }

    /// `va` is 16-byte aligned and follows padding or a `ret`.
    fn after_padding(&self, va: u32) -> bool {
        va.is_multiple_of(16)
            && (self.sp.get(va - 1) == Some(PAD)
                || matches!(self.img.byte(va - 1), Some(0xcc | 0x90 | 0xc3)))
    }

    fn trusted_roots(&mut self) {
        self.add_func(self.img.entry, Source::Entry);
        let exports: Vec<u32> = self.img.exports.iter().map(|e| e.0).collect();
        for va in exports {
            if self.is_exec(va) {
                self.add_func(va, Source::Export);
            }
        }
        for &(s, e) in &self.opts.data {
            let _ = self.mark_data(s, e, DataKind::Hint, true);
        }
        for &va in &self.opts.seeds {
            self.add_func(va, Source::Seed);
        }
        let joins: Vec<u32> = self.joins.iter().copied().collect();
        for va in joins {
            self.add_func(va, Source::CrossJump);
        }
        let _ = self.drain(true);
    }

    fn process_cands(&mut self) -> bool {
        let mut progress = false;
        while !self.cands.is_empty() {
            let batch = std::mem::take(&mut self.cands);
            for (t, src, from) in batch {
                progress |= self.try_candidate(t, src, from);
            }
        }
        progress
    }

    fn process_seh(&mut self) -> bool {
        let mut progress = false;
        while !self.seh_cands.is_empty() {
            let batch = std::mem::take(&mut self.seh_cands);
            for (frame, st, handler) in batch {
                if self
                    .scope_tables
                    .iter()
                    .any(|s| s.table == st && s.frame == frame)
                {
                    continue;
                }
                let mut n = 0u32;
                let mut roots = Vec::new();
                while n < 64 {
                    let e = st + 12 * n;
                    let (Some(prev), Some(filter), Some(h)) = (
                        self.img.dword(e),
                        self.img.dword(e + 4),
                        self.img.dword(e + 8),
                    ) else {
                        break;
                    };
                    let prev_ok = prev == u32::MAX || prev < n;
                    if !prev_ok || !(filter == 0 || self.is_exec(filter)) || !self.is_exec(h) {
                        break;
                    }
                    if filter != 0 {
                        roots.push((filter, Source::SehFilter));
                    }
                    roots.push((h, Source::SehHandler));
                    n += 1;
                }
                self.scope_tables.push(ScopeTable {
                    frame,
                    table: st,
                    handler,
                    entries: n,
                });
                for (t, s) in roots {
                    progress |= self.try_candidate(t, s, frame);
                }
            }
        }
        progress
    }

    /// C++ EH `FuncInfo` records (magic 0x19930520) in the data sections: unwind actions and
    /// catch handlers are code that only the CRT's frame handler calls.
    fn scan_eh(&mut self) -> bool {
        let mut roots = Vec::new();
        let img = self.img;
        for s in img.sections.iter().filter(|s| !s.exec) {
            for o in (0..s.data.len().saturating_sub(20)).step_by(4) {
                let fi = s.va + o as u32;
                if img.dword(fi) != Some(EH_MAGIC) || self.eh_funcinfos.contains(&fi) {
                    continue;
                }
                let d = |a: u32| img.dword(a).unwrap_or(0);
                let (max_state, unwind, ntry, tries) =
                    (d(fi + 4), d(fi + 8), d(fi + 12), d(fi + 16));
                if max_state > 4096 || ntry > 1024 {
                    continue;
                }
                if (max_state > 0 && !img.contains(unwind)) || (ntry > 0 && !img.contains(tries)) {
                    continue;
                }
                self.eh_funcinfos.push(fi);
                for k in 0..max_state {
                    let a = d(unwind + 8 * k + 4);
                    if self.is_exec(a) {
                        roots.push((a, Source::EhUnwind, fi));
                    }
                }
                for t in 0..ntry {
                    let e = tries + 20 * t;
                    let (nc, hs) = (d(e + 12).min(256), d(e + 16));
                    for c in 0..nc {
                        let a = d(hs + 16 * c + 12);
                        if self.is_exec(a) {
                            roots.push((a, Source::EhCatch, fi));
                        }
                    }
                }
            }
        }
        let mut progress = false;
        for (t, s, from) in roots {
            progress |= self.try_candidate(t, s, from);
        }
        progress
    }

    /// Code constants in `mov [obj+d], imm32` that point at runs of code pointers: vtables.
    fn process_vtables(&mut self) -> bool {
        if self.vt_cands.is_empty() {
            return false;
        }
        let mut all: BTreeMap<u32, u32> = std::mem::take(&mut self.vt_cands).into_iter().collect();
        for (&a, v) in &self.vtables {
            all.entry(a).or_insert(v.from);
        }
        let starts: Vec<u32> = all.keys().copied().collect();
        let mut progress = false;
        for (k, (&addr, &from)) in all.iter().enumerate() {
            if self.vtables.contains_key(&addr) {
                continue;
            }
            let limit = starts.get(k + 1).copied().unwrap_or(u32::MAX);
            let mut entries = Vec::new();
            let mut a = addr;
            while a < limit && entries.len() < 1024 {
                match self.img.dword(a) {
                    Some(t) if self.is_exec(t) => entries.push(t),
                    _ => break,
                }
                a += 4;
            }
            if entries.is_empty() {
                continue;
            }
            for &t in &entries {
                progress |= self.try_candidate(t, Source::Vtable, addr);
            }
            self.vtables.insert(addr, Vtable { entries, from });
        }
        progress
    }

    /// Aligned 32-bit constants in the data sections that point into code. DLLs: only the
    /// fixup sites, which are exact; their sites inside code sections are data in code.
    fn scan_data(&mut self) -> bool {
        let img = self.img;
        let mut found = Vec::new();
        if img.relocs.is_empty() || !self.opts.use_relocs {
            for s in img
                .sections
                .iter()
                .filter(|s| !s.exec && s.name != ".rsrc" && s.name != ".reloc")
            {
                for o in (0..s.data.len().saturating_sub(3)).step_by(4) {
                    let v = u32::from_le_bytes(s.data[o..o + 4].try_into().unwrap_or([0; 4]));
                    if self.is_exec(v) {
                        found.push((v, Source::DataPointer, s.va + o as u32));
                    }
                }
            }
        } else {
            let sites: Vec<u32> = img.relocs.iter().copied().collect();
            for site in sites {
                let Some(v) = img.dword(site) else { continue };
                if !self.is_exec(v) {
                    continue;
                }
                if self.is_exec(site) {
                    // Inside code, only a site that ends up outside every instruction is a
                    // stored pointer; that is decided at the end (`final_relocs`).
                    if self.sp.get(site) == Some(DATA) {
                        found.push((v, Source::Reloc, site));
                    }
                } else {
                    found.push((v, Source::Reloc, site));
                }
            }
        }
        let mut progress = false;
        for (t, s, from) in found {
            progress |= self.try_candidate(t, s, from);
        }
        progress
    }

    /// Unknown runs in the code sections: padding, then code that nothing references yet.
    fn fill_gaps(&mut self) -> bool {
        let gaps = self.unknown_runs();
        let mut progress = false;
        // Addresses used as data, or pointers whose target failed to decode as code.
        let mut refs: HashSet<u32> = self.data_refs.iter().copied().collect();
        refs.extend(
            self.rejected
                .iter()
                .filter(|r| r.1 != Source::Gap)
                .map(|r| r.0),
        );
        for (s, e) in gaps {
            if self.sp.get(s) != Some(UNK) {
                continue;
            }
            let mut p = s;
            while p < e {
                let n = self
                    .img
                    .tail(p)
                    .map_or(0, |t| padding_len(&t[..t.len().min((e - p) as usize)]));
                if n == 0 || p + n as u32 > e {
                    break;
                }
                p += n as u32;
            }
            // Zero fill up to the end of a section (virtual size rounded up beyond the raw data),
            // unless something points at it.
            let sec_end = self.img.section_of(p).map(|x| x.va + x.size);
            if sec_end == Some(e)
                && !refs.contains(&p)
                && self
                    .img
                    .tail(p)
                    .is_some_and(|t| t[..(e - p) as usize].iter().all(|&b| b == 0))
            {
                p = e;
            }
            for va in s..p {
                self.set_state(va, PAD);
            }
            if p >= e {
                continue;
            }
            if self.opts.gap_functions {
                // Candidate starts: the gap start, then 16-byte boundaries after padding or a ret.
                let mut starts = vec![p];
                let mut q = (p + 16) & !15;
                while q < e {
                    let prev = self.img.byte(q - 1);
                    if matches!(prev, Some(0xcc | 0x90 | 0xc3)) {
                        starts.push(q);
                    }
                    q += 16;
                }
                for q in starts {
                    if self.sp.get(q) != Some(UNK) || !self.try_candidate(q, Source::Gap, q) {
                        continue;
                    }
                    progress = true;
                    if self.after_noret.contains(&q)
                        && self.funcs.get(&q) == Some(&Source::Gap.bit())
                    {
                        // Unreachable clean-up code after a call that does not return.
                        self.funcs.remove(&q);
                        let mut end = q;
                        while end < e {
                            match self.ins.get(&end) {
                                Some(i) => end += u32::from(i.len),
                                None => break,
                            }
                        }
                        self.dead.push((q, end));
                    }
                }
            }
            if p == s && self.dead_tail(p, e, &refs) {
                continue;
            }
            if self.alignment_fill(p, e, &refs) {
                for va in p..e {
                    self.set_state(va, PAD);
                }
                continue;
            }
            if self.sp.get(p) == Some(UNK) {
                if let Some((n, pad)) = self.string_at(p, e) {
                    let _ = self.mark_data(p, p + n, DataKind::String, true);
                    for va in p + n..p + n + pad {
                        self.set_state(va, PAD);
                    }
                    continue;
                }
            }
            if self.sp.get(p) == Some(UNK) && refs.contains(&p) {
                let mut end = p;
                while end < e && self.sp.get(end) == Some(UNK) {
                    end += 1;
                }
                let _ = self.mark_data(p, end, DataKind::DataRef, true);
            }
        }
        progress
    }

    /// The stack clean-up after a call that does not return, squeezed in front of the next
    /// function (`call _CxxThrowException; pop esi` with an EH funclet right after it), which
    /// the gap pass cannot try as a function of its own. It must decode into whole instructions
    /// that end exactly where the code after the gap starts, and nothing may refer to it.
    fn dead_tail(&mut self, p: u32, e: u32, refs: &HashSet<u32>) -> bool {
        if !self.after_noret.contains(&p)
            || e - p >= 16
            || self.sp.get(e) != Some(INSN)
            || (p..e).any(|va| self.sp.get(va) != Some(UNK) || refs.contains(&va))
        {
            return false;
        }
        let mut q = p;
        let mut lens = Vec::new();
        while q < e {
            let Some(i) = decode_at(self.img, q) else {
                return false;
            };
            lens.push((q, i.len() as u32));
            q += i.len() as u32;
        }
        if q != e {
            return false;
        }
        for (va, n) in lens {
            self.set_state(va, INSN);
            for b in va + 1..va + n {
                self.set_state(b, BODY);
            }
        }
        self.dead.push((p, e));
        true
    }

    /// Alignment fill that is neither `int3` nor `nop`: some MSVC 6 builds (Heroes IV) pad the
    /// space between a function's last instruction and the next 16-byte aligned function with
    /// leftover bytes, often the tail of a nearby instruction (`ff 15 <iat> … c3`). Such a gap is
    /// shorter than 16 bytes, follows code, ends where a function or label starts at a 16-byte
    /// boundary, is
    /// still unknown after trying it as a function, and nothing refers to it.
    fn alignment_fill(&self, p: u32, e: u32, refs: &HashSet<u32>) -> bool {
        p > 0
            && e > p
            && e - p < 16
            && e.is_multiple_of(16)
            && matches!(self.sp.get(p - 1), Some(INSN | BODY))
            && self.sp.get(e) == Some(INSN)
            && (self.funcs.contains_key(&e) || self.labels.contains_key(&e))
            && (p..e).all(|va| self.sp.get(va) == Some(UNK) && !refs.contains(&va))
    }

    /// A run of at least 8 printable ASCII characters (CR, LF and tab included) that nothing
    /// decodes as code: copyright notices that some libraries keep in their code section, and
    /// the CRT's `VC20XC00` marker in front of `__except_handler3`. It ends at a NUL (the NULs
    /// after it belong to the string), at `int3` padding, right where known code starts, or at
    /// the end of the section.
    /// Returns the string's length and the length of the padding after it.
    fn string_at(&self, p: u32, e: u32) -> Option<(u32, u32)> {
        let t = self.img.tail(p)?;
        let t = &t[..t.len().min((e - p) as usize)];
        let printable = |b: u8| b == b'\r' || b == b'\n' || b == b'\t' || (0x20..0x7f).contains(&b);
        let n = t.iter().position(|&b| !printable(b)).unwrap_or(t.len());
        if n < 8 {
            return None;
        }
        if n == t.len() {
            // Up to the end of the gap: only if code or the end of the section follows.
            let sec_end = self.img.section_of(p).map(|x| x.va + x.size);
            return (self.sp.get(e) == Some(INSN) || sec_end == Some(e)).then_some((n as u32, 0));
        }
        match t[n] {
            0 => {
                let zeros = t[n..].iter().take_while(|&&b| b == 0).count();
                Some(((n + zeros) as u32, 0))
            }
            0xcc => {
                let pad = t[n..].iter().take_while(|&&b| b == 0xcc).count();
                Some((n as u32, pad as u32))
            }
            _ => None,
        }
    }

    pub fn unknown_runs(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for (va, st) in self.ranges() {
            let mut i = 0usize;
            while i < st.len() {
                if st[i] != UNK {
                    i += 1;
                    continue;
                }
                let j = st[i..]
                    .iter()
                    .position(|&b| b != UNK)
                    .map_or(st.len(), |k| i + k);
                out.push((va + i as u32, va + j as u32));
                i = j;
            }
        }
        out
    }

    /// Fixup sites in code that no instruction covers are pointers stored in code; their
    /// targets are candidates like any other pointer.
    fn final_relocs(&mut self) -> bool {
        if !self.opts.use_relocs {
            return false;
        }
        let img = self.img;
        let mut progress = false;
        let sites: Vec<u32> = img
            .relocs
            .iter()
            .copied()
            .filter(|&s| self.is_exec(s))
            .collect();
        for site in sites {
            if (0..4).all(|k| matches!(self.sp.get(site + k), Some(UNK | PAD))) {
                let _ = self.mark_data(site, site + 4, DataKind::Reloc, true);
                if let Some(v) = img.dword(site).filter(|&v| self.is_exec(v)) {
                    progress |= self.try_candidate(v, Source::Reloc, site);
                }
            }
        }
        progress
    }

    /// The whole discovery for one no-return set.
    pub fn run_all(&mut self) {
        self.trusted_roots();
        loop {
            let mut progress = self.process_cands();
            progress |= self.process_seh();
            progress |= self.process_vtables();
            if progress {
                continue;
            }
            progress |= self.scan_eh();
            progress |= self.scan_data();
            if progress || self.resolve_scans() {
                continue;
            }
            if self.fill_gaps()
                || !self.cands.is_empty()
                || !self.seh_cands.is_empty()
                || !self.vt_cands.is_empty()
            {
                continue;
            }
            if !self.final_relocs() {
                break;
            }
        }
    }

    /// The body of function `f`: what it reaches without entering another function (a
    /// successor that is another entry is a tail transfer, also by falling through).
    pub fn body(&self, f: u32, no_return: &HashSet<u32>) -> Body {
        let mut b = Body::default();
        b.blocks.insert(f);
        let mut seen = HashSet::new();
        let mut stack = vec![f];
        let go = |from: u32, to: u32, leader: bool, stack: &mut Vec<u32>, b: &mut Body| {
            if to != f && self.funcs.contains_key(&to) {
                b.tails.insert(to);
            } else {
                if leader {
                    b.blocks.insert(to);
                }
                b.edges.push((from, to));
                stack.push(to);
            }
        };
        while let Some(a) = stack.pop() {
            if !seen.insert(a) {
                continue;
            }
            let Some(ins) = self.ins.get(&a) else {
                b.exits = true; // unknown territory: assume it can return
                continue;
            };
            b.insns.insert(a);
            let next = a + u32::from(ins.len);
            match ins.flow {
                Flow::Next | Flow::CallInd => go(a, next, false, &mut stack, &mut b),
                Flow::Jcc(t) => {
                    go(a, t, true, &mut stack, &mut b);
                    go(a, next, true, &mut stack, &mut b);
                }
                Flow::Jmp(t) => go(a, t, true, &mut stack, &mut b),
                Flow::Call(t) => {
                    if !no_return.contains(&t) {
                        go(a, next, true, &mut stack, &mut b);
                    }
                }
                Flow::CallImp { noret, .. } => {
                    if !noret {
                        go(a, next, true, &mut stack, &mut b);
                    }
                }
                Flow::Table(id) => {
                    for &t in &self.tables[id as usize].targets {
                        go(a, t, true, &mut stack, &mut b);
                    }
                }
                Flow::JmpImp(slot) => b.exits |= !self.import_noret(slot),
                Flow::Ret | Flow::JmpInd => b.exits = true,
                Flow::Stop => {}
            }
        }
        b
    }

    /// Split the code into function bodies. A function returns if its body exits or it
    /// tail-transfers to a function that returns; the rest never return. Bodies are
    /// recomputed until that set is stable. Instructions in several bodies are shared; where
    /// unshared code enters shared code is a join, which becomes an entry next round.
    pub fn partition(&self) -> Partition {
        let mut nr: HashSet<u32> = self
            .no_return
            .iter()
            .copied()
            .filter(|f| self.funcs.contains_key(f))
            .collect();
        let bodies = loop {
            let bodies: BTreeMap<u32, Body> =
                self.funcs.keys().map(|&f| (f, self.body(f, &nr))).collect();
            let mut returns: HashSet<u32> = bodies
                .iter()
                .filter(|(f, b)| b.exits && !self.no_return.contains(f))
                .map(|(&f, _)| f)
                .collect();
            loop {
                let mut changed = false;
                for (&f, b) in &bodies {
                    if !returns.contains(&f)
                        && !self.no_return.contains(&f)
                        && b.tails.iter().any(|t| returns.contains(t))
                    {
                        returns.insert(f);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let grown: HashSet<u32> = bodies
                .keys()
                .copied()
                .filter(|f| !returns.contains(f))
                .collect();
            if grown.is_subset(&nr) {
                break bodies;
            }
            nr.extend(grown);
        };
        let mut owners: HashMap<u32, u32> = HashMap::new();
        for b in bodies.values() {
            for &a in &b.insns {
                *owners.entry(a).or_default() += 1;
            }
        }
        let shared = |a: &u32| owners.get(a).is_some_and(|&n| n > 1);
        let mut joins = BTreeSet::new();
        for b in bodies.values() {
            for (from, to) in &b.edges {
                if shared(to) && !shared(from) {
                    joins.insert(*to);
                }
            }
        }
        let mut shared_insns: Vec<u32> = owners
            .iter()
            .filter(|(_, &n)| n > 1)
            .map(|(&a, _)| a)
            .collect();
        shared_insns.sort_unstable();
        Partition {
            bodies,
            no_return: nr,
            joins,
            shared: shared_insns,
        }
    }
}

impl Engine<'_> {
    /// `(callee, call site)` pairs where the call falls straight into another function's
    /// entry. Usually the callee does not return (the CRT's `exit`, whose return path exists
    /// only for other callers of the shared `doexit`), but MSVC's x87 math routines have a
    /// second entry right after a returning call (`sin` falls into `_CIsin`), so this is only
    /// reported, for `no_return` hints in the manifest.
    pub fn no_return_evidence(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for (&va, ins) in &self.ins {
            if let Flow::Call(t) = ins.flow {
                let next = va + u32::from(ins.len);
                let strong = Source::Call.bit() | Source::Entry.bit() | Source::Export.bit();
                if t != next
                    && !self.opts.returns.contains(&t)
                    && self.funcs.contains_key(&t)
                    && !self.no_return.contains(&t)
                    && self.funcs.get(&next).is_some_and(|b| b & strong != 0)
                {
                    out.push((t, va));
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// Discovery checked against the base relocations of a DLL.
#[derive(Debug, Clone, Default)]
pub struct RelocCheck {
    /// Fixup sites inside code sections.
    pub sites: u32,
    /// Sites that are not inside an instruction operand or a data range.
    pub bad_sites: Vec<u32>,
    /// Fixed-up values that point into code.
    pub targets: u32,
    /// Targets that are not an instruction start or data.
    pub bad_targets: Vec<(u32, u32)>,
}

impl Engine<'_> {
    pub fn check_relocs(&self) -> Option<RelocCheck> {
        if self.img.relocs.is_empty() {
            return None;
        }
        let mut c = RelocCheck::default();
        // A table address in a `jmp [idx*4 + table]` may point below the real table, into
        // the jump instruction itself (negative or 1-based indices).
        let bases: HashSet<u32> = self
            .tables
            .iter()
            .filter_map(|t| decode_at(self.img, t.at))
            .map(|i| i.memory_displacement32())
            .collect();
        for &site in &self.img.relocs {
            if self.is_exec(site) {
                c.sites += 1;
                let st: Vec<Option<u8>> = (0..4).map(|k| self.sp.get(site + k)).collect();
                let in_operand = st.iter().all(|s| *s == Some(BODY));
                let in_data = st.iter().all(|s| *s == Some(DATA));
                if !in_operand && !in_data {
                    c.bad_sites.push(site);
                }
            }
            let Some(v) = self.img.dword(site) else {
                continue;
            };
            if self.is_exec(v) {
                c.targets += 1;
                if !matches!(self.sp.get(v), Some(INSN | DATA)) && !bases.contains(&v) {
                    c.bad_targets.push((site, v));
                }
            }
        }
        Some(c)
    }
}
