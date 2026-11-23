// SPDX-License-Identifier: MIT
//! The runtime lifted code runs on: a port of the sister projects' C11 runtime to Rust.
//!
//! - [`mem`]: the 4 GiB guest address space (reserved on 64-bit hosts, a smaller checked
//!   region elsewhere).
//! - [`cpu`]: the guest CPU state shared by all back ends (`#[repr(C)]`, the layout
//!   `rc-emit-native` and `rc-emit-wasm` address); [`x87`] in fast mode, [`mmx`].
//! - [`dispatch`]: indirect calls and jumps (patches, the sorted tables of the lifted binaries,
//!   the reserved import addresses), traps that name the address, unwinding.
//! - [`handles`]: kernel objects (files, searches, synchronisation objects).
//! - [`imports`]: reserved addresses for imported functions, the HLE registry.
//! - [`process`]: the PE loader (packed executables restored first), stack, TIB and PEB.
//! - [`threads`]: cooperative guest threads (one host thread each, a baton).
//! - [`vfs`]: Windows paths onto the install, the virtual CD drive, the user directory.
//! - [`vm`]: page regions and heaps in guest memory.
//! - [`clock`]: real and virtual time.
//! - [`lifted`]: what generated code imports.
//!
//! Still to come (docs/PLAN.md, M3r): SEH.
//!
//! See docs/ARCHITECTURE.md.

pub mod clock;
pub mod cpu;
pub mod dispatch;
pub mod handles;
pub mod imports;
pub mod lifted;
pub mod mem;
pub mod mmx;
pub mod process;
pub mod threads;
pub mod vfs;
pub mod vm;
pub mod x87;

pub use cpu::Cpu;
