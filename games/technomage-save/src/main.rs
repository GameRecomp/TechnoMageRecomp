// SPDX-License-Identifier: MIT
//! `tmsave`: TechnoMage's saved games on the command line (see `tmsave help`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use technomage_save::{check, json, names, picture, schema, Field, Names, Save, Value};

const HELP: &str = "\
tmsave - read, check and change TechnoMage's saved games (Saves/TMN_NN.TSG)

USAGE:
    tmsave [--exe EXE] COMMAND ...

COMMANDS:
    show SAVE [--all] [PREFIX]      the fields with their meaning; without --all only those
                                    not zero, without the picture's bytes
    get SAVE PATH...                values of fields (a PATH ending in '.' or a group name:
                                    every field below it)
    set SAVE PATH=VALUE... [-o OUT] change fields (in place without -o); VALUE: a number
                                    (0x.. hex), a name the field knows (an item, FLAG|FLAG,
                                    true/false), hex for byte fields
    export SAVE [-o OUT.json]       every field as JSON (lossless)
    import JSON -o OUT [--base SAVE]
                                    a saved game from JSON (without --base every field must be
                                    in it; with it, the JSON's fields replace the base's)
    picture SAVE OUT.ppm            the picture as a 160x120 PPM
    picture SAVE --set IN.ppm [-o OUT]
                                    a 160x120 PPM as the picture
    raw SAVE OFFSET [LEN]           bytes as hex (OFFSET and LEN decimal or 0x..)
    raw SAVE OFFSET=HEX [-o OUT]    write bytes (the fields they belong to are listed)
    diff SAVE SAVE                  the fields that differ
    check SAVE                      the game's rules a saved game must keep: errors (the game
                                    crashes or misreads; exit 1) and warnings
    schema [PREFIX]                 every field: offset, type, path, meaning (world.* once
                                    for each world's layout)
    help

SAVE is a file, or a slot number N for the user directory's Saves/TMN_NN.TSG
($RC_USER_DIR/technomagerecomp, else the platform's data directory). Item names come from
the game's own TechnoMage.exe (--exe, else $RC_GAME_DIR, else orig/technomage/cd-de);
without it items are shown by number.
";

/// `println!` that ends quietly when the reader went away (`tmsave show | head`).
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        if writeln!(std::io::stdout(), $($t)*).is_err() {
            std::process::exit(0);
        }
    }};
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tmsave: {e}");
            ExitCode::from(2)
        }
    }
}

/// Options taken out of the arguments: `--name VALUE` and bare `--flag`s.
struct Opts {
    rest: Vec<String>,
    exe: Option<PathBuf>,
    out: Option<PathBuf>,
    base: Option<String>,
    set: Option<PathBuf>,
    all: bool,
}

fn opts(args: Vec<String>) -> Result<Opts, String> {
    let mut o = Opts {
        rest: Vec::new(),
        exe: None,
        out: None,
        base: None,
        set: None,
        all: false,
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--exe" => o.exe = Some(value("--exe")?.into()),
            "-o" | "--out" => o.out = Some(value("-o")?.into()),
            "--base" => o.base = Some(value("--base")?),
            "--set" => o.set = Some(value("--set")?.into()),
            "--all" => o.all = true,
            _ => o.rest.push(a),
        }
    }
    Ok(o)
}

fn saves_dir() -> Option<PathBuf> {
    let base = std::env::var_os("RC_USER_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            if cfg!(windows) {
                std::env::var_os("APPDATA").map(PathBuf::from)
            } else {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
                    })
            }
        })?;
    Some(base.join("technomagerecomp").join("Saves"))
}

/// A file, or a slot number for the user directory's saved games.
fn save_path(arg: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(arg);
    if p.exists() {
        return Ok(p);
    }
    match arg.parse::<u32>() {
        Ok(n) if (1..=99).contains(&n) => {
            let dir = saves_dir().ok_or("no user directory (set RC_USER_DIR)")?;
            Ok(dir.join(format!("TMN_{n:02}.TSG")))
        }
        _ => Err(format!("{arg}: no such file")),
    }
}

fn load_names(exe: Option<&Path>) -> Option<Names> {
    let path = names::find_exe(exe)?;
    match names::load(&path) {
        Ok(n) => Some(n),
        Err(e) => {
            if exe.is_some() {
                eprintln!("tmsave: {e}");
            }
            None
        }
    }
}

/// `f`'s value as text: the number and, where the field knows one, its name.
fn show_value(s: &Save, f: &Field, names: Option<&Names>) -> String {
    if f.label == schema::Label::Text {
        if let Value::Bytes(b) = s.get(f) {
            let t: String = b
                .iter()
                .take_while(|&&x| x != 0)
                .map(|&x| char::from(x))
                .collect();
            return format!("{t:?}");
        }
    }
    match s.get(f) {
        Value::Int(n) => match f.label.show(n, names, Some(s)) {
            Some(l) => format!("{n} ({l})"),
            None => n.to_string(),
        },
        Value::Bytes(b) if b.len() > 32 => {
            let nonzero = b.iter().filter(|&&x| x != 0).count();
            format!(
                "[{} bytes, {nonzero} not zero] {}…",
                b.len(),
                technomage_save::hex(&b[..16])
            )
        }
        v => v.to_string(),
    }
}

/// The fields `sel` names: one path, or every field below a group (`player`, `items.`).
fn select<'a>(fields: &'a [Field], sel: &str, names: Option<&Names>) -> Vec<&'a Field> {
    let sel = schema::resolve(sel, names);
    if let Some(f) = fields.iter().find(|f| f.path == sel) {
        return vec![f];
    }
    let prefix = if sel.ends_with('.') || sel.ends_with('[') {
        sel.clone()
    } else {
        format!("{sel}.")
    };
    let alt = format!("{}[", sel.trim_end_matches('.'));
    fields
        .iter()
        .filter(|f| f.path.starts_with(&prefix) || f.path.starts_with(&alt))
        .collect()
}

fn write_out(s: &Save, out: Option<&Path>, input: &Path) -> Result<(), String> {
    let target = out.unwrap_or(input);
    s.write(target)?;
    eprintln!("tmsave: wrote {}", target.display());
    Ok(())
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let o = opts(args)?;
    let Some((cmd, rest)) = o.rest.split_first() else {
        out!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    };
    let need = |n: usize| -> Result<(), String> {
        if rest.len() < n {
            Err(format!("{cmd}: too few arguments (tmsave help)"))
        } else {
            Ok(())
        }
    };
    match cmd.as_str() {
        "help" | "--help" | "-h" => out!("{HELP}"),
        "schema" => {
            // The static fields, then each world's state layout.
            let prefix = rest.first().map(String::as_str).unwrap_or("");
            let mut all: Vec<Field> = schema::fields()
                .into_iter()
                .filter(|f| !f.path.starts_with("world."))
                .collect();
            for w in 1..=8 {
                all.extend(schema::world_fields(w));
            }
            for f in all.iter().filter(|f| f.path.starts_with(prefix)) {
                out!(
                    "{:#06x}  {:<8} {:<34} {}",
                    f.offset,
                    f.ty.name(),
                    f.path,
                    f.doc
                );
            }
        }
        "show" => {
            need(1)?;
            let path = save_path(&rest[0])?;
            let s = Save::read(&path)?;
            let names = load_names(o.exe.as_deref());
            let fields = all_fields(&s);
            let chosen: Vec<&Field> = match rest.get(1) {
                Some(sel) => select(&fields, sel, names.as_ref()),
                None => fields.iter().collect(),
            };
            for f in chosen {
                let v = s.get(f);
                // Without --all: what is not zero, the bitfields by their bits, no picture.
                if !o.all
                    && (schema::is_unused(f, &v)
                        || f.path == "picture"
                        || f.path.ends_with(".bits"))
                {
                    continue;
                }
                out!(
                    "{:#06x}  {:<34} {}",
                    f.offset,
                    schema::display_path(f, names.as_ref(), Some(&s)),
                    show_value(&s, f, names.as_ref())
                );
            }
        }
        "get" => {
            need(2)?;
            let s = Save::read(&save_path(&rest[0])?)?;
            let names = load_names(o.exe.as_deref());
            let fields = all_fields(&s);
            for sel in &rest[1..] {
                let chosen = select(&fields, sel, names.as_ref());
                if chosen.is_empty() {
                    return Err(format!("no field {sel} (tmsave schema)"));
                }
                for f in chosen {
                    if rest.len() == 2 && chosen_single(sel, f, names.as_ref()) {
                        out!("{}", show_value(&s, f, names.as_ref()));
                    } else {
                        out!(
                            "{} = {}",
                            schema::display_path(f, names.as_ref(), Some(&s)),
                            show_value(&s, f, names.as_ref())
                        );
                    }
                }
            }
        }
        "set" => {
            need(2)?;
            let path = save_path(&rest[0])?;
            let mut s = Save::read(&path)?;
            let names = load_names(o.exe.as_deref());
            let world = s.bytes()[schema::LOCATION_WORLD];
            for a in &rest[1..] {
                let (sel, text) = a
                    .split_once('=')
                    .ok_or_else(|| format!("{a}: PATH=VALUE wanted"))?;
                // Each change in turn: a new world changes the world state's layout.
                let fields = all_fields(&s);
                let chosen = select(&fields, sel.trim(), names.as_ref());
                let [f] = chosen.as_slice() else {
                    return Err(format!("{sel}: not one field (tmsave schema)"));
                };
                let v = s.parse(f, text.trim(), names.as_ref())?;
                s.set(f, &v)?;
            }
            write_out(&s, o.out.as_deref(), &path)?;
            if s.bytes()[schema::LOCATION_WORLD] != world {
                eprintln!(
                    "tmsave: note: the world changed; world.* and the level slots (levels.mask, slots[*], \
                     enemies, npcs) still hold the old world's state: set them for the new world \
                     (zero levels.mask and the world state to have it start fresh)"
                );
            }
            report_check(&s, names.as_ref());
        }
        "export" => {
            need(1)?;
            let s = Save::read(&save_path(&rest[0])?)?;
            let text = json::export(&s);
            match &o.out {
                Some(p) => std::fs::write(p, text).map_err(|e| format!("{}: {e}", p.display()))?,
                None => out!("{text}"),
            }
        }
        "import" => {
            need(1)?;
            let out = o.out.as_deref().ok_or("import needs -o OUT")?;
            let text =
                std::fs::read_to_string(&rest[0]).map_err(|e| format!("{}: {e}", rest[0]))?;
            let base = match &o.base {
                Some(b) => Some(Save::read(&save_path(b)?)?),
                None => None,
            };
            let s = json::import(&text, base)?;
            s.write(out)?;
            eprintln!("tmsave: wrote {}", out.display());
            report_check(&s, load_names(o.exe.as_deref()).as_ref());
        }
        "picture" => {
            need(1)?;
            let path = save_path(&rest[0])?;
            let mut s = Save::read(&path)?;
            let fields = schema::fields();
            let f = fields
                .iter()
                .find(|f| f.path == "picture")
                .expect("picture");
            if let Some(input) = &o.set {
                let data = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
                let px =
                    picture::from_ppm(&data).map_err(|e| format!("{}: {e}", input.display()))?;
                s.set(f, &Value::Bytes(px))?;
                write_out(&s, o.out.as_deref(), &path)?;
            } else {
                need(2)?;
                let Value::Bytes(px) = s.get(f) else {
                    unreachable!("the picture is bytes")
                };
                std::fs::write(&rest[1], picture::to_ppm(&px))
                    .map_err(|e| format!("{}: {e}", rest[1]))?;
            }
        }
        "raw" => {
            need(2)?;
            let path = save_path(&rest[0])?;
            let mut s = Save::read(&path)?;
            let num = |t: &str| {
                technomage_save::parse_int(t)
                    .filter(|&n| n >= 0)
                    .map(|n| n as usize)
                    .ok_or_else(|| format!("{t}: not an offset"))
            };
            if let Some((off, hexs)) = rest[1].split_once('=') {
                let off = num(off)?;
                let bytes = technomage_save::unhex(hexs)?;
                if off + bytes.len() > technomage_save::SIZE {
                    return Err("past the end of the saved game".into());
                }
                let mut b = s.bytes().to_vec();
                b[off..off + bytes.len()].copy_from_slice(&bytes);
                s = Save::from_bytes(b)?;
                for f in schema::fields_for(Some(&s))
                    .iter()
                    .filter(|f| f.offset < off + bytes.len() && off < f.offset + f.ty.size())
                {
                    eprintln!("tmsave: changes {}", f.path);
                }
                write_out(&s, o.out.as_deref(), &path)?;
            } else {
                let off = num(&rest[1])?;
                let len = rest.get(2).map(|t| num(t)).transpose()?.unwrap_or(16);
                let end = (off + len).min(technomage_save::SIZE);
                let b = s
                    .bytes()
                    .get(off..end)
                    .ok_or("past the end of the saved game")?;
                for (i, row) in b.chunks(16).enumerate() {
                    out!("{:#06x}  {}", off + i * 16, technomage_save::hex(row));
                }
            }
        }
        "diff" => {
            need(2)?;
            let a = Save::read(&save_path(&rest[0])?)?;
            let b = Save::read(&save_path(&rest[1])?)?;
            let names = load_names(o.exe.as_deref());
            let mut fields = schema::fields_for(Some(&a));
            if a.bytes()[schema::LOCATION_WORLD] != b.bytes()[schema::LOCATION_WORLD] {
                // Different worlds: their states compared as bytes.
                fields.retain(|f| !f.path.starts_with("world."));
                fields.extend(schema::world_fields(0));
            }
            fields.extend(
                schema::views(&a)
                    .into_iter()
                    .filter(|v| schema::views(&b).iter().any(|w| w.path == v.path)),
            );
            let d = technomage_save::diff(&fields, &a, &b);
            for (f, _, _) in &d {
                out!(
                    "{:#06x}  {:<34} {}  ->  {}",
                    f.offset,
                    schema::display_path(f, names.as_ref(), Some(&a)),
                    show_value(&a, f, names.as_ref()),
                    show_value(&b, f, names.as_ref())
                );
            }
            return Ok(if d.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            });
        }
        "check" => {
            need(1)?;
            let s = Save::read(&save_path(&rest[0])?)?;
            let problems = check::check(&s, load_names(o.exe.as_deref()).as_ref());
            for p in &problems {
                out!("{p}");
            }
            if problems.is_empty() {
                out!("ok");
            }
            if problems.iter().any(|p| p.error) {
                return Ok(ExitCode::from(1));
            }
        }
        _ => return Err(format!("unknown command {cmd} (tmsave help)")),
    }
    Ok(ExitCode::SUCCESS)
}

/// The fields of `s` and the bit views of its levels' bitfields.
fn all_fields(s: &Save) -> Vec<Field> {
    let mut f = schema::fields_for(Some(s));
    f.extend(schema::views(s));
    f
}

/// Whether `get` was asked for exactly this one field (then only its value is printed).
fn chosen_single(sel: &str, f: &Field, names: Option<&Names>) -> bool {
    schema::resolve(sel, names) == f.path
}

/// After a change: the rules the result breaks, as warnings (the file is written anyway, for
/// tests that want a broken one).
fn report_check(s: &Save, names: Option<&Names>) {
    for p in check::check(s, names) {
        eprintln!("tmsave: {p}");
    }
}
