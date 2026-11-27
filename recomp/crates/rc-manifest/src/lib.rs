// SPDX-License-Identifier: MIT
//! Loader for `games/<id>/game.toml` and a hash catalogue over all of them.
//!
//! The manifests are the single source of truth for which installers, media and binaries
//! the project accepts (see `docs/TARGETS.md`).

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub game: Game,
    #[serde(rename = "variant")]
    pub variants: Vec<Variant>,
    #[serde(rename = "binary", default)]
    pub binaries: Vec<Binary>,
    /// Patch definitions, filled in later.
    #[serde(default)]
    pub patch: Option<toml::Table>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub title_de: String,
    pub engine: Engine,
    pub gog_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// Cultures 1 (CulturesRecomp).
    C1,
    /// The Cultures 2 engine (CulturesRecomp).
    C2,
    /// Heroes of Might and Magic IV (HoMM4Recomp).
    H4,
    /// Limbic Entertainment's 2D engine: Sacked / Gefeuert!, Crazy School.
    Limbic,
    /// Sunflowers' TechnoMage engine (TechnoMageRecomp).
    Technomage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub id: String,
    pub store: String,
    pub language: String,
    pub content: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub media: Vec<Media>,
    /// Folders the game reads from its CD at run time (`Mov`, `Mus`, `Spc`): `recomp extract`
    /// copies them from the CD image into `cd/` beside the install, with the disc's label in
    /// `cd/volume-label.txt`, and the runtime presents that folder as the CD drive.
    #[serde(default)]
    pub cd_dirs: Vec<String>,
    /// Files the medium lacks or holds damaged, and the other archived media they are taken
    /// from instead (the German TechnoMage cover CD's 20 unreadable sound banks).
    #[serde(default, rename = "supplement")]
    pub supplements: Vec<Supplement>,
}

/// Other media that supply some of a variant's files: downloaded beside the main medium, and
/// only the listed files taken from them, each checked by size and SHA-256.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Supplement {
    #[serde(default)]
    pub notes: Option<String>,
    pub media: Vec<Media>,
    pub files: Vec<SuppliedFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuppliedFile {
    /// Path in the install, `/`-separated, as the main medium names it.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub file: String,
    pub kind: MediaKind,
    pub size: u64,
    pub sha256: String,
    /// Where the converter downloads the file (abandonware archived on archive.org); none for
    /// media the user brings.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// Inno Setup installer (GOG).
    Inno,
    /// An external data slice of the Inno Setup installer in the same variant
    /// (`setup_….exe` keeps the headers, `setup_…-1.bin` the file data).
    Slice,
    /// ISO 9660 CD image, cooked (`.iso`) or raw (`.bin` with 2352-byte sectors).
    Iso,
    /// Zip archive of an installed copy (the original directory layout, uncompressed or deflated).
    Zip,
    /// Nullsoft (NSIS) installer.
    Nsis,
    /// An InstallShield cabinet set on its own: the header (`.hdr`) first, then the volumes.
    Iscab,
    /// A CD image packed for archiving (a tar or 7z archive, or a 7z inside a tar), or the first
    /// part of one split into parts (`.001`): the image is taken out and read as `iso`.
    Packed,
    /// The next part of the medium before it (`.002`, …), joined to it.
    Part,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binary {
    pub name: String,
    pub role: Role,
    pub treatment: Treatment,
    pub size: u64,
    pub sha256: String,
    pub variants: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// The packer the shipped file is packed with (`"upx-1.07"`); the recompiler restores the
    /// image first (`rc-unpack`) and checks it against `unpacked_size` and `unpacked_sha256`.
    #[serde(default)]
    pub packed: Option<String>,
    #[serde(default)]
    pub unpacked_size: Option<u64>,
    #[serde(default)]
    pub unpacked_sha256: Option<String>,
    /// Load address for a lifted DLL whose preferred base collides with another lifted
    /// binary. The recompiler applies the relocations before discovery and lifting, and the
    /// runtime loader maps the DLL here.
    #[serde(default)]
    pub base: Option<u32>,
    /// Hints for code discovery: what analysis cannot find by itself (from M2 on).
    #[serde(default)]
    pub discover: Option<DiscoverHints>,
    /// The game project's patches to this binary.
    #[serde(default)]
    pub patch: Patch,
}

impl Binary {
    /// Directory name below `generated/<game>/` for this binary: the lower-case file name
    /// and the first variant, e.g. `cultures.exe-gog-en`. Unique within a game because a
    /// variant has at most one binary of each name.
    pub fn output_dir_name(&self) -> String {
        let v = self.variants.first().map_or("any", String::as_str);
        format!("{}-{v}", self.name.to_ascii_lowercase())
    }

    /// What the manifest gives the lift of this binary besides the binary itself (the
    /// discovery hints and the patches), as text. The lift writes it beside the lifted code
    /// ([`LIFT_RECORD`]); [`stale_lifts`] compares it with the manifest's.
    pub fn lift_record(&self) -> String {
        Self::lift_record_of(&self.discover, &self.patch)
    }

    fn lift_record_of(discover: &Option<DiscoverHints>, patch: &Patch) -> String {
        format!("{discover:#?}\n{patch:#?}\n")
    }
}

/// The file in a lifted binary's directory that holds its [`Binary::lift_record`].
pub const LIFT_RECORD: &str = "lift-inputs.txt";

/// The lifted binaries in `generated` (`generated/<game>/`, a directory per binary) lifted
/// with other discovery hints or patches than `m` now has: their directory names. A directory
/// without a record (lifted before records were written) is stale unless the manifest gives
/// its binary neither; one no binary of `m` names is left alone.
pub fn stale_lifts(m: &Manifest, generated: &Path) -> Vec<String> {
    let mut stale = Vec::new();
    let Ok(entries) = fs::read_dir(generated) else {
        return stale;
    };
    let empty = Binary::lift_record_of(&None, &Patch::default());
    for e in entries.filter_map(Result::ok) {
        let dir = e.path();
        let Some(name) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        let Some(b) = m.binaries.iter().find(|b| b.output_dir_name() == name) else {
            continue;
        };
        let want = b.lift_record();
        let have = fs::read_to_string(dir.join(LIFT_RECORD)).unwrap_or_else(|_| empty.clone());
        if have != want {
            stale.push(name);
        }
    }
    stale.sort();
    stale
}

/// Where the game project changes what the game does (`[binary.patch]`, as in TropicoRecomp).
/// Its code is the game project's (`games/<id>/src/patches.rs` here, `patches.c` in the C sister
/// projects); everything here is applied when the binary is lifted, to the IR or to the image.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// Functions the game project wraps: calls and jumps to them are lifted
    /// as transfers through the runtime's dispatch, where the wrapper is
    /// registered (rc_hook); it calls the lifted original itself.
    #[serde(default)]
    pub hooks: Vec<u32>,
    /// Instructions before which the runtime calls the game project's code
    /// with the whole CPU state (rc_point), which may change it.
    #[serde(default)]
    pub points: Vec<u32>,
    /// Constants of instructions that are read from guest memory instead,
    /// where the game project puts the value it wants.
    #[serde(default)]
    pub operands: Vec<Operand>,
    /// Instruction bytes replaced, length for length, before discovery.
    #[serde(default)]
    pub code: Vec<CodePatch>,
}

/// In the instruction at `at`, the 32-bit constant `value` becomes a load
/// of the dword at `load`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operand {
    pub at: u32,
    pub value: u32,
    pub load: u32,
}

/// The bytes `from` at `at` become `to` (hex, spaces allowed).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodePatch {
    pub at: u32,
    pub from: String,
    pub to: String,
}

impl CodePatch {
    /// `from` and `to` as bytes; an error if they are not hex or differ in
    /// length.
    pub fn bytes(&self) -> Result<(Vec<u8>, Vec<u8>), String> {
        let hex = |s: &str| -> Result<Vec<u8>, String> {
            let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            if !t.len().is_multiple_of(2) {
                return Err(format!("{s:?}: odd number of hex digits"));
            }
            (0..t.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&t[i..i + 2], 16).map_err(|e| format!("{s:?}: {e}")))
                .collect()
        };
        let (a, b) = (hex(&self.from)?, hex(&self.to)?);
        if a.len() != b.len() || a.is_empty() {
            return Err(format!(
                "code patch at {:#010x}: from and to differ in length",
                self.at
            ));
        }
        Ok((a, b))
    }
}

/// Per-binary discovery hints. All addresses are virtual addresses after rebasing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverHints {
    /// Extra function entry points.
    #[serde(default)]
    pub seeds: Vec<u32>,
    /// Functions that never return to their caller.
    #[serde(default)]
    pub no_return: Vec<u32>,
    /// Functions checked by hand to return although a call to them falls into another
    /// function (`no-return?` candidates that are MSVC x87 helpers).
    #[serde(default)]
    pub returns: Vec<u32>,
    /// Ranges inside code sections that hold data (`end` exclusive).
    #[serde(default)]
    pub data: Vec<Range>,
    /// Switch tables that analysis cannot bound: `jmp` at `at`, `count` dword entries at `table`.
    #[serde(default)]
    pub jump_tables: Vec<JumpTableHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JumpTableHint {
    pub at: u32,
    pub table: u32,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Main,
    /// Another single-player build shipped next to `main` (e.g. the Saga's `Game.exe`).
    Alternate,
    Multiplayer,
    Editor,
    Library,
    /// A helper program shipped with the game (setup or diagnostics tools).
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Treatment {
    /// Statically recompiled.
    Lift,
    /// Reimplemented in `hle/`.
    Hle,
    /// Not needed at all.
    Ignore,
}

macro_rules! display_lower {
    ($($t:ty),*) => {$(
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&format!("{self:?}").to_lowercase())
            }
        }
    )*};
}
display_lower!(Engine, MediaKind, Role, Treatment);

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, io::Error),
    Parse(PathBuf, String),
    Invalid(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Invalid(errs) => write!(f, "invalid manifests:\n  {}", errs.join("\n  ")),
        }
    }
}

impl std::error::Error for Error {}

pub fn parse_str(text: &str, origin: &Path) -> Result<Manifest, Error> {
    toml::from_str(text).map_err(|e| Error::Parse(origin.to_owned(), e.to_string()))
}

/// Load every `<dir>/*/game.toml`, sorted by game id, and validate them together.
pub fn load_dir(dir: &Path) -> Result<Vec<Manifest>, Error> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| Error::Io(dir.to_owned(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::Io(dir.to_owned(), e))?;
        let path = entry.path().join("game.toml");
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| Error::Io(path.clone(), e))?;
        out.push(parse_str(&text, &path)?);
    }
    out.sort_by(|a, b| a.game.id.cmp(&b.game.id));
    validate(&out)?;
    Ok(out)
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Cross-check a set of manifests. Every problem is collected, not just the first.
pub fn validate(manifests: &[Manifest]) -> Result<(), Error> {
    let mut errs = Vec::new();
    let mut game_ids = BTreeSet::new();
    let mut media_hashes = BTreeMap::new();
    for m in manifests {
        let g = &m.game.id;
        if m.schema != SCHEMA_VERSION {
            errs.push(format!(
                "{g}: schema {} (expected {SCHEMA_VERSION})",
                m.schema
            ));
        }
        if !game_ids.insert(g.clone()) {
            errs.push(format!("{g}: duplicate game id"));
        }
        let mut variant_ids = BTreeSet::new();
        for v in &m.variants {
            if !variant_ids.insert(v.id.as_str()) {
                errs.push(format!("{g}/{}: duplicate variant id", v.id));
            }
            if v.media.is_empty() {
                errs.push(format!("{g}/{}: no media", v.id));
            }
            for s in &v.supplements {
                for md in &s.media {
                    if !is_sha256(&md.sha256) {
                        errs.push(format!("{g}/{}: bad sha256 for {}", v.id, md.file));
                    }
                }
                for f in &s.files {
                    if !is_sha256(&f.sha256) {
                        errs.push(format!("{g}/{}: bad sha256 for {}", v.id, f.path));
                    }
                }
            }
            for md in &v.media {
                if !is_sha256(&md.sha256) {
                    errs.push(format!("{g}/{}: bad sha256 for {}", v.id, md.file));
                }
                if let Some(prev) = media_hashes.insert(md.sha256.clone(), format!("{g}/{}", v.id))
                {
                    errs.push(format!(
                        "{g}/{}: media {} also listed by {prev}",
                        v.id, md.file
                    ));
                }
            }
        }
        let mut seen = BTreeSet::new();
        for b in &m.binaries {
            if !is_sha256(&b.sha256) {
                errs.push(format!("{g}: bad sha256 for {}", b.name));
            }
            if !seen.insert(b.sha256.as_str()) {
                errs.push(format!("{g}: {} ({}) listed twice", b.name, b.sha256));
            }
            if b.packed.is_some() != (b.unpacked_size.is_some() && b.unpacked_sha256.is_some()) {
                errs.push(format!(
                    "{g}: {} needs packed, unpacked_size and unpacked_sha256 together",
                    b.name
                ));
            }
            if let Some(h) = &b.unpacked_sha256 {
                if !is_sha256(h) {
                    errs.push(format!("{g}: bad unpacked_sha256 for {}", b.name));
                }
            }
            if b.variants.is_empty() {
                errs.push(format!("{g}: {} has no variants", b.name));
            }
            if b.base.is_some() && b.treatment != Treatment::Lift {
                errs.push(format!("{g}: {} has a base but is not lifted", b.name));
            }
            if let Some(base) = b.base {
                if base & 0xffff != 0 {
                    errs.push(format!(
                        "{g}: {} base {base:#x} is not 64 KiB aligned",
                        b.name
                    ));
                }
            }
            if let Some(h) = &b.discover {
                for r in &h.data {
                    if r.start >= r.end {
                        errs.push(format!(
                            "{g}: {} data range {:#x}..{:#x} is empty",
                            b.name, r.start, r.end
                        ));
                    }
                }
                for t in &h.jump_tables {
                    if t.count == 0 || t.count > 4096 {
                        errs.push(format!(
                            "{g}: {} jump table at {:#x} has count {}",
                            b.name, t.at, t.count
                        ));
                    }
                }
            }
            for v in &b.variants {
                if !variant_ids.contains(v.as_str()) {
                    errs.push(format!("{g}: {} refers to unknown variant {v}", b.name));
                }
            }
        }
        for v in &m.variants {
            let mains = m
                .binaries
                .iter()
                .filter(|b| b.role == Role::Main && b.variants.contains(&v.id))
                .count();
            if mains != 1 {
                errs.push(format!("{g}/{}: {mains} main executables (need 1)", v.id));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(errs))
    }
}

/// What a hash matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Media {
        game: String,
        variant: String,
        kind: MediaKind,
        file: String,
    },
    Binary {
        game: String,
        variants: Vec<String>,
        name: String,
        role: Role,
        treatment: Treatment,
    },
}

impl fmt::Display for Hit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hit::Media {
                game,
                variant,
                kind,
                file,
            } => write!(f, "{game}/{variant} {kind} media ({file})"),
            Hit::Binary {
                game,
                variants,
                name,
                role,
                treatment,
            } => write!(
                f,
                "{game}/{} {name} [{role}, {treatment}]",
                variants.join(",")
            ),
        }
    }
}

/// Hash → matches over all manifests. A binary can appear in several games
/// (for example `gedx8musicdrv.dll`), so one hash may yield several hits.
#[derive(Debug, Default)]
pub struct Catalog {
    by_hash: BTreeMap<String, Vec<Hit>>,
    sizes: BTreeSet<u64>,
}

impl Catalog {
    pub fn new(manifests: &[Manifest]) -> Self {
        let mut c = Catalog::default();
        for m in manifests {
            for v in &m.variants {
                for md in &v.media {
                    c.sizes.insert(md.size);
                    c.by_hash
                        .entry(md.sha256.clone())
                        .or_default()
                        .push(Hit::Media {
                            game: m.game.id.clone(),
                            variant: v.id.clone(),
                            kind: md.kind,
                            file: md.file.clone(),
                        });
                }
            }
            for b in &m.binaries {
                c.sizes.insert(b.size);
                c.by_hash
                    .entry(b.sha256.clone())
                    .or_default()
                    .push(Hit::Binary {
                        game: m.game.id.clone(),
                        variants: b.variants.clone(),
                        name: b.name.clone(),
                        role: b.role,
                        treatment: b.treatment,
                    });
            }
        }
        c
    }

    /// Cheap pre-filter: only files with a known size need hashing.
    pub fn is_known_size(&self, size: u64) -> bool {
        self.sizes.contains(&size)
    }

    pub fn lookup(&self, sha256: &str) -> &[Hit] {
        self.by_hash.get(sha256).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

/// Lower-case hex SHA-256 of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Lower-case hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Locate the `games/` directory: `$RC_GAMES_DIR`, else the nearest `games/` in the current
/// directory or one of its parents, else the copy in this source tree.
pub fn default_games_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RC_GAMES_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            let g = dir.join("games");
            let has_manifest = std::fs::read_dir(&g)
                .map(|d| {
                    d.filter_map(Result::ok)
                        .any(|e| e.path().join("game.toml").is_file())
                })
                .unwrap_or(false);
            if has_manifest {
                return g;
            }
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_games() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games")
    }

    #[test]
    fn all_manifests_are_consistent() {
        let ms = load_dir(&repo_games()).unwrap_or_else(|e| panic!("{e}"));
        let ids: Vec<_> = ms.iter().map(|m| m.game.id.as_str()).collect();
        assert_eq!(ids, ["technomage"]);
        let c = Catalog::new(&ms);
        // Every medium can be downloaded from archive.org.
        assert!(ms
            .iter()
            .flat_map(|m| &m.variants)
            .flat_map(|v| &v.media)
            .all(|m| m
                .url
                .as_deref()
                .is_some_and(|u| u.starts_with("https://archive.org/download/"))));
        // The main executable is packed and its restored image is pinned too.
        let main = ms
            .iter()
            .flat_map(|m| &m.binaries)
            .find(|b| b.role == Role::Main)
            .expect("a main binary");
        assert_eq!(main.packed.as_deref(), Some("upx-1.07"));
        assert!(!c.is_empty());
    }

    #[test]
    fn the_german_cd_image_is_known() {
        let ms = load_dir(&repo_games()).unwrap();
        let c = Catalog::new(&ms);
        let bin = c.lookup("c9db7f91ae3d7755b88f9921c0904ee9f335463c4dde700c779454bd918e8ed7");
        assert!(matches!(
            bin,
            [Hit::Media {
                kind: MediaKind::Iso,
                ..
            }]
        ));
        assert!(c.lookup(&"0".repeat(64)).is_empty());
    }

    #[test]
    fn the_converter_page_offers_every_medium() {
        // web/games.js lists the releases the page downloads: the same files as the manifests.
        let page = fs::read_to_string(repo_games().join("../web/games.js")).expect("web/games.js");
        let ms = load_dir(&repo_games()).unwrap();
        for v in ms.iter().flat_map(|m| &m.variants) {
            for md in &v.media {
                assert!(
                    page.contains(&md.sha256),
                    "web/games.js lacks {} ({})",
                    md.file,
                    md.sha256
                );
                assert!(
                    page.contains(&md.size.to_string()),
                    "web/games.js lacks the size of {}",
                    md.file
                );
                if let Some(u) = &md.url {
                    let path = u.trim_start_matches("https://archive.org/download/");
                    assert!(
                        page.contains(path),
                        "web/games.js lacks the URL of {}",
                        md.file
                    );
                }
            }
            for md in v.supplements.iter().flat_map(|s| &s.media) {
                assert!(page.contains(&md.sha256), "web/games.js lacks {}", md.file);
                let path = md
                    .url
                    .as_deref()
                    .unwrap_or_default()
                    .trim_start_matches("https://archive.org/download/");
                assert!(
                    page.contains(path),
                    "web/games.js lacks the URL of {}",
                    md.file
                );
            }
        }
    }

    #[test]
    fn validation_reports_errors() {
        let text = r#"
            schema = 1
            [game]
            id = "x"
            title = "X"
            title_de = "X"
            engine = "c1"
            [[variant]]
            id = "a"
            store = "gog"
            language = "en"
            content = ["base"]
            media = [{ file = "f", kind = "inno", size = 1, sha256 = "nothex" }]
            [[binary]]
            name = "X.exe"
            role = "main"
            treatment = "lift"
            size = 1
            sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
            variants = ["b"]
        "#;
        let m = parse_str(text, Path::new("test")).unwrap();
        let Err(Error::Invalid(errs)) = validate(&[m]) else {
            panic!("expected validation errors")
        };
        assert!(errs.iter().any(|e| e.contains("bad sha256")));
        assert!(errs.iter().any(|e| e.contains("unknown variant b")));
        assert!(errs.iter().any(|e| e.contains("0 main executables")));
    }

    #[test]
    fn binary_hints_parse_and_validate() {
        let text = r#"
            schema = 1
            [game]
            id = "x"
            title = "X"
            title_de = "X"
            engine = "c1"
            [[variant]]
            id = "a"
            store = "gog"
            language = "en"
            content = ["base"]
            media = [{ file = "f", kind = "inno", size = 1, sha256 = "1111111111111111111111111111111111111111111111111111111111111111" }]
            [[binary]]
            name = "X.exe"
            role = "main"
            treatment = "lift"
            size = 1
            sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
            variants = ["a"]
            [binary.discover]
            seeds = [0x401000, 0x402000]
            no_return = [0x403000]
            data = [{ start = 0x404000, end = 0x404010 }, { start = 0x405000, end = 0x405000 }]
            jump_tables = [{ at = 0x406000, table = 0x406010, count = 0 }]
            [[binary]]
            name = "Y.dll"
            role = "library"
            treatment = "hle"
            size = 2
            base = 0x11000800
            sha256 = "2222222222222222222222222222222222222222222222222222222222222222"
            variants = ["a"]
        "#;
        let m = parse_str(text, Path::new("test")).unwrap();
        let h = m.binaries[0].discover.as_ref().unwrap();
        assert_eq!(h.seeds, [0x40_1000, 0x40_2000]);
        assert_eq!(h.no_return, [0x40_3000]);
        assert_eq!(
            h.data[0],
            Range {
                start: 0x40_4000,
                end: 0x40_4010
            }
        );
        assert_eq!(m.binaries[0].output_dir_name(), "x.exe-a");
        assert_eq!(m.binaries[1].base, Some(0x1100_0800));
        let Err(Error::Invalid(errs)) = validate(&[m]) else {
            panic!("expected validation errors")
        };
        assert!(errs.iter().any(|e| e.contains("is empty")));
        assert!(errs.iter().any(|e| e.contains("has count 0")));
        assert!(errs.iter().any(|e| e.contains("not lifted")));
        assert!(errs.iter().any(|e| e.contains("not 64 KiB aligned")));
        assert_eq!(errs.len(), 4, "{errs:?}");
    }

    #[test]
    fn a_lift_with_other_patches_is_stale() {
        let text = fs::read_to_string(repo_games().join("technomage/game.toml")).unwrap();
        let m = parse_str(&text, Path::new("game.toml")).unwrap();
        let main = m.binaries.iter().find(|b| b.role == Role::Main).unwrap();
        assert!(!main.patch.hooks.is_empty());
        let dir = std::env::temp_dir().join(format!("rc-stale-lifts-{}", std::process::id()));
        let lifted = dir.join(main.output_dir_name());
        fs::create_dir_all(&lifted).unwrap();
        fs::create_dir_all(dir.join("not-in-the-manifest")).unwrap();
        // Lifted before records were written: stale (the manifest patches this binary).
        assert_eq!(stale_lifts(&m, &dir), [main.output_dir_name()]);
        fs::write(lifted.join(LIFT_RECORD), main.lift_record()).unwrap();
        assert!(stale_lifts(&m, &dir).is_empty());
        let mut fewer = main.clone();
        fewer.patch.hooks.pop();
        fs::write(lifted.join(LIFT_RECORD), fewer.lift_record()).unwrap();
        assert_eq!(stale_lifts(&m, &dir), [main.output_dir_name()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
