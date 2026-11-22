// SPDX-License-Identifier: MIT
//! Packages that run without a compiler, as TropicoRecomp's converter makes them: a shell template
//! (the game's program without lifted code, built by CI for a system) with the game's native code
//! and its files in the template's `data/` directory.
//!
//! - [`make_template`]: a shell and the targets it runs, as a template zip.
//! - [`template`]: read one.
//! - [`package`]: the template's files, `data/code-<target>-<exe>.bin` for each program and target,
//!   and the game's files under `data/game/`.
//!
//! Nothing here touches the file system: the caller reads and writes, so the crate can run in a
//! browser later as TropicoRecomp's does.

pub mod apk;
pub mod install;
pub mod lift;
pub mod zip;

pub use rc_emit_native;
pub use rc_manifest;
pub use rc_media;

use std::io::Write;

use rc_emit_native::Target;

pub type Error = String;

/// The target names a template lists for each system (TropicoRecomp's).
pub fn default_targets(system: &str) -> Option<&'static [&'static str]> {
    match system {
        "windows" => Some(&["x86_64-windows"]),
        "linux" => Some(&["x86_64-sysv"]),
        "macos" => Some(&["arm64-apple", "x86_64-sysv"]),
        "android" => Some(&["arm64-linux", "x86_64-sysv"]),
        _ => None,
    }
}

/// How a game's packages are named: the folder (and file stem) in the zip.
pub fn stem(game: &str) -> &'static str {
    match game {
        "sacked" => "Sacked",
        "crazyschool" => "CrazySchool",
        _ => "Game",
    }
}

/// A template: the shell `shell_name` (its bytes) and the targets it runs, in `<stem>/`.
pub fn make_template(
    game: &str,
    system: &str,
    shell_name: &str,
    shell: &[u8],
) -> Result<Vec<u8>, Error> {
    let targets = default_targets(system).ok_or_else(|| format!("unknown system {system:?}"))?;
    let io = |e: std::io::Error| e.to_string();
    if system == "android" {
        // The shell is an APK (tools/build-android): its entries, and assets/data/ for the code
        // and the game's files.
        let entries = zip::read_stored(shell)?;
        let mut z = zip::ZipWriter::new(Vec::new());
        for e in &entries {
            if e.is_dir {
                z.dir(&e.name).map_err(io)?;
            } else {
                z.file(&e.name, e.mode, &e.data).map_err(io)?;
            }
        }
        z.dir("assets/").map_err(io)?;
        z.dir("assets/data/").map_err(io)?;
        z.file(
            "assets/data/targets",
            0o644,
            format!("{}\n", targets.join(" ")).as_bytes(),
        )
        .map_err(io)?;
        return z.finish().map_err(io);
    }
    let root = format!("{}/", stem(game));
    let mut z = zip::ZipWriter::new(Vec::new());
    z.dir(&root).map_err(io)?;
    z.file(&format!("{root}{shell_name}"), 0o755, shell)
        .map_err(io)?;
    z.dir(&format!("{root}data/")).map_err(io)?;
    z.file(
        &format!("{root}data/targets"),
        0o644,
        format!("{}\n", targets.join(" ")).as_bytes(),
    )
    .map_err(io)?;
    z.finish().map_err(io)
}

/// A read template.
#[derive(Debug)]
pub struct Template<'a> {
    pub entries: Vec<zip::StoredEntry<'a>>,
    /// The `data/` directory's path in the zip, ending in `/`.
    pub data: String,
    pub targets: Vec<Target>,
    /// An Android package: the output is a signed APK, its data under `assets/data/`.
    pub apk: bool,
}

/// A target by its name in a template's `targets`.
pub fn target_named(name: &str) -> Option<Target> {
    [
        Target::X64SysV,
        Target::X64Windows,
        Target::Arm64Linux,
        Target::Arm64Apple,
    ]
    .into_iter()
    .find(|t| t.name() == name)
}

pub fn template(bytes: &[u8]) -> Result<Template<'_>, Error> {
    let entries = zip::read_stored(bytes)?;
    let data = entries
        .iter()
        .find(|e| e.is_dir && (e.name == "data/" || e.name.ends_with("/data/")))
        .map(|e| e.name.clone())
        .ok_or("shell template: no data/ directory")?;
    let list = entries
        .iter()
        .find(|e| e.name == format!("{data}targets"))
        .ok_or("shell template: no data/targets")?;
    let targets = String::from_utf8_lossy(&list.data)
        .split_whitespace()
        .map(|t| target_named(t).ok_or_else(|| format!("shell template: target {t}")))
        .collect::<Result<Vec<_>, _>>()?;
    if targets.is_empty() {
        return Err("shell template: no targets".into());
    }
    let apk = entries.iter().any(|e| e.name == "AndroidManifest.xml");
    Ok(Template {
        entries,
        data,
        targets,
        apk,
    })
}

/// Write the package: the template's files (but `targets`), `data/code-<target>-<exe>.bin` for
/// each blob (`blobs`: target, executable's lower-case name, bytes), then the game's files under
/// `data/game/`, which `game` hands to the sink it gets (directories first, then one file after
/// the other). For an Android template the result is an APK signed with `key` (the converter's
/// built-in one if none), entries aligned as zipalign does, without directory entries, and an
/// index `files.txt` (a line per file: its size and path; directories end in `/`) that the app
/// unpacks from on its first start (rc_plat_bundled_data). `progress` hears the bytes of each
/// game file written.
pub fn package<W: Write>(
    out: W,
    tpl: &Template<'_>,
    blobs: &[(Target, String, Vec<u8>)],
    game: &mut dyn FnMut(&mut dyn rc_media::Sink) -> Result<(), Error>,
    key: Option<&apk::ApkKey>,
    progress: &mut dyn FnMut(u64),
) -> Result<W, Error> {
    let io = |e: std::io::Error| e.to_string();
    if !tpl.apk {
        let mut z = zip::ZipWriter::new(out);
        write_contents(&mut z, tpl, blobs, game, 1, progress)?;
        return z.finish().map_err(io);
    }
    let builtin;
    let key = match key {
        Some(k) => k,
        None => {
            builtin = apk::ApkKey::builtin();
            &builtin
        }
    };
    let mut z = zip::ZipWriter::new(apk::Chunked::new(out));
    let list = write_contents(&mut z, tpl, blobs, game, 4, progress)?;
    z.file_aligned(&format!("{}files.txt", tpl.data), 0o644, list.as_bytes(), 4)
        .map_err(io)?;
    apk::finish(z, key).map_err(io)
}

/// The game's files into the zip below `prefix`, each collected whole first (the zip writer
/// needs its size and CRC up front); the APK's index on the side.
struct ZipSink<'z, 'p, W: Write> {
    z: &'z mut zip::ZipWriter<W>,
    prefix: String,
    align: u32,
    path: String,
    data: Vec<u8>,
    list: String,
    progress: &'p mut dyn FnMut(u64),
}

fn media_err(e: std::io::Error) -> rc_media::Error {
    rc_media::Error::Io(e)
}

impl<W: Write> rc_media::Sink for ZipSink<'_, '_, W> {
    fn dir(&mut self, path: &str) -> rc_media::Result<()> {
        if self.align == 1 {
            self.z
                .dir(&format!("{}{path}/", self.prefix))
                .map_err(media_err)?;
        }
        self.list.push_str(&format!("0 game/{path}/\n"));
        Ok(())
    }
    fn begin(&mut self, path: &str, size: u64) -> rc_media::Result<()> {
        self.path = path.to_string();
        self.data.clear();
        self.data.reserve(size as usize);
        Ok(())
    }
    fn write(&mut self, data: &[u8]) -> rc_media::Result<()> {
        self.data.extend_from_slice(data);
        Ok(())
    }
    fn end(&mut self) -> rc_media::Result<()> {
        self.z
            .file_aligned(
                &format!("{}{}", self.prefix, self.path),
                0o644,
                &self.data,
                self.align,
            )
            .map_err(media_err)?;
        self.list
            .push_str(&format!("{} game/{}\n", self.data.len(), self.path));
        (self.progress)(self.data.len() as u64);
        Ok(())
    }
}

/// The template's entries, the code and the game's files; returns the APK's index.
fn write_contents<W: Write>(
    z: &mut zip::ZipWriter<W>,
    tpl: &Template<'_>,
    blobs: &[(Target, String, Vec<u8>)],
    game: &mut dyn FnMut(&mut dyn rc_media::Sink) -> Result<(), Error>,
    align: u32,
    progress: &mut dyn FnMut(u64),
) -> Result<String, Error> {
    let io = |e: std::io::Error| e.to_string();
    let apk = align > 1;
    for e in &tpl.entries {
        if e.name == format!("{}targets", tpl.data) || (apk && e.is_dir) {
            continue;
        }
        if e.is_dir {
            z.dir(&e.name).map_err(io)?;
        } else {
            // Native libraries page-aligned, everything else to 4 bytes, as zipalign does.
            let a = if apk && e.name.ends_with(".so") {
                4096
            } else {
                align
            };
            z.file_aligned(&e.name, e.mode, &e.data, a).map_err(io)?;
        }
    }
    let mut list = String::new();
    for (t, exe, blob) in blobs {
        let name = format!("code-{}-{exe}.bin", t.name());
        z.file_aligned(&format!("{}{name}", tpl.data), 0o644, blob, align)
            .map_err(io)?;
        list.push_str(&format!("{} {name}\n", blob.len()));
    }
    let prefix = format!("{}game/", tpl.data);
    if !apk {
        z.dir(&prefix).map_err(io)?;
    }
    list.push_str("0 game/\n");
    let mut sink = ZipSink {
        z,
        prefix,
        align,
        path: String::new(),
        data: Vec::new(),
        list,
        progress,
    };
    game(&mut sink)?;
    Ok(sink.list)
}

/// The game's files of an installed folder into `sink`, sorted, directories first in each.
pub fn folder_files(root: &std::path::Path, sink: &mut dyn rc_media::Sink) -> Result<(), Error> {
    fn walk(root: &std::path::Path, rel: &str, sink: &mut dyn rc_media::Sink) -> Result<(), Error> {
        let dir = root.join(rel);
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if e.path().is_dir() {
                sink.dir(&path).map_err(|e| e.to_string())?;
                walk(root, &path, sink)?;
            } else {
                let data =
                    std::fs::read(e.path()).map_err(|x| format!("{}: {x}", e.path().display()))?;
                let err = |e: rc_media::Error| e.to_string();
                sink.begin(&path, data.len() as u64).map_err(err)?;
                sink.write(&data).map_err(err)?;
                sink.end().map_err(err)?;
            }
        }
        Ok(())
    }
    walk(root, "", sink)
}
