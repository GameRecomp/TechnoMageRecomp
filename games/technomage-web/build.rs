// SPDX-License-Identifier: MIT
//! The build information of the "Show information" page, as the native shell has it.

include!("../technomage/build_info.rs");

fn main() {
    build_info(&["src", "../technomage/src"]);
}
