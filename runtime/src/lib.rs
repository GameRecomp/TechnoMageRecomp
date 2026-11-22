// SPDX-License-Identifier: MIT
//! The runtime lifted code runs on: a port of the sister projects' C11 runtime to Rust.
//!
//! - `mem`: the 4 GiB guest address space (reserved on 64-bit hosts, a region of linear
//!   memory in the browser) with a virtual-memory and heap layer.
//! - `cpu`: the guest CPU state shared by all back ends (`#[repr(C)]`, the layout
//!   `rc-emit-native` and `rc-emit-wasm` address), x87 in fast mode, `cpuid`, `rdtsc`.
//! - `dispatch`: indirect calls and jumps (patches, the sorted tables of the lifted binaries,
//!   the reserved import addresses), traps that name the address.
//! - `loader`: maps the restored executable and the lifted DLLs, binds imports to HLE.
//! - `seh`: structured exception handling and unwinding over the guest stack.
//! - `threads`: guest threads as cooperative fibers (the music and loading threads).
//! - `clock`: real and virtual time (scripted runs).
//! - `vfs`: the game directory, case-insensitive, `\` as separator, the CD's files.
//!
//! See docs/ARCHITECTURE.md.

/// Guest CPU state. The full layout (flags, x87, segment bases, `mem`, `helpers`) follows the
/// sister projects' `rc_cpu` so the Cranelift and WebAssembly back ends can be shared unchanged.
#[repr(C)]
#[derive(Debug, Default, Clone)]
pub struct Cpu {
    /// `eax`, `ecx`, `edx`, `ebx`, `esp`, `ebp`, `esi`, `edi`.
    pub r: [u32; 8],
}
