// SPDX-License-Identifier: MIT
//! `recomp`: the recompiler command line.
//!
//! Commands: `identify`, `info`, `list`, `extract`, `discover`, `codediff`, `lift`, `convert`.

mod convert;
mod discover;
mod download;
mod lift;
mod media;
mod native;
mod package;

use rc_manifest::{Catalog, Hit};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
recomp - static recompiler for Sacked (Gefeuert!) and Crazy School

USAGE:
    recomp identify [--games DIR] PATH...   match files (installers, ISO, binaries) by SHA-256
    recomp info [--games DIR] [--imports] FILE
                                            show PE facts and manifest matches for one file
    recomp list MEDIA                       list the files of an installer, CD image or zip
    recomp extract [--games DIR] [--dest DIR] [--force] MEDIA
                                            unpack MEDIA (default: orig/<game>/<variant>/) and
                                            verify the binaries against the manifest
    recomp extract [--games DIR] [--force] --game ID FOLDER
                                            unpack game ID from the first of its variants
                                            whose medium is in FOLDER (what the build does
                                            for a game that has no folder in orig/ yet)
    recomp discover [--games DIR] [--out DIR] [--base ADDR] [--no-write] [--no-relocs]
                    [--verbose] FILE...     find the code of an executable or DLL, print the
                                            coverage report and write the code map (default:
                                            generated/<game>/<binary>-<variant>/codemap.json).
                                            --no-relocs ignores a DLL's relocations, to check
                                            the heuristics against them
    recomp codediff [--games DIR] [--verbose] FILE_A FILE_B
                                            compare two builds function by function
    recomp lift [--games DIR] [--out DIR] [--set FILE] [--base ADDR] [--comments] [--verbose]
                FILE_OR_FOLDER...
                                            discover, lift every function and write C (default:
                                            generated/<game>/<binary>-<variant>/, plus the
                                            game's set.c); a folder: every binary in it the
                                            manifest lifts. Writes only files that changed;
                                            --set FILE also writes a set of just these
                                            binaries (one build target's); --comments puts
                                            each instruction above its C
    recomp convert [--games DIR] [--preset NAME] [--force] MEDIA_OR_FOLDER
                                            in one step: unpack (if needed), lift every binary
                                            the manifest lifts, build the game (CMake preset,
                                            default: release) into out/<game>/ with play.sh
    recomp convert --system SYSTEM [--templates DIR] [-o FILE] INSTALLER_OR_ZIP
    recomp convert --template TEMPLATE.zip -o FILE INSTALLER_OR_ZIP
                                            without a compiler, as the converter page: the
                                            game's code compiled to native code into its shell
                                            template for SYSTEM (windows, linux, macos,
                                            android; from DIR, by default templates/ beside
                                            recomp), with the game's files: <Game>-SYSTEM.zip
                                            (.apk for android) in the current folder
    recomp download [--games DIR] [--dest DIR] GAME LANGUAGE
                                            download the game's medium in that language
                                            (sacked: de, en, hu, pl, nl, ru; crazyschool: de,
                                            ru, pl) from archive.org into DIR (default: the
                                            current folder), checked by its SHA-256
    recomp convert --download GAME --language LANGUAGE [--dest DIR] ...
                                            the same, then convert it (with --system: the
                                            converter's way, else the build's)
    recomp help | --help
    recomp version | --version

The manifests are read from --games DIR, $RC_GAMES_DIR, or the nearest games/ directory.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

struct Opts {
    games: PathBuf,
    imports: bool,
    dest: Option<PathBuf>,
    game: Option<String>,
    force: bool,
    out: Option<PathBuf>,
    base: Option<u32>,
    write: bool,
    verbose: bool,
    no_relocs: bool,
    comments: bool,
    preset: Option<String>,
    set: Option<PathBuf>,
    native: Vec<String>,
    system: Option<String>,
    shell: Option<PathBuf>,
    template: Option<PathBuf>,
    templates: Option<PathBuf>,
    download: Option<String>,
    language: Option<String>,
    paths: Vec<PathBuf>,
}

fn parse_u32(s: &str) -> Result<u32, String> {
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| format!("bad address {s:?}"))
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        games: rc_manifest::default_games_dir(),
        imports: false,
        dest: None,
        game: None,
        force: false,
        out: None,
        base: None,
        write: true,
        verbose: false,
        no_relocs: false,
        comments: false,
        preset: None,
        set: None,
        native: Vec::new(),
        system: None,
        shell: None,
        template: None,
        templates: None,
        download: None,
        language: None,
        paths: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--games" => o.games = it.next().ok_or("--games needs a directory")?.into(),
            "--imports" => o.imports = true,
            "--dest" => o.dest = Some(it.next().ok_or("--dest needs a directory")?.into()),
            "--game" => o.game = Some(it.next().ok_or("--game needs a game id")?.clone()),
            "--force" => o.force = true,
            "--out" | "-o" => o.out = Some(it.next().ok_or("--out needs a directory")?.into()),
            "--base" => o.base = Some(parse_u32(it.next().ok_or("--base needs an address")?)?),
            "--no-write" => o.write = false,
            "--no-relocs" => o.no_relocs = true,
            "--verbose" | "-v" => o.verbose = true,
            "--comments" => o.comments = true,
            "--preset" => o.preset = Some(it.next().ok_or("--preset needs a name")?.clone()),
            "--set" => o.set = Some(it.next().ok_or("--set needs a file")?.into()),
            "--system" => o.system = Some(it.next().ok_or("--system needs a system")?.clone()),
            "--shell" => o.shell = Some(it.next().ok_or("--shell needs a file")?.into()),
            "--templates" => {
                o.templates = Some(it.next().ok_or("--templates needs a folder")?.into())
            }
            "--template" => o.template = Some(it.next().ok_or("--template needs a file")?.into()),
            "--download" => {
                o.download = Some(it.next().ok_or("--download needs a game id")?.clone())
            }
            "--language" => {
                o.language = Some(it.next().ok_or("--language needs a language")?.clone())
            }
            "--native" => {
                let t = it
                    .next()
                    .ok_or("--native needs targets (host, x86_64-sysv, ...)")?;
                o.native = t.split(',').map(str::to_string).collect();
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            p => o.paths.push(p.into()),
        }
    }
    Ok(o)
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some((cmd, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(ExitCode::from(2));
    };
    match cmd.as_str() {
        "identify" => identify(parse_opts(rest)?),
        "info" => info(parse_opts(rest)?),
        "list" => {
            let o = parse_opts(rest)?;
            media::list(&o.paths)
        }
        "extract" => {
            let o = parse_opts(rest)?;
            match &o.game {
                Some(id) => media::extract_game(&o.games, id, o.force, &o.paths),
                None => media::extract(&o.games, o.dest.as_deref(), o.force, &o.paths),
            }
        }
        "discover" => {
            let o = parse_opts(rest)?;
            discover::discover(
                &discover::DiscoverOpts {
                    games: &o.games,
                    out: o.out.as_deref(),
                    base: o.base,
                    write: o.write,
                    verbose: o.verbose,
                    no_relocs: o.no_relocs,
                },
                &o.paths,
            )
        }
        "lift" => {
            let o = parse_opts(rest)?;
            if !o.native.is_empty() {
                return native::lift_native(&o.games, &o.paths, &o.native, o.out.as_deref());
            }
            lift::lift(
                &lift::LiftOpts {
                    games: &o.games,
                    out: o.out.as_deref(),
                    set: o.set.as_deref(),
                    base: o.base,
                    comments: o.comments,
                    verbose: o.verbose,
                },
                &o.paths,
            )
        }
        "verify-apk" => {
            let o = parse_opts(rest)?;
            let [apk] = o.paths.as_slice() else {
                return Err("verify-apk needs one APK".into());
            };
            let mut f = std::fs::File::open(apk).map_err(|e| format!("{}: {e}", apk.display()))?;
            rc_convert::apk::verify(&mut f)?;
            println!("{}: the APK signature (v2) is valid", apk.display());
            Ok(ExitCode::SUCCESS)
        }
        "template" => {
            let o = parse_opts(rest)?;
            let game = o.game.as_deref().ok_or("template needs --game ID")?;
            let system = o
                .system
                .as_deref()
                .ok_or("template needs --system SYSTEM")?;
            let shell = o.shell.as_deref().ok_or("template needs --shell PROGRAM")?;
            let out = o.out.as_deref().ok_or("template needs --out FILE")?;
            package::template(game, system, shell, out)
        }
        "package" => {
            let o = parse_opts(rest)?;
            let tpl = o
                .template
                .as_deref()
                .ok_or("package needs --template FILE")?;
            let [install] = o.paths.as_slice() else {
                return Err("package needs one install folder".into());
            };
            package::package(&o.games, tpl, o.out.as_deref(), install)
        }
        "download" => {
            let o = parse_opts(rest)?;
            let [game, language] = o.paths.as_slice() else {
                return Err(
                    "download needs a game and a language (recomp download sacked en)".into(),
                );
            };
            let dest = o.dest.clone().unwrap_or_else(|| PathBuf::from("."));
            download::download(
                &o.games,
                &game.to_string_lossy(),
                &language.to_string_lossy(),
                &dest,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        "convert" => {
            let mut o = parse_opts(rest)?;
            if let Some(game) = o.download.clone() {
                let language = o.language.clone().ok_or("--download needs --language")?;
                let dest = o.dest.clone().unwrap_or_else(|| PathBuf::from("."));
                let path = download::download(&o.games, &game, &language, &dest)?;
                o.paths = vec![path];
            }
            if o.system.is_some() || o.template.is_some() {
                let [media] = o.paths.as_slice() else {
                    return Err("convert needs exactly one installer or zip".into());
                };
                return package::convert_package(
                    media,
                    o.system.as_deref(),
                    o.templates.as_deref(),
                    o.template.as_deref(),
                    o.out.as_deref(),
                );
            }
            convert::convert(
                &convert::ConvertOpts {
                    games: &o.games,
                    preset: o.preset.as_deref().unwrap_or("release"),
                    force: o.force,
                },
                &o.paths,
            )
        }
        "codediff" => {
            let o = parse_opts(rest)?;
            discover::codediff(&o.games, o.verbose, &o.paths)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "version" | "--version" | "-V" => {
            println!("recomp {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command {other:?} (try `recomp help`)")),
    }
}

fn load_catalog(games: &Path) -> Result<Catalog, String> {
    let ms = rc_manifest::load_dir(games).map_err(|e| e.to_string())?;
    Ok(Catalog::new(&ms))
}

/// Recursively collect regular files, sorted, without following symlinks.
fn walk(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_file() {
        out.push(path.to_owned());
    } else if meta.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort();
        for e in entries {
            walk(&e, out)?;
        }
    }
    Ok(())
}

fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Store and installer extras that differ per release and are never used by the games
/// (GOG Galaxy glue, GOG's Ogg/Vorbis CD-audio wrapper dependencies, uninstallers).
/// They are skipped instead of being reported as unknown. Prefixes, lower case.
const IGNORED_PREFIXES: &[&str] = &[
    "goggame-",
    "gameuxinstallhelper",
    "webcache.zip",
    "libogg-",
    "libvorbis",
    "unins",
];

fn is_ignored_extra(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|n| IGNORED_PREFIXES.iter().any(|pre| n.starts_with(pre)))
}

fn identify(o: Opts) -> Result<ExitCode, String> {
    if o.paths.is_empty() {
        return Err("identify needs at least one PATH".into());
    }
    let cat = load_catalog(&o.games)?;
    let mut files = Vec::new();
    for p in &o.paths {
        walk(p, &mut files)?;
    }

    let mut matched = 0usize;
    let mut unknown = 0usize;
    // game -> variant -> what was found for it
    let mut found: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for f in &files {
        let size = fs::metadata(f)
            .map_err(|e| format!("{}: {e}", f.display()))?
            .len();
        let candidate = has_ext(f, &["exe", "dll", "asi", "iso", "zip"]);
        if !cat.is_known_size(size) && !candidate {
            continue;
        }
        let hash = rc_manifest::sha256_file(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let hits = cat.lookup(&hash);
        if hits.is_empty() {
            if candidate && !is_ignored_extra(f) {
                unknown += 1;
                println!("unknown  {}  {}", &hash[..16], f.display());
            }
            continue;
        }
        matched += 1;
        for h in hits {
            println!("match    {h}  {}", f.display());
            match h {
                Hit::Media { game, variant, .. } => found
                    .entry(game.clone())
                    .or_default()
                    .entry(variant.clone())
                    .or_default()
                    .push("installer/media".into()),
                Hit::Binary {
                    game,
                    variants,
                    name,
                    role,
                    ..
                } => {
                    for v in variants {
                        found
                            .entry(game.clone())
                            .or_default()
                            .entry(v.clone())
                            .or_default()
                            .push(format!("{name} ({role})"));
                    }
                }
            }
        }
    }

    println!();
    println!(
        "{} files scanned, {matched} recognised, {unknown} unknown executables",
        files.len()
    );
    for (game, variants) in &found {
        for (variant, what) in variants {
            println!("  {game}/{variant}: {}", what.join(", "));
        }
    }
    Ok(if matched > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn fmt_timestamp(t: u32) -> String {
    let secs = i64::from(t);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn info(o: Opts) -> Result<ExitCode, String> {
    let [file] = o.paths.as_slice() else {
        return Err("info needs exactly one FILE".into());
    };
    let data = fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let hash = rc_manifest::sha256_hex(&data);
    println!("file        {}", file.display());
    println!("size        {}", data.len());
    println!("sha256      {hash}");
    match load_catalog(&o.games) {
        Ok(cat) => {
            let hits = cat.lookup(&hash);
            if hits.is_empty() {
                println!("manifest    not listed (unsupported build)");
            }
            for h in hits {
                println!("manifest    {h}");
            }
        }
        Err(e) => println!("manifest    unavailable: {e}"),
    }

    let pe = match rc_pe::parse(&data) {
        Ok(pe) => pe,
        Err(e) => {
            println!("pe          not a PE32 image: {e}");
            return Ok(ExitCode::SUCCESS);
        }
    };
    println!("timestamp   {}", fmt_timestamp(pe.timestamp));
    println!(
        "linker      {}.{:02}",
        pe.linker_version.0, pe.linker_version.1
    );
    println!(
        "kind        {}, subsystem {}",
        if pe.is_dll() { "dll" } else { "exe" },
        pe.subsystem
    );
    println!("image base  {:#010x}", pe.image_base);
    println!("entry       {:#010x}", pe.entry_va());
    println!(
        "relocations {}",
        if pe.relocs_stripped() {
            "none (fixed base)"
        } else {
            "present"
        }
    );
    match &pe.rich {
        Some(r) => {
            let ids: Vec<_> = r
                .iter()
                .map(|e| format!("{}.{}x{}", e.product, e.build, e.count))
                .collect();
            println!("rich        {}", ids.join(" "));
        }
        None => println!("rich        none"),
    }
    println!("sections");
    for s in &pe.sections {
        println!(
            "  {:8} va {:#010x} vsize {:#08x} raw {:#08x}{}",
            s.name,
            pe.image_base.wrapping_add(s.virtual_address),
            s.virtual_size,
            s.raw_size,
            if s.is_executable() { "  code" } else { "" }
        );
    }
    println!("imports     {} functions", pe.import_count());
    for d in &pe.imports {
        if o.imports {
            let names: Vec<_> = d.functions.iter().map(|f| f.name.to_string()).collect();
            println!("  {}: {}", d.dll, names.join(" "));
        } else {
            println!("  {:16} {}", d.dll, d.functions.len());
        }
    }
    if !pe.exports.is_empty() {
        println!("exports     {}", pe.exports.len());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_extras_are_ignored() {
        assert!(is_ignored_extra(Path::new("app/goggame-1207663793.dll")));
        assert!(is_ignored_extra(Path::new("GameuxInstallHelper.dll")));
        assert!(is_ignored_extra(Path::new("libvorbisfile-3.dll")));
        assert!(!is_ignored_extra(Path::new("Cultures.exe")));
        assert!(!is_ignored_extra(Path::new("winmm.dll")));
    }

    #[test]
    fn addresses() {
        assert_eq!(parse_u32("0x11000000"), Ok(0x1100_0000));
        assert_eq!(parse_u32("4096"), Ok(4096));
        assert!(parse_u32("0xzz").is_err());
    }

    #[test]
    fn timestamps() {
        assert_eq!(fmt_timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(fmt_timestamp(1_000_000_000), "2001-09-09 01:46:40 UTC");
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
