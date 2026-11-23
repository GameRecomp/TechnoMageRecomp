// SPDX-License-Identifier: MIT
//! d3d: Direct3D 7 on the platform's wgpu device (docs/PLAN.md, M5). Not there yet:
//! `IDirectDraw7::QueryInterface(IID_IDirect3D7)` reports it missing.

use rc_runtime::mem::Mem;

/// The `IDirect3D7` object for the DirectDraw object `ddraw`, once Direct3D exists.
pub fn create(m: Mem, ddraw: u32) -> Option<u32> {
    let _ = (m, ddraw);
    eprintln!("[d3d] IDirect3D7 requested: Direct3D 7 is not implemented yet (M5)");
    None
}
