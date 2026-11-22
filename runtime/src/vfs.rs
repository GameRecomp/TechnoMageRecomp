// SPDX-License-Identifier: MIT
//! The guest's file system: Windows paths onto the host.
//!
//! - `C:\TECHNOMAGE\` (and relative paths, from the current directory) is the game's install.
//! - `D:\` is the CD drive: the folder `recomp extract` copied the disc's run-time files to
//!   (`<install>/cd`), with the disc's label from `cd/volume-label.txt`.
//! - Names match case-insensitively, `\` and `/` both separate.
//! - Files the game writes (settings, saves) go to the user directory, mirroring the
//!   install's layout; reads look there first.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const GAME_ROOT: &str = "C:\\TECHNOMAGE";
pub const CD_DRIVE: char = 'D';

#[derive(Debug, Clone, Default)]
pub struct Vfs {
    pub game_dir: PathBuf,
    pub cd_dir: Option<PathBuf>,
    pub user_dir: Option<PathBuf>,
    pub cd_label: String,
    /// The guest's current directory, a full Windows path.
    pub cwd: String,
}

static VFS: RwLock<Option<Vfs>> = RwLock::new(None);

/// Set up the file system for an install in `game_dir`.
pub fn init(game_dir: &Path, user_dir: Option<PathBuf>) {
    let cd = game_dir.join("cd");
    let cd_dir = cd.is_dir().then_some(cd);
    let cd_label = cd_dir
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join("volume-label.txt")).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    *VFS.write().expect("vfs") = Some(Vfs {
        game_dir: game_dir.to_path_buf(),
        cd_dir,
        user_dir,
        cd_label,
        cwd: GAME_ROOT.to_string(),
    });
}

pub fn get() -> Vfs {
    VFS.read().expect("vfs").clone().unwrap_or_else(|| Vfs {
        cwd: GAME_ROOT.to_string(),
        ..Vfs::default()
    })
}

pub fn set_cwd(dir: &str) -> bool {
    let full = absolute(dir);
    if host_path(&full).is_some_and(|p| p.is_dir()) {
        if let Some(v) = VFS.write().expect("vfs").as_mut() {
            v.cwd = full.trim_end_matches('\\').to_string();
        }
        true
    } else {
        false
    }
}

/// `path` as a full Windows path (drive, backslashes, `.` and `..` resolved).
pub fn absolute(path: &str) -> String {
    let p = path.replace('/', "\\");
    let cwd = get().cwd;
    let full = if p.len() >= 2 && p.as_bytes()[1] == b':' {
        p
    } else if let Some(rest) = p.strip_prefix('\\') {
        format!("{}\\{rest}", cwd.get(..2).unwrap_or("C:"))
    } else {
        format!("{cwd}\\{p}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for (i, c) in full.split('\\').enumerate() {
        match c {
            "" if i > 0 => {}
            "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            c => parts.push(c),
        }
    }
    let mut s = parts.join("\\");
    if s.len() == 2 {
        s.push('\\');
    }
    s
}

/// Which host tree a full guest path is in, and the rest of the path.
fn split(full: &str) -> Option<(PathBuf, Vec<String>, bool)> {
    let v = get();
    let up = full.to_ascii_uppercase();
    let rest: &str;
    let (root, is_game) = if up.starts_with(&GAME_ROOT.to_ascii_uppercase())
        && (up.len() == GAME_ROOT.len() || up.as_bytes()[GAME_ROOT.len()] == b'\\')
    {
        rest = &full[GAME_ROOT.len()..];
        (v.game_dir.clone(), true)
    } else if up.starts_with(&format!("{CD_DRIVE}:")) {
        rest = &full[2..];
        (v.cd_dir.clone()?, false)
    } else {
        return None;
    };
    let parts = rest
        .split('\\')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Some((root, parts, is_game))
}

/// Follow `parts` below `root`, matching each name case-insensitively. The last part may be
/// missing (then it is used as given) when `allow_new`.
fn walk(root: &Path, parts: &[String], allow_new: bool) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        let exact = p.join(part);
        if exact.exists() {
            p = exact;
            continue;
        }
        let found = std::fs::read_dir(&p).ok().and_then(|d| {
            d.filter_map(Result::ok)
                .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))
                .map(|e| e.path())
        });
        match found {
            Some(f) => p = f,
            None if allow_new && i + 1 == parts.len() => p = exact,
            None => return None,
        }
    }
    Some(p)
}

/// The host file a guest path reads from (the user's copy first), if it exists.
pub fn host_path(path: &str) -> Option<PathBuf> {
    let full = absolute(path);
    let (root, parts, is_game) = split(&full)?;
    if is_game {
        if let Some(u) = get().user_dir {
            if let Some(p) = walk(&u, &parts, false) {
                return Some(p);
            }
        }
    }
    walk(&root, &parts, false)
}

/// The host file a guest path writes to: in the user directory for the install (created as
/// needed), never on the CD. `None` for paths outside both.
pub fn host_path_for_write(path: &str) -> Option<PathBuf> {
    let full = absolute(path);
    let (root, parts, is_game) = split(&full)?;
    if !is_game {
        return None;
    }
    let base = get().user_dir.unwrap_or(root);
    if let Some(parent) = parts.split_last().map(|(_, p)| p) {
        let mut d = base.clone();
        for part in parent {
            let next = walk(&d, std::slice::from_ref(part), true)?;
            if !next.exists() {
                std::fs::create_dir_all(&next).ok()?;
            }
            d = next;
        }
        let name = parts.last()?;
        return walk(&d, std::slice::from_ref(name), true);
    }
    Some(base)
}

/// The drive type of a root path (`GetDriveTypeA`): 3 fixed, 5 CD-ROM, 1 no such root.
pub fn drive_type(root: &str) -> u32 {
    match root.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 3,
        Some(d) if d == CD_DRIVE && get().cd_dir.is_some() => 5,
        _ => 1,
    }
}

/// Drives as a bit mask (`GetLogicalDrives`).
pub fn logical_drives() -> u32 {
    let mut m = 1 << 2; // C:
    if get().cd_dir.is_some() {
        m |= 1 << (CD_DRIVE as u32 - 'A' as u32);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_made_absolute() {
        assert_eq!(absolute("C:\\a\\..\\b"), "C:\\b");
        assert_eq!(absolute("D:\\"), "D:\\");
        assert_eq!(absolute("x/y"), format!("{}\\x\\y", get().cwd));
    }
}
