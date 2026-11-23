// SPDX-License-Identifier: MIT
//! One shard of the lifted code; see `../shard-build.rs`.

#![allow(clippy::all)]

use rc_abi as _;

include!(concat!(env!("OUT_DIR"), "/shard.rs"));
