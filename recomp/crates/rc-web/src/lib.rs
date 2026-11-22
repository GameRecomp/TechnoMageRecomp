// SPDX-License-Identifier: MIT
//! The converter for the browser, as in TropicoRecomp: rc-convert's pipeline on a GOG installer
//! the page reads through a callback, so that nothing large is held in memory and nothing leaves
//! the machine. The page runs it in a Web Worker (web/worker.js).
//!
//! - `convert`: a package for a shell template (zip, or a signed APK), handed out in pieces.
//! - Each takes the installer and, for an installer with its data in an external slice
//!   (`setup_….exe` with `-1.bin`), that file too (or null).
//! - `plan_web` and `convert_web`: for playing in the browser, the game's files and the
//!   WebAssembly module of its code, handed to callbacks that store them.
//! - `recompile_web`: the code again from a stored game, when the converter makes different code.

use std::io::{self, Read, Seek, SeekFrom, Write};

use js_sys::{Function, Reflect, Uint8Array};
use rc_convert::install::Install;
use rc_convert::lift::{Program, compile_native, compile_wasm, manifests};
use wasm_bindgen::prelude::*;

/// The installer, read through `read(offset, length) -> Uint8Array` in blocks of 4 MiB.
struct JsReader {
    read: Function,
    size: u64,
    pos: u64,
    block: Vec<u8>,
    block_at: u64,
}

const BLOCK: u64 = 4 << 20;

impl JsReader {
    fn new(size: f64, read: Function) -> JsReader {
        JsReader {
            read,
            size: size as u64,
            pos: 0,
            block: Vec::new(),
            block_at: 0,
        }
    }

    fn fill(&mut self, at: u64) -> io::Result<()> {
        let start = at - at % BLOCK;
        let len = BLOCK.min(self.size - start);
        let v = self
            .read
            .call2(
                &JsValue::NULL,
                &JsValue::from_f64(start as f64),
                &JsValue::from_f64(len as f64),
            )
            .map_err(|e| io::Error::other(format!("reading the installer: {e:?}")))?;
        let arr = Uint8Array::new(&v);
        if u64::from(arr.length()) != len {
            return Err(io::Error::other("reading the installer: short read"));
        }
        self.block.resize(len as usize, 0);
        arr.copy_to(&mut self.block);
        self.block_at = start;
        Ok(())
    }
}

impl Read for JsReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let end = self.block_at + self.block.len() as u64;
        if self.pos < self.block_at || self.pos >= end {
            self.fill(self.pos)?;
        }
        let off = (self.pos - self.block_at) as usize;
        let n = buf.len().min(self.block.len() - off);
        buf[..n].copy_from_slice(&self.block[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for JsReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::End(d) => i128::from(self.size) + i128::from(d),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
        };
        if p < 0 {
            return Err(io::Error::other("seek before the start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// The package, handed out through `output(Uint8Array)` in pieces of 8 MiB.
struct JsWriter {
    output: Function,
    buf: Vec<u8>,
}

impl JsWriter {
    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let arr = Uint8Array::new_with_length(self.buf.len() as u32);
        arr.copy_from(&self.buf);
        self.buf.clear();
        self.output
            .call1(&JsValue::NULL, &arr)
            .map_err(|e| io::Error::other(format!("writing the output: {e:?}")))?;
        Ok(())
    }
}

impl Write for JsWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= 8 << 20 {
            self.send()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

fn tell(progress: &Function, stage: &str, done: f64, total: f64) {
    let _ = progress.call3(
        &JsValue::NULL,
        &JsValue::from_str(stage),
        &JsValue::from_f64(done),
        &JsValue::from_f64(total),
    );
}

fn err(e: String) -> JsValue {
    JsValue::from_str(&e)
}

/// Read and identify the installer, then lift its programs (`identify`, `lift`).
/// The installer's external data slice (`setup_….exe`'s `-1.bin`), if the page has one:
/// `slice_read` is a function like `read`, or null.
fn slices(slice_size: f64, slice_read: &JsValue) -> Vec<Box<dyn rc_convert::rc_media::ReadSeek>> {
    match slice_read.dyn_ref::<Function>() {
        Some(f) => vec![Box::new(JsReader::new(slice_size, f.clone()))],
        None => Vec::new(),
    }
}

fn open(
    size: f64,
    read: Function,
    slice_size: f64,
    slice_read: &JsValue,
    progress: &Function,
) -> Result<(Install, Vec<Program>), JsValue> {
    let ms = manifests().map_err(err)?;
    tell(progress, "identify", 0.0, 1.0);
    let inst = Install::open_with(
        Box::new(JsReader::new(size, read)),
        slices(slice_size, slice_read),
        &ms,
    )
    .map_err(err)?;
    tell(progress, "identify", 1.0, 1.0);
    tell(progress, "lift", 0.0, 1.0);
    let progs = inst.programs(&ms).map_err(err)?;
    tell(progress, "lift", 1.0, 1.0);
    Ok((inst, progs))
}

fn functions(progs: &[Program]) -> usize {
    progs.iter().map(|p| p.functions.len()).sum()
}

/// Convert the installer of `size` bytes read by `read(offset, length)` into a package (an APK
/// for an Android template, signed with the converter's built-in key) for the shell template
/// `template`, handed out through `output(chunk)`. `progress(stage, done, total)` hears the
/// stages: `identify`, `lift`, `compile` (functions), `package` (bytes of game files). Returns a
/// line describing the result.
#[wasm_bindgen]
pub fn convert(
    size: f64,
    read: Function,
    slice_size: f64,
    slice_read: JsValue,
    template: Vec<u8>,
    progress: Function,
    output: Function,
) -> Result<String, JsValue> {
    let tpl = rc_convert::template(&template).map_err(err)?;
    let (mut inst, progs) = open(size, read, slice_size, &slice_read, &progress)?;
    let total = (functions(&progs) * tpl.targets.len()) as f64;
    let mut done = 0usize;
    let mut blobs = Vec::new();
    for p in &progs {
        for &t in &tpl.targets {
            let blob = compile_native(t, p, &mut |k| {
                done += k;
                if done.is_multiple_of(200) {
                    tell(&progress, "compile", done as f64, total);
                }
            })
            .map_err(err)?;
            blobs.push((t, p.exe.to_ascii_lowercase(), blob));
        }
    }
    tell(&progress, "compile", total, total);

    let (_, files) = inst.plan();
    let game: f64 = files.iter().map(|f| f.1 as f64).sum();
    let mut written = 0u64;
    let mut last = 0u64;
    let mut w = rc_convert::package(
        JsWriter {
            output,
            buf: Vec::with_capacity(9 << 20),
        },
        &tpl,
        &blobs,
        &mut |sink| inst.files(sink),
        None,
        &mut |k| {
            written += k;
            if written - last >= 4 << 20 {
                last = written;
                tell(&progress, "package", written as f64, game);
            }
        },
    )
    .map_err(err)?;
    w.flush().map_err(|e| err(e.to_string()))?;
    tell(&progress, "package", game, game);
    Ok(format!(
        "{} ({}): {} files, {} functions for {}",
        inst.manifest.game.title,
        inst.variant,
        files.len(),
        functions(&progs),
        tpl.targets
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(" and ")
    ))
}

/// A string as a JSON string literal.
fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The game in the installer of `size` bytes read by `read(offset, length)`, as JSON:
/// `{"id", "title", "variant", "dirs": [path...], "files": [[path, size]...]}` with paths under
/// `game/`. The page creates the files with it before `convert_web` fills them.
#[wasm_bindgen]
pub fn plan_web(
    size: f64,
    read: Function,
    slice_size: f64,
    slice_read: JsValue,
) -> Result<String, JsValue> {
    let ms = manifests().map_err(err)?;
    let inst = Install::open_with(
        Box::new(JsReader::new(size, read)),
        slices(slice_size, &slice_read),
        &ms,
    )
    .map_err(err)?;
    let (dirs, files) = inst.plan();
    let dirs: Vec<String> = dirs
        .iter()
        .map(|d| json_str(&format!("game/{d}")))
        .collect();
    let files: Vec<String> = files
        .iter()
        .map(|(p, n)| format!("[{},{n}]", json_str(&format!("game/{p}"))))
        .collect();
    Ok(format!(
        "{{\"id\":{},\"title\":{},\"variant\":{},\"dirs\":[{}],\"files\":[{}]}}",
        json_str(&inst.manifest.game.id),
        json_str(&inst.manifest.game.title),
        json_str(&inst.variant),
        dirs.join(","),
        files.join(",")
    ))
}

/// The game's files into `sink`, an object with `dir(path)`, `begin(path, size)`, `write(chunk)`
/// and `end()` (paths as `plan_web` lists them).
struct JsSink {
    sink: JsValue,
    dir: Function,
    begin: Function,
    write: Function,
    end: Function,
    progress: Function,
    written: u64,
    last: u64,
    total: f64,
}

fn method(o: &JsValue, name: &str) -> Result<Function, JsValue> {
    Reflect::get(o, &JsValue::from_str(name))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str(&format!("the sink has no {name}()")))
}

fn sink_err(e: JsValue) -> rc_convert::rc_media::Error {
    rc_convert::rc_media::Error::Io(io::Error::other(format!("storing the game: {e:?}")))
}

impl rc_convert::rc_media::Sink for JsSink {
    fn dir(&mut self, path: &str) -> rc_convert::rc_media::Result<()> {
        self.dir
            .call1(&self.sink, &JsValue::from_str(&format!("game/{path}")))
            .map(|_| ())
            .map_err(sink_err)
    }
    fn begin(&mut self, path: &str, size: u64) -> rc_convert::rc_media::Result<()> {
        self.begin
            .call2(
                &self.sink,
                &JsValue::from_str(&format!("game/{path}")),
                &JsValue::from_f64(size as f64),
            )
            .map(|_| ())
            .map_err(sink_err)
    }
    fn write(&mut self, data: &[u8]) -> rc_convert::rc_media::Result<()> {
        let arr = Uint8Array::new_with_length(data.len() as u32);
        arr.copy_from(data);
        self.write.call1(&self.sink, &arr).map_err(sink_err)?;
        self.written += data.len() as u64;
        if self.written - self.last >= 4 << 20 {
            self.last = self.written;
            tell(&self.progress, "package", self.written as f64, self.total);
        }
        Ok(())
    }
    fn end(&mut self) -> rc_convert::rc_media::Result<()> {
        self.end.call0(&self.sink).map(|_| ()).map_err(sink_err)
    }
}

/// The WebAssembly module of the main program (the web shell runs one program).
fn web_module(progs: &[Program], progress: &Function) -> Result<Vec<u8>, JsValue> {
    let p = progs
        .first()
        .ok_or_else(|| err("no program to compile".into()))?;
    let n = p.functions.len() as f64;
    let mut done = 0usize;
    let module = compile_wasm(p, &mut |k| {
        done += k;
        if done.is_multiple_of(200) {
            tell(progress, "compile", done as f64, n);
        }
    })
    .map_err(err)?;
    tell(progress, "compile", n, n);
    Ok(module)
}

/// Convert the installer for playing in the browser: the game's code as a WebAssembly module for
/// the web shell, handed to `sink.code(bytes)`, and the game's files to the sink's `dir`,
/// `begin`, `write` and `end` (see `plan_web`). `progress(stage, done, total)` hears `identify`,
/// `lift`, `compile` and `package`. Returns a line describing the result.
#[wasm_bindgen]
pub fn convert_web(
    size: f64,
    read: Function,
    slice_size: f64,
    slice_read: JsValue,
    progress: Function,
    sink: JsValue,
) -> Result<String, JsValue> {
    let code = method(&sink, "code")?;
    let (mut inst, progs) = open(size, read, slice_size, &slice_read, &progress)?;
    let module = web_module(&progs, &progress)?;
    let arr = Uint8Array::new_with_length(module.len() as u32);
    arr.copy_from(&module);
    code.call1(&sink, &arr)?;

    let (_, files) = inst.plan();
    let mut js = JsSink {
        dir: method(&sink, "dir")?,
        begin: method(&sink, "begin")?,
        write: method(&sink, "write")?,
        end: method(&sink, "end")?,
        sink,
        progress: progress.clone(),
        written: 0,
        last: 0,
        total: files.iter().map(|f| f.1 as f64).sum(),
    };
    inst.files(&mut js).map_err(err)?;
    tell(&progress, "package", js.total, js.total);
    Ok(format!(
        "{} ({}): {} files, {} functions as WebAssembly ({:.1} MB)",
        inst.manifest.game.title,
        inst.variant,
        files.len(),
        progs[0].functions.len(),
        module.len() as f64 / 1e6
    ))
}

/// The version of the game code the converter makes for the browser: when a stored game's
/// differs, its code is compiled again (`recompile_web`).
#[wasm_bindgen]
pub fn code_version() -> String {
    env!("RC_CODE_VERSION").to_string()
}

/// Compile the game's code for the browser again from its stored files: `file(name)` returns a
/// file of the game directory (any case) as a Uint8Array, or null. `progress(stage, done,
/// total)` hears `lift` and `compile`. Returns the WebAssembly module.
#[wasm_bindgen]
pub fn recompile_web(file: Function, progress: Function) -> Result<Vec<u8>, JsValue> {
    let ms = manifests().map_err(err)?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let v = file.call1(&JsValue::NULL, &JsValue::from_str(name)).ok()?;
        if v.is_null() || v.is_undefined() {
            return None;
        }
        Some(Uint8Array::new(&v).to_vec())
    };
    tell(&progress, "lift", 0.0, 1.0);
    let (game, variant, mut progs) = rc_convert::lift::lift_install(&ms, &mut read).map_err(err)?;
    let main = ms
        .iter()
        .find(|m| m.game.id == game)
        .and_then(|m| {
            m.binaries.iter().find(|b| {
                matches!(b.role, rc_convert::rc_manifest::Role::Main)
                    && b.variants.contains(&variant)
            })
        })
        .map(|b| b.name.to_ascii_lowercase());
    progs.sort_by_key(|p| Some(p.exe.to_ascii_lowercase()) != main);
    tell(&progress, "lift", 1.0, 1.0);
    web_module(&progs, &progress)
}

/// The version of the converter, for the page to show.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
