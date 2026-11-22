// SPDX-License-Identifier: MIT
//! Discovery on small hand-assembled images.

use crate::{discover, Image, Options, Section};
use std::collections::{BTreeMap, BTreeSet};

const TEXT: u32 = 0x40_1000;
const RDATA: u32 = 0x40_2000;
const EXIT_SLOT: u32 = RDATA;

fn image(code: &[u8], rdata: &[u8]) -> Image {
    let mut imports = BTreeMap::new();
    imports.insert(EXIT_SLOT, "kernel32.dll!ExitProcess".to_string());
    Image {
        base: 0x40_0000,
        entry: TEXT,
        sections: vec![
            Section {
                name: ".text".into(),
                va: TEXT,
                size: code.len() as u32,
                exec: true,
                data: code.to_vec(),
            },
            Section {
                name: ".rdata".into(),
                va: RDATA,
                size: rdata.len() as u32,
                exec: false,
                data: rdata.to_vec(),
            },
        ],
        imports,
        exports: Vec::new(),
        is_dll: false,
        relocs: BTreeSet::new(),
        preferred_base: 0x40_0000,
    }
}

/// Little assembler: bytes at fixed offsets, the rest `int3`.
fn code(size: usize, parts: &[(u32, &[u8])]) -> Vec<u8> {
    let mut c = vec![0xcc; size];
    for (va, b) in parts {
        let o = (va - TEXT) as usize;
        c[o..o + b.len()].copy_from_slice(b);
    }
    c
}

fn le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

fn sample() -> Image {
    let t = |a: u32| le(a);
    let table: Vec<u8> = [0x40_102c, 0x40_1032, 0x40_1038]
        .iter()
        .flat_map(|&a| t(a))
        .collect();
    let c = code(
        0x60,
        &[
            (0x40_1000, &[0xe8, 0x0b, 0, 0, 0]),             // call 0x401010
            (0x40_1005, &[0x6a, 0x00]),                      // push 0
            (0x40_1007, &[0xff, 0x15, 0x00, 0x20, 0x40, 0]), // call [ExitProcess]
            (0x40_1010, &[0x8b, 0x44, 0x24, 0x04]),          // mov eax, [esp+4]
            (0x40_1014, &[0x83, 0xf8, 0x02]),                // cmp eax, 2
            (0x40_1017, &[0x77, 0x22]),                      // ja 0x40103b
            (0x40_1019, &[0xff, 0x24, 0x85, 0x20, 0x10, 0x40, 0x00]), // jmp [eax*4+0x401020]
            (0x40_1020, &table),
            (0x40_102c, &[0xb8, 1, 0, 0, 0, 0xc3]), // case 0
            (0x40_1032, &[0xb8, 2, 0, 0, 0, 0xc3]), // case 1
            (0x40_1038, &[0x31, 0xc0, 0xc3]),       // case 2
            (0x40_103b, &[0x83, 0xc8, 0xff, 0xc3]), // default
            (0x40_1040, &[0x55, 0x8b, 0xec, 0x5d, 0xc3]), // only referenced from .rdata
            (0x40_1045, &[0x90; 11]),
            (0x40_1050, &[0x56, 0x8b, 0xf1, 0x5e, 0xc3]), // referenced by nothing
        ],
    );
    let mut rdata = vec![0u8; 0x20];
    rdata[0x10..0x14].copy_from_slice(&le(0x40_1040));
    image(&c, &rdata)
}

#[test]
fn descent_jump_table_pointers_and_gaps() {
    let m = discover(&sample(), &Options::default(), "t.exe", "00");
    let f = |va: u32| m.functions.iter().find(|f| f.entry.0 == va);
    let entry = f(TEXT).expect("entry function");
    assert!(entry.no_return, "ends in ExitProcess");
    assert!(entry.sources.contains(&"entry".to_string()));
    let sw = f(0x40_1010).expect("called function");
    assert!(sw.sources.contains(&"call".to_string()));
    assert_eq!(sw.chunks.len(), 2, "body is split by the jump table");
    assert!(f(0x40_1040)
        .unwrap()
        .sources
        .contains(&"data_pointer".to_string()));
    assert!(f(0x40_1050).unwrap().sources.contains(&"gap".to_string()));

    assert_eq!(m.jump_tables.len(), 1);
    let jt = &m.jump_tables[0];
    assert_eq!((jt.at.0, jt.table.0, jt.count), (0x40_1019, 0x40_1020, 3));
    assert_eq!(jt.bound, "compare");
    assert_eq!(
        jt.targets.iter().map(|v| v.0).collect::<Vec<_>>(),
        [0x40_102c, 0x40_1032, 0x40_1038]
    );
    assert!(m
        .data
        .iter()
        .any(|d| d.start.0 == 0x40_1020 && d.end.0 == 0x40_102c && d.kind == "jump_table"));

    assert_eq!(m.imports_called.get("kernel32.dll!ExitProcess"), Some(&1));
    assert!(m.unknown.is_empty(), "unknown: {:?}", m.unknown);
    assert!(m
        .padding
        .iter()
        .any(|p| p[0].0 == 0x40_1045 && p[1].0 == 0x40_1050));
    assert!((m.classified_percent() - 100.0).abs() < f64::EPSILON);
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
}

#[test]
fn no_return_is_propagated_through_wrappers() {
    // entry: call die; <garbage>      die: push 1; call [ExitProcess]
    let c = code(
        0x20,
        &[
            (0x40_1000, &[0xe8, 0x0b, 0, 0, 0]),
            (0x40_1005, &[0xff, 0xff, 0xff, 0xff]),
            (0x40_1010, &[0x6a, 0x01]),
            (0x40_1012, &[0xff, 0x15, 0x00, 0x20, 0x40, 0]),
        ],
    );
    let m = discover(&image(&c, &[0; 4]), &Options::default(), "t.exe", "00");
    assert_eq!(m.stats.iterations, 2);
    assert!(m.functions.iter().all(|f| f.no_return));
    assert!(
        m.warnings.is_empty(),
        "the garbage after the call is not decoded: {:?}",
        m.warnings
    );
    assert_eq!(m.functions[0].size, 5);
}

#[test]
fn pointers_to_data_are_not_code() {
    // entry: ret; then 16 bytes of zeros that a .rdata constant points to.
    let mut c = code(0x20, &[(0x40_1000, &[0xc3])]);
    c[0x10..0x20].fill(0);
    let mut rdata = vec![0u8; 8];
    rdata[4..8].copy_from_slice(&le(0x40_1010));
    let m = discover(&image(&c, &rdata), &Options::default(), "t.exe", "00");
    assert_eq!(m.functions.len(), 1);
    assert_eq!(m.rejected.len(), 1);
    assert!(m
        .data
        .iter()
        .any(|d| d.start.0 == 0x40_1010 && d.kind == "data_ref"));
}

#[test]
fn manifest_hints_seed_and_bound() {
    // entry: ret. A seeded function with an unbounded `jmp [ecx*4+table]` and a hint.
    let table: Vec<u8> = [0x40_1020u32, 0x40_1021]
        .iter()
        .flat_map(|&a| le(a))
        .collect();
    let c = code(
        0x30,
        &[
            (0x40_1000, &[0xc3]),
            (0x40_1010, &[0xff, 0x24, 0x8d, 0x18, 0x10, 0x40, 0x00]),
            (0x40_1018, &table),
            (0x40_1020, &[0xc3, 0xc3]),
        ],
    );
    let opts = Options {
        seeds: vec![0x40_1010],
        jump_tables: vec![(0x40_1010, 0x40_1018, 2)],
        ..Options::default()
    };
    let m = discover(&image(&c, &[0; 4]), &opts, "t.exe", "00");
    assert!(m
        .functions
        .iter()
        .any(|f| f.entry.0 == 0x40_1010 && f.sources == ["seed"]));
    assert_eq!(m.jump_tables[0].bound, "hint");
    assert_eq!(m.jump_tables[0].targets.len(), 2);
}

#[test]
fn padding_sequences() {
    use crate::engine::padding_len;
    assert_eq!(padding_len(&[0xcc, 0x90]), 1);
    assert_eq!(padding_len(&[0x8d, 0xa4, 0x24, 0, 0, 0, 0]), 7);
    assert_eq!(padding_len(&[0x8d, 0x49, 0x00]), 3);
    assert_eq!(padding_len(&[0x8b, 0xff]), 2);
    assert_eq!(padding_len(&[0x55]), 0);
}

#[test]
fn rebase_moves_fixups_and_addresses() {
    let mut img = sample();
    img.relocs.insert(0x40_1024); // the second jump table entry
    img.rebase(0x50_0000).unwrap();
    assert_eq!(img.entry, 0x50_1000);
    assert_eq!(img.sections[0].va, 0x50_1000);
    assert_eq!(img.dword(0x50_1024), Some(0x50_1032));
    assert!(img.imports.contains_key(&0x50_2000));
    assert!(img.relocs.contains(&0x50_1024));
    let mut fixed = sample();
    assert!(fixed.rebase(0x50_0000).is_err(), "no relocations");
}

#[test]
fn moved_code_has_the_same_fingerprint() {
    let a = sample();
    let mut b = sample();
    // The absolute addresses: the jump's table operand, the table entries, the IAT operand.
    for site in [0x40_101cu32, 0x40_1020, 0x40_1024, 0x40_1028, 0x40_1009] {
        b.relocs.insert(site);
    }
    b.rebase(0x50_0000).unwrap();
    let ma = discover(&a, &Options::default(), "a", "00");
    let mb = discover(&b, &Options::default(), "b", "00");
    let d = crate::diff(&crate::fingerprints(&a, &ma), &crate::fingerprints(&b, &mb));
    assert_eq!(d.identical, ma.functions.len());
    assert!(d.a_only.is_empty() && d.b_only.is_empty());
}

#[test]
fn strings_in_code_end_at_padding_or_code() {
    let c = code(
        0x40,
        &[
            (0x40_1000, &[0xe8, 0x0b, 0, 0, 0, 0xc3]), // call 0x401010; ret
            (0x40_1008, b"VC20XC00"),                  // CRT marker, code follows directly
            (0x40_1010, &[0x55, 0x8b, 0xec, 0x5d, 0xc3]), // push ebp; mov ebp, esp; pop ebp; ret
            (0x40_1015, b"\r\nCopyright (C) 1999 Example Tools\r\n"), // right after a ret; int3 follows
        ],
    );
    let m = discover(&image(&c, &[0; 8]), &Options::default(), "t.exe", "00");
    let string = |va: u32| m.data.iter().any(|d| d.start.0 == va && d.kind == "string");
    assert!(string(0x40_1008), "{:?}", m.data);
    assert!(string(0x40_1015), "{:?}", m.data);
    assert!(m.unknown.is_empty(), "{:?}", m.unknown);
}

#[test]
fn aligned_gap_code_may_fall_into_a_second_entry() {
    // The entry calls a second entry at 0x401030; the unreferenced code in front of it starts
    // at a 16-byte boundary after padding and runs into it, like the CRT's strchr.
    let c = code(
        0x40,
        &[
            (0x40_1000, &[0xe8, 0x2b, 0, 0, 0, 0xc3]), // call 0x401030; ret
            (0x40_1020, &[0x31, 0xc0]),                // xor eax, eax
            (0x40_1022, &[0x8b, 0x44, 0x24, 0x04]),    // mov eax, [esp+4]
            (0x40_1026, &[0x8b, 0x44, 0x24, 0x04]),
            (0x40_102a, &[0x8b, 0x44, 0x24, 0x04]),
            (0x40_102e, &[0x89, 0xc1]), // mov ecx, eax
            (0x40_1030, &[0xc3]),       // ret
        ],
    );
    let m = discover(&image(&c, &[0; 8]), &Options::default(), "t.exe", "00");
    assert!(m.functions.iter().any(|f| f.entry.0 == 0x40_1020));
    assert!(m.functions.iter().any(|f| f.entry.0 == 0x40_1030));
    assert!(m.unknown.is_empty(), "{:?}", m.unknown);
}

#[test]
fn returns_hint_settles_a_no_return_candidate() {
    // A calls a helper that returns, and the call is A's last instruction: B, another entry,
    // follows it (MSVC's x87 routines are laid out like this).
    let c = code(
        0x30,
        &[
            (0x40_1000, &[0xe8, 0x06, 0, 0, 0]), // call A (0x40100b)
            (0x40_1005, &[0xe8, 0x06, 0, 0, 0]), // call B (0x401010)
            (0x40_100a, &[0xc3]),
            (0x40_100b, &[0xe8, 0x10, 0, 0, 0]), // A: call helper (0x401020)
            (0x40_1010, &[0x31, 0xc0, 0xc3]),    // B: xor eax, eax; ret
            (0x40_1020, &[0xc3]),                // helper: ret
        ],
    );
    let img = image(&c, &[0; 8]);
    let m = discover(&img, &Options::default(), "t.exe", "00");
    assert!(!m.no_return_candidates.is_empty());
    let opts = Options {
        returns: m.no_return_candidates.iter().map(|c| c[0].0).collect(),
        ..Options::default()
    };
    let m = discover(&img, &opts, "t.exe", "00");
    assert!(m.no_return_candidates.is_empty());
}

#[test]
fn switch_bounded_through_a_stack_slot() {
    // cmp dword [esp+4], 1; ja default; mov eax, [esp+4]; jmp [eax*4+0x401030]
    let table: Vec<u8> = [0x40_1038u32, 0x40_103b]
        .iter()
        .flat_map(|&a| le(a))
        .collect();
    let c = code(
        0x50,
        &[
            (0x40_1000, &[0x83, 0x7c, 0x24, 0x04, 0x01]), // cmp dword [esp+4], 1
            (0x40_1005, &[0x77, 0x37]),                   // ja 0x40103e
            (0x40_1007, &[0x8b, 0x44, 0x24, 0x04]),       // mov eax, [esp+4]
            (0x40_100b, &[0xff, 0x24, 0x85, 0x30, 0x10, 0x40, 0x00]), // jmp [eax*4+0x401030]
            (0x40_1030, &table),
            (0x40_1038, &[0x31, 0xc0, 0xc3]),       // case 0
            (0x40_103b, &[0x40, 0xc3]),             // case 1: inc eax; ret
            (0x40_103e, &[0x83, 0xc8, 0xff, 0xc3]), // default
        ],
    );
    let m = discover(&image(&c, &[0; 8]), &Options::default(), "t.exe", "00");
    let t = m
        .jump_tables
        .iter()
        .find(|t| t.at.0 == 0x40_100b)
        .expect("table");
    assert_eq!(t.count, 2);
    assert_eq!(t.bound, "compare");
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
}

#[test]
fn switch_bound_across_an_x87_store() {
    // cmp eax, 2; fstp dword [esp+4]; ja default; jmp [eax*4+table]: x87 instructions write
    // only C0-C3, so the compare still bounds the table.
    let table: Vec<u8> = [0x40_1030u32, 0x40_1036, 0x40_103c]
        .iter()
        .flat_map(|&a| le(a))
        .collect();
    let c = code(
        0x50,
        &[
            (0x40_1000, &[0xe8, 0x0b, 0, 0, 0]),             // call 0x401010
            (0x40_1005, &[0x6a, 0x00]),                      // push 0
            (0x40_1007, &[0xff, 0x15, 0x00, 0x20, 0x40, 0]), // call [ExitProcess]
            (0x40_1010, &[0x8b, 0x44, 0x24, 0x04]),          // mov eax, [esp+4]
            (0x40_1014, &[0x83, 0xf8, 0x02]),                // cmp eax, 2
            (0x40_1017, &[0xd9, 0x5c, 0x24, 0x04]),          // fstp dword [esp+4]
            (0x40_101b, &[0x77, 0x22]),                      // ja 0x40103f
            (0x40_101d, &[0xff, 0x24, 0x85, 0x24, 0x10, 0x40, 0x00]), // jmp [eax*4+0x401024]
            (0x40_1024, &table),
            (0x40_1030, &[0xb8, 1, 0, 0, 0, 0xc3]), // case 0
            (0x40_1036, &[0xb8, 2, 0, 0, 0, 0xc3]), // case 1
            (0x40_103c, &[0x31, 0xc0, 0xc3]),       // case 2
            (0x40_103f, &[0x83, 0xc8, 0xff, 0xc3]), // default
        ],
    );
    let m = discover(&image(&c, &[0; 0x10]), &Options::default(), "t.exe", "00");
    assert_eq!(m.jump_tables.len(), 1);
    assert_eq!(m.jump_tables[0].count, 3);
    assert_eq!(m.jump_tables[0].bound, "compare");
    assert!(m.unknown.is_empty(), "unknown: {:?}", m.unknown);
}

#[test]
fn alignment_fill_of_leftover_bytes() {
    // After the entry's `ret`, bytes that are neither int3 nor nop fill up to the next
    // 16-byte aligned function; tried as code they run into it.
    let c = code(
        0x20,
        &[
            (0x40_1000, &[0xe8, 0x0b, 0, 0, 0]), // call 0x401010
            (0x40_1005, &[0xc3]),                // ret
            (
                0x40_1006,
                &[0x8b, 0x44, 0x24, 0x04, 0x8b, 0x4c, 0x24, 0x08, 0x8b, 0x54],
            ),
            (0x40_1010, &[0x56, 0x8b, 0xf1, 0x5e, 0xc3]),
        ],
    );
    let m = discover(&image(&c, &[0; 0x10]), &Options::default(), "t.exe", "00");
    assert!(m.unknown.is_empty(), "unknown: {:?}", m.unknown);
    assert!(m
        .padding
        .iter()
        .any(|p| p[0].0 == 0x40_1006 && p[1].0 == 0x40_1010));
}

#[test]
fn dead_tail_after_a_call_that_does_not_return() {
    // push esi; call noreturn; pop esi (dead), and a function right after the dead byte.
    let c = code(
        0x30,
        &[
            (0x40_1000, &[0x56]),                            // push esi
            (0x40_1001, &[0xe8, 0x1a, 0, 0, 0]),             // call 0x401020
            (0x40_1006, &[0x5e]),                            // pop esi (never runs)
            (0x40_1007, &[0x33, 0xc0, 0xc3]),                // only referenced from .rdata
            (0x40_1020, &[0x6a, 0x00]),                      // push 0
            (0x40_1022, &[0xff, 0x15, 0x00, 0x20, 0x40, 0]), // call [ExitProcess]
        ],
    );
    let mut rdata = vec![0u8; 0x20];
    rdata[0x10..0x14].copy_from_slice(&le(0x40_1007));
    let m = discover(&image(&c, &rdata), &Options::default(), "t.exe", "00");
    assert!(m.unknown.is_empty(), "unknown: {:?}", m.unknown);
    assert!(m
        .dead_code
        .iter()
        .any(|d| d[0].0 == 0x40_1006 && d[1].0 == 0x40_1007));
}

#[test]
fn a_failed_gap_guess_does_not_block_a_pointer() {
    // Fill after the entry's ret decodes across the start of a function that only .rdata points
    // to (a CRT initialiser): the pointer wins and the fill is padding.
    let c = code(
        0x20,
        &[
            (0x40_1000, &[0x33, 0xc0, 0xc3]),       // xor eax, eax; ret
            (0x40_1003, &[0x8b, 0x44, 0x24, 0x04]), // leftover bytes ...
            (0x40_1007, &[0xff, 0x15, 0x04, 0x20, 0x40, 0x00]),
            (0x40_100d, &[0x00, 0x83, 0xc4]), // ... running into the next function
            (0x40_1010, &[0x33, 0xc0, 0xc3]), // only referenced from .rdata
        ],
    );
    let mut rdata = vec![0u8; 0x20];
    rdata[0x10..0x14].copy_from_slice(&le(0x40_1010));
    let img = image(&c, &rdata);
    let opts = Options::default();
    let (joins, deny) = (BTreeSet::new(), std::collections::HashSet::new());
    let mut e = crate::engine::Engine::new(&img, &opts, Default::default(), &joins, &deny);
    e.run_all();
    assert!(
        !e.blocking_pointers().contains(&0x40_1010),
        "the gap guess's failure counted against the pointer"
    );
    let m = discover(&img, &opts, "t.exe", "00");
    let f = m
        .functions
        .iter()
        .find(|f| f.entry.0 == 0x40_1010)
        .expect("pointer function");
    assert!(f.sources.contains(&"data_pointer".to_string()));
    assert!(m.functions.iter().all(|f| f.entry.0 != 0x40_1003));
    assert!(m.unknown.is_empty(), "unknown: {:?}", m.unknown);
}
