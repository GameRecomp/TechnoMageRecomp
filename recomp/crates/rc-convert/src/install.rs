// SPDX-License-Identifier: MIT
//! A GOG installer or a zip of an installed copy (the Saga) as the converter reads it, from any
//! reader (in a browser: the chosen file in pieces): which game it holds, its lifted programs, and
//! its files, streamed to a sink.

use std::collections::BTreeMap;

use rc_manifest::Manifest;
use rc_media::{Archive, Entry, ReadSeek, Sink};
use std::io::Read as _;

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
    /// Supplements given so far ([`Install::supply`]): their archives and the paths taken.
    supplements: Vec<(Box<dyn Archive>, Vec<String>)>,
    /// The CD's own files ([`Install::add_cd`]): the folders the game reads from the disc.
    cd: Option<(rc_media::iso::Iso, Vec<String>, String)>,
}

/// Prefixes every path (the CD's files go to `cd/`).
struct Prefixed<'a>(&'a mut dyn Sink, &'a str);

impl Sink for Prefixed<'_> {
    fn dir(&mut self, _: &str) -> rc_media::Result<()> {
        Ok(())
    }
    fn begin(&mut self, path: &str, size: u64) -> rc_media::Result<()> {
        self.0.begin(&format!("{}{path}", self.1), size)
    }
    fn write(&mut self, data: &[u8]) -> rc_media::Result<()> {
        self.0.write(data)
    }
    fn end(&mut self) -> rc_media::Result<()> {
        self.0.end()
    }
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
            supplements: Vec::new(),
            cd: None,
        })
    }

    /// The variant's manifest entry.
    fn variant_entry(&self) -> Option<&rc_manifest::Variant> {
        self.manifest.variants.iter().find(|v| v.id == self.variant)
    }

    /// The media each supplement needs (the caller downloads them and hands their readers to
    /// [`Install::supply`], in this order).
    pub fn supplements_needed(&self) -> Vec<Vec<rc_manifest::Media>> {
        self.variant_entry()
            .map(|v| v.supplements.iter().map(|s| s.media.clone()).collect())
            .unwrap_or_default()
    }

    /// Paths (lower case) a supplement provides instead of the medium.
    fn supplied(&self) -> Vec<String> {
        self.variant_entry()
            .map(|v| {
                v.supplements
                    .iter()
                    .flat_map(|s| s.files.iter().map(|f| f.path.to_ascii_lowercase()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Supplement `index`'s media, in the manifest's order: its files replace or add to the
    /// medium's, and a binary among them is the one lifted.
    pub fn supply(
        &mut self,
        index: usize,
        mut readers: Vec<Box<dyn ReadSeek>>,
    ) -> Result<(), Error> {
        let Some(sup) = self
            .variant_entry()
            .and_then(|v| v.supplements.get(index))
            .cloned()
        else {
            return Err(format!("no supplement {index}"));
        };
        if readers.is_empty() {
            return Err("a supplement needs its media".into());
        }
        let mut ar: Box<dyn Archive> = if sup.media[0].kind == rc_manifest::MediaKind::Iscab {
            let mut hdr = Vec::new();
            readers[0]
                .read_to_end(&mut hdr)
                .map_err(|e| e.to_string())?;
            let vols = readers.drain(1..).map(Some).collect();
            Box::new(rc_media::iscab::Cabinet::open_volumes(&hdr, vols).map_err(|e| e.to_string())?)
        } else {
            let first = readers.remove(0);
            rc_media::open_readers(first, readers)
                .map_err(|e| e.to_string())?
                .1
        };
        let paths: Vec<String> = sup
            .files
            .iter()
            .map(|f| f.path.to_ascii_lowercase())
            .collect();
        // Binaries among them (the English variant's executable) go to the lifter.
        let names: std::collections::BTreeSet<String> = self
            .manifest
            .binaries
            .iter()
            .map(|b| b.name.to_ascii_lowercase())
            .collect();
        let mut found = BTreeMap::new();
        let select = |e: &Entry| {
            let p = e.path.to_ascii_lowercase();
            paths.contains(&p) && names.contains(&p)
        };
        ar.extract_to(
            &mut MemSink {
                files: &mut found,
                current: String::new(),
            },
            &select,
        )
        .map_err(|e| e.to_string())?;
        for (name, data) in found {
            let want = sup
                .files
                .iter()
                .find(|f| f.path.eq_ignore_ascii_case(&name));
            if want.is_some_and(|f| rc_manifest::sha256_hex(&data) != f.sha256) {
                return Err(format!(
                    "{name}: the supplied file differs from the manifest's"
                ));
            }
            self.binaries.insert(name, data);
        }
        self.supplements.push((ar, paths));
        Ok(())
    }

    /// The CD image itself (the medium again, read as ISO 9660): the folders the game reads from
    /// its disc (`cd_dirs`) go to `cd/`, with the disc's label.
    pub fn add_cd(&mut self, reader: Box<dyn ReadSeek>) -> Result<(), Error> {
        let dirs = self
            .variant_entry()
            .map(|v| v.cd_dirs.clone())
            .unwrap_or_default();
        if dirs.is_empty() {
            return Ok(());
        }
        let iso = rc_media::open_cd_files(reader).map_err(|e| e.to_string())?;
        let label = iso.volume_id().to_string();
        self.cd = Some((iso, dirs, label));
        Ok(())
    }

    fn in_cd_dirs(dirs: &[String], path: &str) -> bool {
        let top = path.split('/').next().unwrap_or("");
        dirs.iter().any(|d| d.eq_ignore_ascii_case(top))
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
        let supplied = self.supplied();
        let mut files: Vec<(String, u64)> = self
            .installer
            .entries()
            .iter()
            .filter(|e| !supplied.contains(&e.path.to_ascii_lowercase()))
            .map(|e| (e.path.clone(), e.size))
            .collect();
        for (ar, paths) in &self.supplements {
            files.extend(
                ar.entries()
                    .iter()
                    .filter(|e| paths.contains(&e.path.to_ascii_lowercase()))
                    .map(|e| (e.path.clone(), e.size)),
            );
        }
        if let Some((iso, cd_dirs, label)) = &self.cd {
            files.extend(
                iso.entries()
                    .iter()
                    .filter(|e| Self::in_cd_dirs(cd_dirs, &e.path))
                    .map(|e| (format!("cd/{}", e.path), e.size)),
            );
            files.push(("cd/volume-label.txt".into(), label.len() as u64 + 1));
        }
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
        let mut add = |path: &str| {
            let mut p = path;
            while let Some(cut) = p.rfind('/') {
                p = &p[..cut];
                dirs.insert(p.to_string());
            }
        };
        for e in self.installer.entries() {
            add(&e.path);
        }
        for (ar, paths) in &self.supplements {
            for e in ar
                .entries()
                .iter()
                .filter(|e| paths.contains(&e.path.to_ascii_lowercase()))
            {
                add(&e.path);
            }
        }
        if let Some((iso, cd_dirs, _)) = &self.cd {
            for e in iso
                .entries()
                .iter()
                .filter(|e| Self::in_cd_dirs(cd_dirs, &e.path))
            {
                add(&format!("cd/{}", e.path));
            }
            add("cd/x");
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
        let supplied = self.supplied();
        self.installer
            .extract_to(&mut NoDirs(sink), &|e| {
                !supplied.contains(&e.path.to_ascii_lowercase())
            })
            .map_err(err)?;
        for (ar, paths) in &mut self.supplements {
            let paths = paths.clone();
            ar.extract_to(&mut NoDirs(sink), &|e| {
                paths.contains(&e.path.to_ascii_lowercase())
            })
            .map_err(err)?;
        }
        if let Some((iso, cd_dirs, label)) = &mut self.cd {
            let dirs = cd_dirs.clone();
            iso.extract_to(&mut Prefixed(sink, "cd/"), &|e| {
                Self::in_cd_dirs(&dirs, &e.path)
            })
            .map_err(err)?;
            let text = format!("{label}\n");
            sink.begin("cd/volume-label.txt", text.len() as u64)
                .map_err(err)?;
            sink.write(text.as_bytes()).map_err(err)?;
            sink.end().map_err(err)?;
        }
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
