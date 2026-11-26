// SPDX-License-Identifier: MIT
//! Android (tools/build-android): the app is a NativeActivity running this program as
//! `libtechnomage.so`. A converter package carries the game's code and files as assets under
//! `assets/data/` with an index, `files.txt` (a line per entry: its size and path, directories
//! ending in `/`). On the first start, and after an update brings a different index, they are
//! unpacked into the app's own storage, where the shell finds them as beside a desktop program.

use bevy::android::android_activity::AndroidApp;
use std::ffi::CString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub use bevy::android::android_activity;

/// Keep the app handle for Bevy (its winit backend takes it from here) and for us.
pub fn init(app: AndroidApp) {
    let _ = bevy::android::ANDROID_APP.set(app);
    log_to_file();
}

fn app() -> Option<&'static AndroidApp> {
    bevy::android::ANDROID_APP.get()
}

/// The app's private directory (`/data/data/<package>/files`).
pub fn internal_dir() -> Option<PathBuf> {
    app()?.internal_data_path()
}

/// stderr (where the runtime logs) into `log.txt` in the app's external files directory
/// (`Android/data/<package>/files`, which a computer sees over USB): Android drops it otherwise.
fn log_to_file() {
    unsafe extern "C" {
        fn dup2(old: i32, new: i32) -> i32;
    }
    use std::os::fd::IntoRawFd;
    let Some(dir) = app()
        .and_then(AndroidApp::external_data_path)
        .or_else(internal_dir)
    else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(f) = std::fs::File::create(dir.join("log.txt")) {
        let fd = f.into_raw_fd();
        // SAFETY: both descriptors are open; stderr now writes to the file (which stays open).
        unsafe {
            dup2(fd, 2);
        }
    }
}

/// An asset's whole content (`path` below `assets/`).
fn asset(path: &str) -> Option<ndk::asset::Asset> {
    let name = CString::new(path).ok()?;
    app()?.asset_manager().open(&name)
}

use bevy::android::android_activity::ndk;

/// The package's data directory (`data/` of a desktop package): unpacked from the assets if
/// they are newer than what was unpacked before. `None` without bundled data (a shell APK
/// without a game).
pub fn bundled_data() -> Option<PathBuf> {
    let mut index = String::new();
    asset("data/files.txt")?.read_to_string(&mut index).ok()?;
    let root = internal_dir()?.join("data");
    let stamp = root.join("files.txt");
    if std::fs::read_to_string(&stamp).ok().as_deref() == Some(index.as_str()) {
        return Some(root);
    }
    eprintln!("android: unpacking the game data to {}", root.display());
    // The index last, so an interrupted unpacking is done again on the next start.
    let _ = std::fs::remove_file(&stamp);
    let total: u64 = index
        .lines()
        .filter_map(|l| l.split_once(' ')?.0.parse::<u64>().ok())
        .sum();
    let mut done = 0u64;
    let mut shown = 0;
    for line in index.lines() {
        let Some((size, path)) = line.split_once(' ') else {
            continue;
        };
        if path.contains("..") {
            continue;
        }
        let dest = root.join(path);
        if path.ends_with('/') {
            if let Err(e) = std::fs::create_dir_all(&dest) {
                eprintln!("android: {}: {e}", dest.display());
                return None;
            }
            continue;
        }
        if let Err(e) = unpack(&format!("data/{path}"), &dest) {
            eprintln!("android: {path}: {e}");
            return None;
        }
        done += size.parse::<u64>().unwrap_or(0);
        let percent = (done * 100).checked_div(total).unwrap_or(100);
        if percent >= shown + 10 {
            shown = percent;
            eprintln!("android: {percent} % unpacked");
        }
    }
    std::fs::write(&stamp, &index).ok()?;
    Some(root)
}

/// One asset into a file, replaced whole (an earlier copy is truncated first).
fn unpack(asset_path: &str, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut a = asset(asset_path).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "not in the package's assets")
    })?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(dest)?);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = a.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
    }
    out.flush()
}
