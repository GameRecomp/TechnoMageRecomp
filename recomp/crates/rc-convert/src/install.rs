// SPDX-License-Identifier: MIT
//! A GOG installer or a zip of an installed copy (the Saga) as the converter reads it, from any
//! reader (in a browser: the chosen file in pieces): which game it holds, its lifted programs, and
//! its files, streamed to a sink.

use std::collections::BTreeMap;

use rc_manifest::Manifest;
use rc_media::{Archive, Entry, ReadSeek, Sink};

use crate::Error;
use crate::lift::Program;

/// The file the runtime reads the installer's registry entries from (advapi32).
pub const REGISTRY_FILE: &str = "registry-install.txt";
/// The file that names the release (its variant id: "cd-de", "pl"), for the runtime's language
/// and code page.
pub const VARIANT_FILE: &str = "recomp-variant.txt";

pub struct Install {
    pub installer: Box<dyn Archive>,
    pub manifest: Manifest,
    pub variant: String,
    /// The manifest's binaries found in the installer, by lower-case name.
    binaries: BTreeMap<String, Vec<u8>>,
    registry: String,
}

/// Collects selected files in memory.
struct MemSink<'a> {
    files: &'a mut BTreeMap<String, Vec<u8>>,
    current: String,
}

impl Sink for MemSink<'_> {
    fn dir(&mut self, _: &str) -> rc_media::Result<()> {
        Ok(())
    }
    fn begin(&mut self, path: &str, size: u64) -> rc_media::Result<()> {
        self.current = path.to_ascii_lowercase();
        self.files
            .insert(self.current.clone(), Vec::with_capacity(size as usize));
        Ok(())
    }
    fn write(&mut self, data: &[u8]) -> rc_media::Result<()> {
        if let Some(f) = self.files.get_mut(&self.current) {
            f.extend_from_slice(data);
        }
        Ok(())
    }
    fn end(&mut self) -> rc_media::Result<()> {
        Ok(())
    }
}

impl Install {
    /// Read the installer (or zip) and identify its game: the manifests' binaries are unpacked into
    /// memory and hashed.
    pub fn open(reader: Box<dyn ReadSeek>, ms: &[Manifest]) -> Result<Install, Error> {
        Self::open_with(reader, Vec::new(), ms)
    }

    /// [`Install::open`] for an installer whose data is in external slices (`setup_….exe` and
    /// its `-1.bin`, …), given in slice order.
    pub fn open_with(
        reader: Box<dyn ReadSeek>,
        slices: Vec<Box<dyn ReadSeek>>,
        ms: &[Manifest],
    ) -> Result<Install, Error> {
        let (_, mut installer) =
            rc_media::open_readers(reader, slices).map_err(|e| e.to_string())?;
        let names: std::collections::BTreeSet<String> = ms
            .iter()
            .flat_map(|m| m.binaries.iter().map(|b| b.name.to_ascii_lowercase()))
            .collect();
        let mut binaries = BTreeMap::new();
        let select = |e: &Entry| names.contains(&e.path.to_ascii_lowercase());
        installer
            .extract_to(
                &mut MemSink {
                    files: &mut binaries,
                    current: String::new(),
                },
                &select,
            )
            .map_err(|e| e.to_string())?;
        let (m, variant) = crate::lift::identify(ms, &mut |name| {
            binaries.get(&name.to_ascii_lowercase()).cloned()
        })
        .ok_or("this installer holds none of the games the converter knows")?;
        let registry = rc_media::registry_file(installer.registry());
        Ok(Install {
            manifest: m.clone(),
            variant,
            installer,
            binaries,
            registry,
        })
    }

    /// The game's lifted programs (the main one first).
    pub fn programs(&self, ms: &[Manifest]) -> Result<Vec<Program>, Error> {
        let (_, _, mut progs) = crate::lift::lift_install(ms, &mut |name| {
            self.binaries.get(&name.to_ascii_lowercase()).cloned()
        })?;
        let main = self
            .manifest
            .binaries
            .iter()
            .find(|b| {
                matches!(b.role, rc_manifest::Role::Main) && b.variants.contains(&self.variant)
            })
            .map(|b| b.name.to_ascii_lowercase());
        progs.sort_by_key(|p| Some(p.exe.to_ascii_lowercase()) != main);
        Ok(progs)
    }

    /// The game's directories and files (path, size) as [`Install::files`] writes them.
    pub fn plan(&self) -> (Vec<String>, Vec<(String, u64)>) {
        let dirs = self.dirs();
        let mut files: Vec<(String, u64)> = self
            .installer
            .entries()
            .iter()
            .map(|e| (e.path.clone(), e.size))
            .collect();
        if !self.registry.is_empty() {
            files.push((REGISTRY_FILE.into(), self.registry.len() as u64));
        }
        files.push((VARIANT_FILE.into(), self.variant.len() as u64));
        (dirs, files)
    }

    /// Every directory the files need, and those the installer creates.
    fn dirs(&self) -> Vec<String> {
        let mut dirs: std::collections::BTreeSet<String> =
            self.installer.directories().iter().cloned().collect();
        for e in self.installer.entries() {
            let mut p = e.path.as_str();
            while let Some(cut) = p.rfind('/') {
                p = &p[..cut];
                dirs.insert(p.to_string());
            }
        }
        dirs.into_iter().collect()
    }

    /// The game's files into `sink`: the directories, the installer's files, then the registry
    /// entries the runtime reads (`registry-install.txt`) and the release's name
    /// (`recomp-variant.txt`).
    pub fn files(&mut self, sink: &mut dyn Sink) -> Result<(), Error> {
        let err = |e: rc_media::Error| e.to_string();
        for d in self.dirs() {
            sink.dir(&d).map_err(err)?;
        }
        self.installer
            .extract_to(&mut NoDirs(sink), &|_| true)
            .map_err(err)?;
        if !self.registry.is_empty() {
            sink.begin(REGISTRY_FILE, self.registry.len() as u64)
                .map_err(err)?;
            sink.write(self.registry.as_bytes()).map_err(err)?;
            sink.end().map_err(err)?;
        }
        sink.begin(VARIANT_FILE, self.variant.len() as u64)
            .map_err(err)?;
        sink.write(self.variant.as_bytes()).map_err(err)?;
        sink.end().map_err(err)?;
        Ok(())
    }
}

/// A sink that has had its directories already.
struct NoDirs<'a>(&'a mut dyn Sink);

impl Sink for NoDirs<'_> {
    fn dir(&mut self, _: &str) -> rc_media::Result<()> {
        Ok(())
    }
    fn begin(&mut self, path: &str, size: u64) -> rc_media::Result<()> {
        self.0.begin(path, size)
    }
    fn write(&mut self, data: &[u8]) -> rc_media::Result<()> {
        self.0.write(data)
    }
    fn end(&mut self) -> rc_media::Result<()> {
        self.0.end()
    }
}
