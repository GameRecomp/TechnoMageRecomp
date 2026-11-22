// SPDX-License-Identifier: MIT
//! IR to Rust source, the developer back end.
//!
//! The sister projects emit C (`rc-emit-c`) and compile it with a C compiler. This project uses
//! no C: every guest function becomes a Rust function with the same contract as the other back
//! ends (`rc-emit-native`, `rc-emit-wasm`):
//!
//! ```text
//! pub fn technomage_exe_00401000(c: &mut Cpu) { ... }
//! ```
//!
//! Registers and the six arithmetic flags are locals, written back to `c` around every call,
//! return and indirect transfer. Guest memory is reached through `rc_runtime::mem` accessors
//! (bounds-free, the 4 GiB reservation makes every 32-bit address valid). Direct calls are
//! Rust calls, indirect ones go through `rc_runtime::dispatch`. Jumps inside a function become
//! a `loop { match block { ... } }`, the same block structure `rc-emit-wasm` uses.
//!
//! Output layout (`generated/technomage/<binary>-<variant>/`, git-ignored): `code_XXXX.rs` per
//! 64 KiB of guest address space, `mod.rs` with the sorted dispatch table and the binary record.
//! The output is a function of the input only (deterministic, no hash-map order).

#![forbid(unsafe_code)]
