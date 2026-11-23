// SPDX-License-Identifier: MIT
//! What lifted code is compiled against, and nothing else: the guest CPU state ([`cpu`]), guest
//! memory ([`mem`]), the x87 ([`x87`]) and MMX ([`mmx`]) operations, and the runtime's entry
//! points (dispatch, traps, unwinding), which `rc-runtime` defines under fixed names and this
//! crate only declares ([`lifted`]). Lifted code depends on this crate alone, so changes to the
//! runtime do not rebuild it.

pub mod cpu;
pub mod lifted;
pub mod mem;
pub mod mmx;
pub mod x87;

pub use cpu::Cpu;
