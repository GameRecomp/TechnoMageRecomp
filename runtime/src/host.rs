// SPDX-License-Identifier: MIT
//! What the runtime needs from the host system, in one place: files, the clocks, sleeping.
//!
//! Natively this is `std`. In a browser (`wasm32-unknown-unknown`) `std` has no files and no
//! clock, and the player's JavaScript provides them through the imports of module `rc_js`
//! (web/player.js): the game's files in the origin's private file system, `performance.now()`
//! and `Date.now()`. Those that wait (opening a file, sleeping) are JSPI `Suspending` imports,
//! so the guest thread waits while the browser goes on.
//!
//! Paths are host paths (`Path`); in a browser they are the player's (`/game/…`, `/user/…`).

use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};

/// What a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub len: u64,
    pub is_dir: bool,
    /// Last change, milliseconds since 1970 (0: unknown).
    pub modified_ms: u64,
}

/// How [`File::open`] opens.
#[derive(Debug, Clone, Copy, Default)]
pub struct Open {
    pub write: bool,
    pub create: bool,
    pub create_new: bool,
    pub truncate: bool,
}

#[cfg(not(target_family = "wasm"))]
mod imp {
    use super::{Meta, Open};
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::path::Path;
    use std::sync::OnceLock;
    use std::time::Instant;

    pub fn metadata(p: &Path) -> Option<Meta> {
        let m = std::fs::metadata(p).ok()?;
        let modified_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Some(Meta {
            len: m.len(),
            is_dir: m.is_dir(),
            modified_ms,
        })
    }

    pub fn read_dir(p: &Path) -> Option<Vec<String>> {
        Some(
            std::fs::read_dir(p)
                .ok()?
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
        )
    }

    pub fn create_dir_all(p: &Path) -> bool {
        std::fs::create_dir_all(p).is_ok()
    }

    pub fn remove_file(p: &Path) -> bool {
        std::fs::remove_file(p).is_ok()
    }

    pub fn copy(from: &Path, to: &Path) -> bool {
        std::fs::copy(from, to).is_ok()
    }

    #[derive(Debug)]
    pub struct File(std::fs::File);

    impl File {
        pub fn open(p: &Path, how: Open) -> io::Result<File> {
            let mut o = std::fs::OpenOptions::new();
            o.read(true)
                .write(how.write)
                .create(how.create)
                .create_new(how.create_new)
                .truncate(how.truncate);
            o.open(p).map(File)
        }
        pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
        pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            self.0.write_all(data)
        }
        pub fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.0.seek(to)
        }
        pub fn truncate_here(&mut self) -> io::Result<()> {
            let p = self.0.stream_position()?;
            self.0.set_len(p)
        }
        pub fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    static START: OnceLock<Instant> = OnceLock::new();

    pub fn monotonic_ns() -> u64 {
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    pub fn wall_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    pub fn env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    pub fn log(msg: &str) {
        eprintln!("{msg}");
    }

    pub fn exit(code: i32) -> ! {
        std::process::exit(code)
    }

    pub fn frame_yield() {}

    pub fn sleep_ms(ms: u32) {
        if ms == 0 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
        }
    }
}

#[cfg(target_family = "wasm")]
mod imp {
    use super::{Meta, Open};
    use std::io::{self, SeekFrom};
    use std::path::Path;

    #[link(wasm_import_module = "rc_js")]
    unsafe extern "C" {
        /// 0: no such path, 1: a file, 2: a directory; `out` gets length and change time
        /// (two f64).
        fn fs_stat(path: *const u8, len: usize, out: *mut f64) -> i32;
        /// The names in a directory, separated by `\n`, into `buf`; returns the length needed
        /// (-1: no such directory).
        fn fs_list(path: *const u8, len: usize, buf: *mut u8, cap: usize) -> i32;
        /// A file handle (>= 0), or -1 not found, -2 exists, -3 denied. `flags`: 1 write,
        /// 2 create, 4 create new, 8 truncate.
        fn fs_open(path: *const u8, len: usize, flags: u32) -> i32;
        fn fs_read(fd: i32, at: f64, buf: *mut u8, len: usize) -> i32;
        fn fs_write(fd: i32, at: f64, buf: *const u8, len: usize) -> i32;
        fn fs_size(fd: i32) -> f64;
        fn fs_truncate(fd: i32, size: f64);
        fn fs_flush(fd: i32);
        fn fs_close(fd: i32);
        fn fs_mkdir(path: *const u8, len: usize) -> i32;
        fn fs_remove(path: *const u8, len: usize) -> i32;
        fn now_ms() -> f64;
        fn wall_ms_js() -> f64;
        fn sleep_js(ms: u32);
        /// A setting (the page's address, the harness's environment) into `buf`; returns its
        /// length (-1: not set).
        fn env_get(name: *const u8, len: usize, buf: *mut u8, cap: usize) -> i32;
        fn log_js(msg: *const u8, len: usize);
        /// The game ended (suspends for good).
        fn exit_js(code: i32);
        /// A frame is done: let the browser show it and handle input (suspends).
        fn frame_js();
    }

    pub fn env(name: &str) -> Option<String> {
        let mut buf = vec![0u8; 1024];
        // SAFETY: the pointers are valid for the lengths given.
        let n = unsafe { env_get(name.as_ptr(), name.len(), buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            return None;
        }
        buf.truncate((n as usize).min(1024));
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    pub fn log(msg: &str) {
        // SAFETY: as above.
        unsafe { log_js(msg.as_ptr(), msg.len()) }
    }

    pub fn exit(code: i32) -> ! {
        // SAFETY: a plain call; the player never resumes this stack.
        unsafe { exit_js(code) };
        unreachable!("the game ran on after it ended")
    }

    pub fn frame_yield() {
        // SAFETY: a plain call (it suspends this guest thread's stack).
        unsafe { frame_js() }
    }

    fn bytes(p: &Path) -> &[u8] {
        p.as_os_str().as_encoded_bytes()
    }

    pub fn metadata(p: &Path) -> Option<Meta> {
        let b = bytes(p);
        let mut out = [0f64; 2];
        // SAFETY: the pointers are valid for the lengths given; the import writes two f64.
        let k = unsafe { fs_stat(b.as_ptr(), b.len(), out.as_mut_ptr()) };
        (k != 0).then_some(Meta {
            len: out[0] as u64,
            is_dir: k == 2,
            modified_ms: out[1] as u64,
        })
    }

    pub fn read_dir(p: &Path) -> Option<Vec<String>> {
        let b = bytes(p);
        let mut buf = vec![0u8; 4096];
        loop {
            // SAFETY: as above, `buf` holds `cap` bytes.
            let n = unsafe { fs_list(b.as_ptr(), b.len(), buf.as_mut_ptr(), buf.len()) };
            if n < 0 {
                return None;
            }
            let n = n as usize;
            if n <= buf.len() {
                let s = String::from_utf8_lossy(&buf[..n]);
                return Some(
                    s.split('\n')
                        .filter(|x| !x.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
            }
            buf.resize(n, 0);
        }
    }

    pub fn create_dir_all(p: &Path) -> bool {
        let b = bytes(p);
        // SAFETY: as above.
        unsafe { fs_mkdir(b.as_ptr(), b.len()) == 0 }
    }

    pub fn remove_file(p: &Path) -> bool {
        let b = bytes(p);
        // SAFETY: as above.
        unsafe { fs_remove(b.as_ptr(), b.len()) == 0 }
    }

    pub fn copy(from: &Path, to: &Path) -> bool {
        let Ok(mut a) = File::open(from, Open::default()) else {
            return false;
        };
        let Ok(mut b) = File::open(
            to,
            Open {
                write: true,
                create: true,
                truncate: true,
                ..Open::default()
            },
        ) else {
            return false;
        };
        let mut buf = vec![0u8; 1 << 20];
        loop {
            match a.read(&mut buf) {
                Ok(0) => return b.flush().is_ok(),
                Ok(n) => {
                    if b.write_all(&buf[..n]).is_err() {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
    }

    #[derive(Debug)]
    pub struct File {
        fd: i32,
        pos: u64,
    }

    impl Drop for File {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed once.
            unsafe { fs_close(self.fd) }
        }
    }

    impl File {
        pub fn open(p: &Path, how: Open) -> io::Result<File> {
            let b = bytes(p);
            let flags = u32::from(how.write)
                | (u32::from(how.create) << 1)
                | (u32::from(how.create_new) << 2)
                | (u32::from(how.truncate) << 3);
            // SAFETY: as above.
            let fd = unsafe { fs_open(b.as_ptr(), b.len(), flags) };
            match fd {
                -1 => Err(io::ErrorKind::NotFound.into()),
                -2 => Err(io::ErrorKind::AlreadyExists.into()),
                fd if fd < 0 => Err(io::ErrorKind::PermissionDenied.into()),
                fd => Ok(File { fd, pos: 0 }),
            }
        }
        pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // SAFETY: `buf` is valid for its length.
            let n = unsafe { fs_read(self.fd, self.pos as f64, buf.as_mut_ptr(), buf.len()) };
            if n < 0 {
                return Err(io::Error::other("read failed"));
            }
            self.pos += n as u64;
            Ok(n as usize)
        }
        pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            // SAFETY: `data` is valid for its length.
            let n = unsafe { fs_write(self.fd, self.pos as f64, data.as_ptr(), data.len()) };
            if n < 0 || n as usize != data.len() {
                return Err(io::Error::other("write failed (no room?)"));
            }
            self.pos += n as u64;
            Ok(())
        }
        pub fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            let base = match to {
                SeekFrom::Start(p) => {
                    self.pos = p;
                    return Ok(p);
                }
                SeekFrom::Current(d) => (self.pos as i64, d),
                // SAFETY: a plain call on our handle.
                SeekFrom::End(d) => (unsafe { fs_size(self.fd) } as i64, d),
            };
            let p = base.0 + base.1;
            if p < 0 {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            self.pos = p as u64;
            Ok(self.pos)
        }
        pub fn truncate_here(&mut self) -> io::Result<()> {
            // SAFETY: a plain call on our handle.
            unsafe { fs_truncate(self.fd, self.pos as f64) };
            Ok(())
        }
        pub fn flush(&mut self) -> io::Result<()> {
            // SAFETY: a plain call on our handle.
            unsafe { fs_flush(self.fd) };
            Ok(())
        }
    }

    pub fn monotonic_ns() -> u64 {
        // SAFETY: a plain call.
        (unsafe { now_ms() } * 1e6) as u64
    }

    pub fn wall_ms() -> u64 {
        // SAFETY: a plain call.
        unsafe { wall_ms_js() as u64 }
    }

    pub fn sleep_ms(ms: u32) {
        // SAFETY: a plain call (it suspends this guest thread's stack).
        unsafe { sleep_js(ms) }
    }
}

pub use imp::File;

pub fn metadata(p: &Path) -> Option<Meta> {
    imp::metadata(p)
}
pub fn exists(p: &Path) -> bool {
    metadata(p).is_some()
}
pub fn is_dir(p: &Path) -> bool {
    metadata(p).is_some_and(|m| m.is_dir)
}
pub fn is_file(p: &Path) -> bool {
    metadata(p).is_some_and(|m| !m.is_dir)
}
/// The names in a directory.
pub fn read_dir(p: &Path) -> Option<Vec<String>> {
    imp::read_dir(p)
}
/// The paths in a directory.
pub fn read_dir_paths(p: &Path) -> Option<Vec<PathBuf>> {
    Some(read_dir(p)?.into_iter().map(|n| p.join(n)).collect())
}
pub fn create_dir_all(p: &Path) -> bool {
    imp::create_dir_all(p)
}
pub fn remove_file(p: &Path) -> bool {
    imp::remove_file(p)
}
pub fn copy(from: &Path, to: &Path) -> bool {
    imp::copy(from, to)
}

/// A whole file.
pub fn read(p: &Path) -> io::Result<Vec<u8>> {
    let mut f = File::open(p, Open::default())?;
    let len = f.seek(SeekFrom::End(0))?;
    f.seek(SeekFrom::Start(0))?;
    let mut out = vec![0u8; len as usize];
    let mut at = 0;
    while at < out.len() {
        match f.read(&mut out[at..])? {
            0 => break,
            n => at += n,
        }
    }
    out.truncate(at);
    Ok(out)
}

pub fn read_to_string(p: &Path) -> Option<String> {
    read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Nanoseconds of a monotonic clock (from an arbitrary start).
pub fn monotonic_ns() -> u64 {
    imp::monotonic_ns()
}

/// Milliseconds since 1970.
pub fn wall_ms() -> u64 {
    imp::wall_ms()
}

/// Wait `ms` milliseconds (0: let others run).
pub fn sleep_ms(ms: u32) {
    imp::sleep_ms(ms)
}

/// A moment of the monotonic clock (`std::time::Instant` where `std` has none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stamp(u64);

impl Stamp {
    pub fn now() -> Stamp {
        Stamp(monotonic_ns())
    }
    pub fn elapsed_ns(self) -> u64 {
        monotonic_ns().saturating_sub(self.0)
    }
    pub fn elapsed_ms(self) -> u64 {
        self.elapsed_ns() / 1_000_000
    }
    pub fn elapsed_secs(self) -> f64 {
        self.elapsed_ns() as f64 / 1e9
    }
}

/// A setting from the environment (`RC_*`; in a browser the player's).
pub fn env(name: &str) -> Option<String> {
    imp::env(name)
}

/// A line for the developer (stderr; the browser's console).
pub fn log(msg: &str) {
    imp::log(msg)
}

/// End the process.
pub fn exit(code: i32) -> ! {
    imp::exit(code)
}

/// A frame has been handed over: in a browser, let it show it and handle input before the game
/// goes on.
pub fn frame_yield() {
    imp::frame_yield()
}

/// `eprintln!` through [`log`].
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {
        $crate::host::log(&format!($($t)*))
    };
}
