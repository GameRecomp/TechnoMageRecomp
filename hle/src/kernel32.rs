// SPDX-License-Identifier: MIT
//! kernel32: what TechnoMage's static C runtime and the game itself import (81 functions), and
//! what `binkw32.dll` needs. Files go through the runtime's VFS, memory through its heaps.

#![allow(non_snake_case)]

use crate::codepage;
use rc_runtime::cpu::Cpu;
use rc_runtime::handles::{self, Object, INVALID, STD_ERROR, STD_INPUT, STD_OUTPUT};
use rc_runtime::imports::{self, ret_stdcall};
use rc_runtime::mem::Mem;
use rc_runtime::{clock, process, vfs, vm};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

static LAST_ERROR: AtomicU32 = AtomicU32::new(0);

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_ENVVAR_NOT_FOUND: u32 = 203;

fn set_error(e: u32) {
    LAST_ERROR.store(e, Ordering::Relaxed);
}

fn arg(c: &Cpu, n: u32) -> u32 {
    c.arg(n)
}

fn guest_str(m: Mem, a: u32) -> String {
    if a == 0 {
        return String::new();
    }
    codepage::decode(&m.cstr(a, 4096))
}

/// Write `s` (ANSI) with a NUL into a buffer of `cap` bytes; returns the length without the
/// NUL, or the size needed (with the NUL) when it does not fit.
fn put_str(m: Mem, buf: u32, cap: u32, s: &str) -> u32 {
    let bytes: Vec<u8> = s
        .chars()
        .map(|ch| codepage::from_unicode(ch as u16).unwrap_or(b'?'))
        .collect();
    let n = bytes.len() as u32;
    if buf == 0 || cap <= n {
        return n + 1;
    }
    m.write(buf, &bytes);
    m.st8(buf + n, 0);
    n
}

/// Process strings the runtime keeps in guest memory (command line, module name, environment).
struct ProcStrings {
    cmdline: u32,
    env: u32,
    env_w: u32,
}

static PROC: Mutex<Option<ProcStrings>> = Mutex::new(None);
static IMAGE_BASE: AtomicU32 = AtomicU32::new(0x0040_0000);
const EXE_PATH: &str = "C:\\TECHNOMAGE\\TechnoMage.exe";

fn proc_strings(m: Mem) -> (u32, u32, u32) {
    let mut p = PROC.lock().expect("proc");
    if p.is_none() {
        let base = process::PROC_DATA;
        let cmd = format!("\"{EXE_PATH}\"");
        m.write(base, cmd.as_bytes());
        m.st8(base + cmd.len() as u32, 0);
        let env = base + 0x800;
        m.st16(env, 0); // an empty block: two NULs
        let env_w = base + 0x810;
        m.st32(env_w, 0);
        *p = Some(ProcStrings {
            cmdline: base,
            env,
            env_w,
        });
    }
    let p = p.as_ref().expect("proc");
    (p.cmdline, p.env, p.env_w)
}

/// The executable's base, for `GetModuleHandleA(NULL)`.
pub fn set_image_base(b: u32) {
    IMAGE_BASE.store(b, Ordering::Relaxed);
}

// --- modules -----------------------------------------------------------------------------

/// Pseudo handles of HLE DLLs loaded with `LoadLibraryA`.
const HLE_MODULE_BASE: u32 = 0x7e00_0000;
static LOADED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn dll_name(s: &str) -> String {
    let s = s
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(s)
        .to_ascii_lowercase();
    if s.contains('.') {
        s
    } else {
        format!("{s}.dll")
    }
}

/// DLLs the HLE layer provides as a whole.
const HLE_DLLS: &[&str] = &[
    "kernel32.dll",
    "user32.dll",
    "gdi32.dll",
    "ole32.dll",
    "winmm.dll",
    "ddraw.dll",
    "dinput.dll",
    "dsound.dll",
    "mss32.dll",
    "advapi32.dll",
];

fn LoadLibraryA(c: &mut Cpu) {
    let m = c.mem();
    let name = dll_name(&guest_str(m, arg(c, 0)));
    let h = if HLE_DLLS.contains(&name.as_str()) {
        let mut l = LOADED.lock().expect("modules");
        let i = l.iter().position(|x| *x == name).unwrap_or_else(|| {
            l.push(name.clone());
            l.len() - 1
        });
        HLE_MODULE_BASE + 0x10000 * i as u32
    } else if let Some(b) = crate::lifted_module_base(&name) {
        b
    } else {
        set_error(126);
        log::warn(&format!("LoadLibraryA({name}): not available"));
        0
    };
    ret_stdcall(c, h, 1);
}

fn module_name(h: u32) -> Option<String> {
    if h == 0 || h == IMAGE_BASE.load(Ordering::Relaxed) {
        return None;
    }
    let i = h.checked_sub(HLE_MODULE_BASE)? / 0x10000;
    LOADED.lock().expect("modules").get(i as usize).cloned()
}

fn GetProcAddress(c: &mut Cpu) {
    let m = c.mem();
    let (h, p) = (arg(c, 0), arg(c, 1));
    let name = if p < 0x10000 {
        format!("#{p}")
    } else {
        guest_str(m, p)
    };
    let addr = match module_name(h) {
        Some(dll) if imports::implemented(&dll, &name) => imports::address(&dll, &name),
        Some(dll) => {
            log::warn(&format!("GetProcAddress({dll}, {name}): not implemented"));
            0
        }
        None => crate::lifted_export(h, &name).unwrap_or(0),
    };
    if addr == 0 {
        set_error(127);
    }
    ret_stdcall(c, addr, 2);
}

fn FreeLibrary(c: &mut Cpu) {
    ret_stdcall(c, 1, 1);
}

fn GetModuleHandleA(c: &mut Cpu) {
    let m = c.mem();
    let p = arg(c, 0);
    let h = if p == 0 {
        IMAGE_BASE.load(Ordering::Relaxed)
    } else {
        let name = dll_name(&guest_str(m, p));
        if name.eq_ignore_ascii_case("technomage.exe") {
            IMAGE_BASE.load(Ordering::Relaxed)
        } else {
            let l = LOADED.lock().expect("modules");
            l.iter()
                .position(|x| *x == name)
                .map(|i| HLE_MODULE_BASE + 0x10000 * i as u32)
                .or_else(|| crate::lifted_module_base(&name))
                .unwrap_or(0)
        }
    };
    ret_stdcall(c, h, 1);
}

fn GetModuleFileNameA(c: &mut Cpu) {
    let m = c.mem();
    let (buf, cap) = (arg(c, 1), arg(c, 2));
    let n = put_str(m, buf, cap, EXE_PATH);
    ret_stdcall(c, n.min(cap), 3);
}

// --- process and environment -------------------------------------------------------------

fn GetCommandLineA(c: &mut Cpu) {
    let (cmd, _, _) = proc_strings(c.mem());
    ret_stdcall(c, cmd, 0);
}

fn GetEnvironmentStrings(c: &mut Cpu) {
    let (_, env, _) = proc_strings(c.mem());
    ret_stdcall(c, env, 0);
}

fn GetEnvironmentStringsW(c: &mut Cpu) {
    let (_, _, env) = proc_strings(c.mem());
    ret_stdcall(c, env, 0);
}

fn FreeEnvironmentStrings(c: &mut Cpu) {
    ret_stdcall(c, 1, 1);
}

fn GetEnvironmentVariableA(c: &mut Cpu) {
    set_error(ERROR_ENVVAR_NOT_FOUND);
    ret_stdcall(c, 0, 3);
}

fn SetEnvironmentVariableA(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn GetVersion(c: &mut Cpu) {
    // Windows XP (5.1, build 2600), as the sister projects answer.
    ret_stdcall(c, 0x0a28_0105, 0);
}

fn GetVersionExA(c: &mut Cpu) {
    let m = c.mem();
    let p = arg(c, 0);
    m.st32(p + 4, 5);
    m.st32(p + 8, 1);
    m.st32(p + 12, 2600);
    m.st32(p + 16, 2); // VER_PLATFORM_WIN32_NT
    m.write(p + 20, b"Service Pack 2\0");
    ret_stdcall(c, 1, 1);
}

fn GetStartupInfoA(c: &mut Cpu) {
    let m = c.mem();
    let p = arg(c, 0);
    for i in 0..68 {
        m.st8(p + i, 0);
    }
    m.st32(p, 68);
    ret_stdcall(c, 0, 1);
}

fn GetCurrentProcess(c: &mut Cpu) {
    ret_stdcall(c, 0xffff_ffff, 0);
}

fn GetCurrentThread(c: &mut Cpu) {
    ret_stdcall(c, 0xffff_fffe, 0);
}

fn SetPriorityClass(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn GetPriorityClass(c: &mut Cpu) {
    ret_stdcall(c, 0x20, 1); // NORMAL_PRIORITY_CLASS
}

fn SetThreadPriority(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn GetThreadPriority(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn ExitProcess(c: &mut Cpu) {
    let code = arg(c, 0);
    log::info(&format!("ExitProcess({code})"));
    std::process::exit(code as i32);
}

fn TerminateProcess(c: &mut Cpu) {
    let code = arg(c, 1);
    log::info(&format!("TerminateProcess({code})"));
    std::process::exit(code as i32);
}

fn SetErrorMode(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn GetLastError(c: &mut Cpu) {
    ret_stdcall(c, LAST_ERROR.load(Ordering::Relaxed), 0);
}

fn OutputDebugStringA(c: &mut Cpu) {
    let s = guest_str(c.mem(), arg(c, 0));
    log::debug(&format!("OutputDebugString: {}", s.trim_end()));
    ret_stdcall(c, 0, 1);
}

fn SetHandleCount(c: &mut Cpu) {
    let n = arg(c, 0);
    ret_stdcall(c, n, 1);
}

fn IsBadWritePtr(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn UnhandledExceptionFilter(c: &mut Cpu) {
    log::warn("UnhandledExceptionFilter: an exception reached the top");
    ret_stdcall(c, 1, 1); // EXCEPTION_EXECUTE_HANDLER
}

fn RaiseException(c: &mut Cpu) {
    let code = arg(c, 0);
    rc_runtime::dispatch::fatal(&format!(
        "RaiseException({code:#x}): structured exceptions are not dispatched yet (M3r)"
    ));
}

fn RtlUnwind(c: &mut Cpu) {
    let _ = c;
    rc_runtime::dispatch::fatal("RtlUnwind: SEH unwinding is not implemented yet (M3r)");
}

// --- time ----------------------------------------------------------------------------------

fn Sleep(c: &mut Cpu) {
    let ms = arg(c, 0);
    if clock::is_virtual() {
        clock::advance_ms(u64::from(ms));
    } else if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
    } else {
        std::thread::yield_now();
    }
    crate::yield_point(c);
    ret_stdcall(c, 0, 1);
}

const QPC_HZ: u64 = 1_000_000;

fn QueryPerformanceCounter(c: &mut Cpu) {
    let t = clock::ticks_ns() / (1_000_000_000 / QPC_HZ);
    let p = arg(c, 0);
    c.mem().st64(p, t);
    ret_stdcall(c, 1, 1);
}

fn QueryPerformanceFrequency(c: &mut Cpu) {
    let p = arg(c, 0);
    c.mem().st64(p, QPC_HZ);
    ret_stdcall(c, 1, 1);
}

fn GetTimeZoneInformation(c: &mut Cpu) {
    let m = c.mem();
    let p = arg(c, 0);
    for i in 0..172 {
        m.st8(p + i, 0);
    }
    ret_stdcall(c, 0, 1); // TIME_ZONE_ID_UNKNOWN
}

fn FileTimeToLocalFileTime(c: &mut Cpu) {
    let m = c.mem();
    let v = m.ld64(arg(c, 0));
    m.st64(arg(c, 1), v);
    ret_stdcall(c, 1, 2);
}

fn FileTimeToSystemTime(c: &mut Cpu) {
    let m = c.mem();
    let ft = m.ld64(arg(c, 0));
    let out = arg(c, 1);
    let ms_total = ft / 10_000;
    let days = ms_total / 86_400_000;
    let ms_day = ms_total % 86_400_000;
    // Days since 1601-01-01 to a civil date.
    let (y, mo, d) = civil_from_days(days as i64 - 134_774); // 1601 → 1970
    let dow = ((days + 1) % 7) as u16; // 1601-01-01 was a Monday
    let vals: [u16; 8] = [
        y as u16,
        mo as u16,
        dow,
        d as u16,
        (ms_day / 3_600_000) as u16,
        (ms_day / 60_000 % 60) as u16,
        (ms_day / 1000 % 60) as u16,
        (ms_day % 1000) as u16,
    ];
    for (i, v) in vals.iter().enumerate() {
        m.st16(out + 2 * i as u32, *v);
    }
    ret_stdcall(c, 1, 2);
}

/// Days since 1970-01-01 to (year, month, day) (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A host time as a FILETIME.
fn filetime(t: std::time::SystemTime) -> u64 {
    let s = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() / 100)
        .unwrap_or(0);
    s as u64 + 116_444_736_000_000_000
}

// --- memory --------------------------------------------------------------------------------

const HEAP_ZERO_MEMORY: u32 = 0x8;
static NEXT_HEAP: AtomicU32 = AtomicU32::new(0x0015_0000);

fn HeapCreate(c: &mut Cpu) {
    let h = NEXT_HEAP.fetch_add(0x10, Ordering::Relaxed);
    ret_stdcall(c, h, 3);
}

fn HeapDestroy(c: &mut Cpu) {
    ret_stdcall(c, 1, 1);
}

fn HeapAlloc(c: &mut Cpu) {
    let (flags, size) = (arg(c, 1), arg(c, 2));
    let a = vm::heap_alloc(c.mem(), size, flags & HEAP_ZERO_MEMORY != 0);
    ret_stdcall(c, a, 3);
}

fn HeapReAlloc(c: &mut Cpu) {
    let (flags, p, size) = (arg(c, 1), arg(c, 2), arg(c, 3));
    let a = vm::heap_realloc(c.mem(), p, size, flags & HEAP_ZERO_MEMORY != 0);
    ret_stdcall(c, a, 4);
}

fn HeapFree(c: &mut Cpu) {
    let ok = vm::heap_free(arg(c, 2));
    ret_stdcall(c, u32::from(ok), 3);
}

fn HeapSize(c: &mut Cpu) {
    let n = vm::heap_size(arg(c, 2)).unwrap_or(0xffff_ffff);
    ret_stdcall(c, n, 3);
}

const MEM_RESERVE: u32 = 0x2000;

fn VirtualAlloc(c: &mut Cpu) {
    let (want, size, kind) = (arg(c, 0), arg(c, 1), arg(c, 2));
    let a = vm::region_alloc(want, size);
    if a != 0 && kind & MEM_RESERVE != 0 {
        let m = c.mem();
        let n = size.div_ceil(vm::PAGE) * vm::PAGE;
        for i in (0..n).step_by(4) {
            m.st32(a + i, 0);
        }
    }
    ret_stdcall(c, a, 4);
}

fn VirtualFree(c: &mut Cpu) {
    let (a, _size, kind) = (arg(c, 0), arg(c, 1), arg(c, 2));
    let ok = if kind & 0x8000 != 0 {
        vm::region_free(a)
    } else {
        true // MEM_DECOMMIT: the pages stay
    };
    ret_stdcall(c, u32::from(ok), 3);
}

// --- strings and code pages ----------------------------------------------------------------

fn GetACP(c: &mut Cpu) {
    ret_stdcall(c, 1252, 0);
}

fn GetOEMCP(c: &mut Cpu) {
    ret_stdcall(c, 850, 0);
}

fn GetCPInfo(c: &mut Cpu) {
    let m = c.mem();
    let p = arg(c, 1);
    m.st32(p, 1);
    m.st8(p + 4, b'?');
    m.st8(p + 5, 0);
    for i in 0..12 {
        m.st8(p + 6 + i, 0);
    }
    ret_stdcall(c, 1, 2);
}

fn MultiByteToWideChar(c: &mut Cpu) {
    let m = c.mem();
    let (src, n, dst, cap) = (arg(c, 2), arg(c, 3) as i32, arg(c, 4), arg(c, 5));
    let bytes = if n < 0 {
        let mut v = m.cstr(src, 1 << 20);
        v.push(0);
        v
    } else {
        let mut v = vec![0u8; n as usize];
        m.read(src, &mut v);
        v
    };
    let count = bytes.len() as u32;
    if cap == 0 {
        return ret_stdcall(c, count, 6);
    }
    if cap < count {
        set_error(122);
        return ret_stdcall(c, 0, 6);
    }
    for (i, &b) in bytes.iter().enumerate() {
        m.st16(dst + 2 * i as u32, codepage::to_unicode(b));
    }
    ret_stdcall(c, count, 6);
}

fn WideCharToMultiByte(c: &mut Cpu) {
    let m = c.mem();
    let (src, n, dst, cap) = (arg(c, 2), arg(c, 3) as i32, arg(c, 4), arg(c, 5));
    let mut units = Vec::new();
    if n < 0 {
        let mut i = 0;
        loop {
            let u = m.ld16(src + 2 * i);
            units.push(u);
            if u == 0 {
                break;
            }
            i += 1;
        }
    } else {
        for i in 0..n as u32 {
            units.push(m.ld16(src + 2 * i));
        }
    }
    let count = units.len() as u32;
    if cap == 0 {
        return ret_stdcall(c, count, 8);
    }
    if cap < count {
        set_error(122);
        return ret_stdcall(c, 0, 8);
    }
    for (i, &u) in units.iter().enumerate() {
        m.st8(dst + i as u32, codepage::from_unicode(u).unwrap_or(b'?'));
    }
    ret_stdcall(c, count, 8);
}

/// Character type bits (`CT_CTYPE1`) of a Unicode character.
fn ctype1(u: u16) -> u16 {
    let ch = char::from_u32(u32::from(u)).unwrap_or('\0');
    let mut t = 0;
    if ch.is_uppercase() {
        t |= 0x001;
    }
    if ch.is_lowercase() {
        t |= 0x002;
    }
    if ch.is_ascii_digit() {
        t |= 0x004;
    }
    if ch.is_whitespace() {
        t |= 0x008;
    }
    if ch.is_ascii_punctuation() {
        t |= 0x010;
    }
    if ch.is_control() {
        t |= 0x020;
    }
    if ch == ' ' || ch == '\t' {
        t |= 0x040;
    }
    if ch.is_ascii_hexdigit() {
        t |= 0x080;
    }
    if ch.is_alphabetic() {
        t |= 0x100;
    }
    t
}

fn GetStringTypeA(c: &mut Cpu) {
    let m = c.mem();
    let (src, n, out) = (arg(c, 2), arg(c, 3) as i32, arg(c, 4));
    let len = if n < 0 {
        m.cstr(src, 1 << 20).len() as u32 + 1
    } else {
        n as u32
    };
    for i in 0..len {
        m.st16(out + 2 * i, ctype1(codepage::to_unicode(m.ld8(src + i))));
    }
    ret_stdcall(c, 1, 5);
}

fn GetStringTypeW(c: &mut Cpu) {
    let m = c.mem();
    let (src, n, out) = (arg(c, 1), arg(c, 2) as i32, arg(c, 3));
    let mut i = 0;
    loop {
        if n >= 0 && i >= n as u32 {
            break;
        }
        let u = m.ld16(src + 2 * i);
        m.st16(out + 2 * i, ctype1(u));
        i += 1;
        if n < 0 && u == 0 {
            break;
        }
    }
    ret_stdcall(c, 1, 4);
}

const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;

fn map_case(u: u16, flags: u32) -> u16 {
    let ch = char::from_u32(u32::from(u)).unwrap_or('\0');
    let r = if flags & LCMAP_UPPERCASE != 0 {
        ch.to_uppercase().next().unwrap_or(ch)
    } else if flags & LCMAP_LOWERCASE != 0 {
        ch.to_lowercase().next().unwrap_or(ch)
    } else {
        ch
    };
    u16::try_from(u32::from(r)).unwrap_or(u)
}

fn LCMapStringA(c: &mut Cpu) {
    let m = c.mem();
    let (flags, src, n, dst, cap) = (arg(c, 1), arg(c, 2), arg(c, 3) as i32, arg(c, 4), arg(c, 5));
    let len = if n < 0 {
        m.cstr(src, 1 << 20).len() as u32 + 1
    } else {
        n as u32
    };
    if cap == 0 {
        return ret_stdcall(c, len, 6);
    }
    let len = len.min(cap);
    for i in 0..len {
        let u = map_case(codepage::to_unicode(m.ld8(src + i)), flags);
        m.st8(dst + i, codepage::from_unicode(u).unwrap_or(b'?'));
    }
    ret_stdcall(c, len, 6);
}

fn LCMapStringW(c: &mut Cpu) {
    let m = c.mem();
    let (flags, src, n, dst, cap) = (arg(c, 1), arg(c, 2), arg(c, 3) as i32, arg(c, 4), arg(c, 5));
    let mut len = 0u32;
    if n < 0 {
        while m.ld16(src + 2 * len) != 0 {
            len += 1;
        }
        len += 1;
    } else {
        len = n as u32;
    }
    if cap == 0 {
        return ret_stdcall(c, len, 6);
    }
    let len = len.min(cap);
    for i in 0..len {
        m.st16(dst + 2 * i, map_case(m.ld16(src + 2 * i), flags));
    }
    ret_stdcall(c, len, 6);
}

fn compare(a: &[u16], b: &[u16], ignore_case: bool) -> u32 {
    let f = |u: u16| {
        if ignore_case {
            map_case(u, LCMAP_LOWERCASE)
        } else {
            u
        }
    };
    let a: Vec<u16> = a.iter().map(|&u| f(u)).collect();
    let b: Vec<u16> = b.iter().map(|&u| f(u)).collect();
    match a.cmp(&b) {
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => 2,
        std::cmp::Ordering::Greater => 3,
    }
}

fn CompareStringA(c: &mut Cpu) {
    let m = c.mem();
    let flags = arg(c, 1);
    let get = |p: u32, n: i32| -> Vec<u16> {
        let v = if n < 0 {
            m.cstr(p, 1 << 20)
        } else {
            let mut v = vec![0u8; n as usize];
            m.read(p, &mut v);
            v
        };
        v.iter().map(|&b| codepage::to_unicode(b)).collect()
    };
    let a = get(arg(c, 2), arg(c, 3) as i32);
    let b = get(arg(c, 4), arg(c, 5) as i32);
    let r = compare(&a, &b, flags & 1 != 0);
    ret_stdcall(c, r, 6);
}

fn CompareStringW(c: &mut Cpu) {
    let m = c.mem();
    let flags = arg(c, 1);
    let get = |p: u32, n: i32| -> Vec<u16> {
        let mut v = Vec::new();
        let mut i = 0;
        loop {
            if n >= 0 && i >= n as u32 {
                break;
            }
            let u = m.ld16(p + 2 * i);
            if n < 0 && u == 0 {
                break;
            }
            v.push(u);
            i += 1;
        }
        v
    };
    let a = get(arg(c, 2), arg(c, 3) as i32);
    let b = get(arg(c, 4), arg(c, 5) as i32);
    let r = compare(&a, &b, flags & 1 != 0);
    ret_stdcall(c, r, 6);
}

// --- files ---------------------------------------------------------------------------------

const GENERIC_WRITE: u32 = 0x4000_0000;

fn CreateFileA(c: &mut Cpu) {
    let m = c.mem();
    let name = guest_str(m, arg(c, 0));
    let (access, disposition) = (arg(c, 1), arg(c, 4));
    let write = access & GENERIC_WRITE != 0 || matches!(disposition, 1 | 2 | 5);
    let existing = vfs::host_path(&name);
    let result = if !write {
        match &existing {
            Some(p) if p.is_file() => std::fs::File::open(p).map(|f| (f, p.clone())),
            _ => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        }
    } else {
        match vfs::host_path_for_write(&name) {
            None => Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            Some(target) => {
                // A file of the install opened for update is copied to the user directory first.
                if !target.exists() && matches!(disposition, 3 | 4) {
                    if let Some(src) = existing.as_ref().filter(|p| **p != target) {
                        let _ = std::fs::copy(src, &target);
                    }
                }
                let mut o = std::fs::OpenOptions::new();
                o.read(true).write(true);
                match disposition {
                    1 => o.create_new(true),
                    2 => o.create(true).truncate(true),
                    3 => &mut o,
                    4 => o.create(true),
                    _ => o.truncate(true),
                };
                o.open(&target).map(|f| (f, target.clone()))
            }
        }
    };
    match result {
        Ok((file, path)) => {
            let h = handles::insert(Object::File { file, path });
            set_error(0);
            ret_stdcall(c, h, 7);
        }
        Err(e) => {
            let code = match e.kind() {
                std::io::ErrorKind::NotFound => ERROR_FILE_NOT_FOUND,
                std::io::ErrorKind::AlreadyExists => ERROR_FILE_EXISTS,
                std::io::ErrorKind::PermissionDenied => ERROR_ACCESS_DENIED,
                _ => ERROR_PATH_NOT_FOUND,
            };
            log::trace_files(&format!("CreateFileA({name}): {e}"));
            set_error(code);
            ret_stdcall(c, INVALID, 7);
        }
    }
}

fn ReadFile(c: &mut Cpu) {
    let m = c.mem();
    let (h, buf, n, done) = (arg(c, 0), arg(c, 1), arg(c, 2), arg(c, 3));
    let mut data = vec![0u8; n as usize];
    let got = handles::with(h, |o| match o {
        Object::File { file, .. } => {
            let mut total = 0;
            while total < data.len() {
                match file.read(&mut data[total..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => total += k,
                }
            }
            Some(total)
        }
        _ => None,
    })
    .flatten();
    match got {
        Some(k) => {
            m.write(buf, &data[..k]);
            if done != 0 {
                m.st32(done, k as u32);
            }
            ret_stdcall(c, 1, 5);
        }
        None => {
            set_error(ERROR_INVALID_HANDLE);
            ret_stdcall(c, 0, 5);
        }
    }
}

fn WriteFile(c: &mut Cpu) {
    let m = c.mem();
    let (h, buf, n, done) = (arg(c, 0), arg(c, 1), arg(c, 2), arg(c, 3));
    let mut data = vec![0u8; n as usize];
    m.read(buf, &mut data);
    let ok = if matches!(h, STD_OUTPUT | STD_ERROR) {
        let _ = std::io::stderr().write_all(&data);
        true
    } else {
        handles::with(h, |o| match o {
            Object::File { file, .. } => file.write_all(&data).is_ok(),
            _ => false,
        })
        .unwrap_or(false)
    };
    if done != 0 {
        m.st32(done, if ok { n } else { 0 });
    }
    if !ok {
        set_error(ERROR_INVALID_HANDLE);
    }
    ret_stdcall(c, u32::from(ok), 5);
}

fn SetFilePointer(c: &mut Cpu) {
    let m = c.mem();
    let (h, lo, hi_p, method) = (arg(c, 0), arg(c, 1), arg(c, 2), arg(c, 3));
    let dist = if hi_p != 0 {
        ((u64::from(m.ld32(hi_p)) << 32) | u64::from(lo)) as i64
    } else {
        i64::from(lo as i32)
    };
    let pos = handles::with(h, |o| match o {
        Object::File { file, .. } => {
            let from = match method {
                0 => SeekFrom::Start(dist.max(0) as u64),
                1 => SeekFrom::Current(dist),
                _ => SeekFrom::End(dist),
            };
            file.seek(from).ok()
        }
        _ => None,
    })
    .flatten();
    match pos {
        Some(p) => {
            if hi_p != 0 {
                m.st32(hi_p, (p >> 32) as u32);
            }
            ret_stdcall(c, p as u32, 4);
        }
        None => {
            set_error(ERROR_INVALID_HANDLE);
            ret_stdcall(c, 0xffff_ffff, 4);
        }
    }
}

fn SetEndOfFile(c: &mut Cpu) {
    let h = arg(c, 0);
    let ok = handles::with(h, |o| match o {
        Object::File { file, .. } => file.stream_position().and_then(|p| file.set_len(p)).is_ok(),
        _ => false,
    })
    .unwrap_or(false);
    ret_stdcall(c, u32::from(ok), 1);
}

fn FlushFileBuffers(c: &mut Cpu) {
    let h = arg(c, 0);
    let _ = handles::with(h, |o| {
        if let Object::File { file, .. } = o {
            let _ = file.flush();
        }
    });
    ret_stdcall(c, 1, 1);
}

fn GetFileType(c: &mut Cpu) {
    let h = arg(c, 0);
    let t = match h {
        STD_INPUT | STD_OUTPUT | STD_ERROR => 2, // FILE_TYPE_CHAR
        _ if handles::exists(h) => 1,            // FILE_TYPE_DISK
        _ => 0,
    };
    ret_stdcall(c, t, 1);
}

fn GetStdHandle(c: &mut Cpu) {
    let h = match arg(c, 0) as i32 {
        -10 => STD_INPUT,
        -11 => STD_OUTPUT,
        -12 => STD_ERROR,
        _ => INVALID,
    };
    ret_stdcall(c, h, 1);
}

fn SetStdHandle(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn CloseHandle(c: &mut Cpu) {
    let h = arg(c, 0);
    let ok = handles::close(h) || matches!(h, STD_INPUT | STD_OUTPUT | STD_ERROR);
    ret_stdcall(c, u32::from(ok), 1);
}

fn DeleteFileA(c: &mut Cpu) {
    let name = guest_str(c.mem(), arg(c, 0));
    let ok = vfs::host_path_for_write(&name).is_some_and(|p| std::fs::remove_file(p).is_ok());
    if !ok {
        set_error(ERROR_FILE_NOT_FOUND);
    }
    ret_stdcall(c, u32::from(ok), 1);
}

fn CreateDirectoryA(c: &mut Cpu) {
    let name = guest_str(c.mem(), arg(c, 0));
    let ok = vfs::host_path_for_write(&name).is_some_and(|p| {
        if p.is_dir() {
            false
        } else {
            std::fs::create_dir_all(p).is_ok()
        }
    });
    if !ok {
        set_error(183); // ERROR_ALREADY_EXISTS
    }
    ret_stdcall(c, u32::from(ok), 2);
}

fn SetFileAttributesA(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn GetCurrentDirectoryA(c: &mut Cpu) {
    let (cap, buf) = (arg(c, 0), arg(c, 1));
    let n = put_str(c.mem(), buf, cap, &vfs::get().cwd);
    ret_stdcall(c, n, 2);
}

fn SetCurrentDirectoryA(c: &mut Cpu) {
    let name = guest_str(c.mem(), arg(c, 0));
    let ok = vfs::set_cwd(&name);
    if !ok {
        set_error(ERROR_PATH_NOT_FOUND);
    }
    ret_stdcall(c, u32::from(ok), 1);
}

fn GetFullPathNameA(c: &mut Cpu) {
    let m = c.mem();
    let (name, cap, buf, part) = (arg(c, 0), arg(c, 1), arg(c, 2), arg(c, 3));
    let full = vfs::absolute(&guest_str(m, name));
    let n = put_str(m, buf, cap, &full);
    if part != 0 && n < cap {
        let slash = full.rfind('\\').map_or(0, |i| i + 1) as u32;
        m.st32(part, buf + slash);
    }
    ret_stdcall(c, n, 4);
}

fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) && go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    // "*.*" matches names without a dot too.
    go(&p, &n) || (pattern == "*.*")
}

fn fill_find_data(m: Mem, out: u32, path: &std::path::Path) {
    let meta = std::fs::metadata(path).ok();
    for i in 0..320 {
        m.st8(out + i, 0);
    }
    let is_dir = meta.as_ref().is_some_and(|x| x.is_dir());
    m.st32(out, if is_dir { 0x10 } else { 0x20 });
    if let Some(md) = &meta {
        let t = md.modified().map(filetime).unwrap_or(0);
        for off in [4, 12, 20] {
            m.st64(out + off, t);
        }
        m.st32(out + 28, (md.len() >> 32) as u32);
        m.st32(out + 32, md.len() as u32);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    put_str(m, out + 44, 260, &name);
}

fn FindFirstFileA(c: &mut Cpu) {
    let m = c.mem();
    let pattern = vfs::absolute(&guest_str(m, arg(c, 0)));
    let out = arg(c, 1);
    let (dir, mask) = match pattern.rfind('\\') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => ("", pattern.as_str()),
    };
    let dir = if dir.len() == 2 {
        format!("{dir}\\")
    } else {
        dir.to_string()
    };
    let mut found: Vec<std::path::PathBuf> = vfs::host_path(&dir)
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| wildcard(mask, &e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    if found.is_empty() {
        set_error(ERROR_FILE_NOT_FOUND);
        return ret_stdcall(c, INVALID, 2);
    }
    let first = found.remove(0);
    fill_find_data(m, out, &first);
    found.reverse();
    let h = handles::insert(Object::Find(found));
    ret_stdcall(c, h, 2);
}

fn FindNextFileA(c: &mut Cpu) {
    let m = c.mem();
    let (h, out) = (arg(c, 0), arg(c, 1));
    let next = handles::with(h, |o| match o {
        Object::Find(v) => v.pop(),
        _ => None,
    })
    .flatten();
    match next {
        Some(p) => {
            fill_find_data(m, out, &p);
            ret_stdcall(c, 1, 2);
        }
        None => {
            set_error(ERROR_NO_MORE_FILES);
            ret_stdcall(c, 0, 2);
        }
    }
}

fn FindClose(c: &mut Cpu) {
    let ok = handles::close(arg(c, 0));
    ret_stdcall(c, u32::from(ok), 1);
}

// --- drives --------------------------------------------------------------------------------

fn GetLogicalDrives(c: &mut Cpu) {
    ret_stdcall(c, vfs::logical_drives(), 0);
}

fn GetDriveTypeA(c: &mut Cpu) {
    let root = guest_str(c.mem(), arg(c, 0));
    let root = if root.is_empty() {
        vfs::get().cwd
    } else {
        root
    };
    ret_stdcall(c, vfs::drive_type(&root), 1);
}

fn GetVolumeInformationA(c: &mut Cpu) {
    let m = c.mem();
    let root = guest_str(m, arg(c, 0));
    let (name, name_cap, serial, maxlen, flags, fs, fs_cap) = (
        arg(c, 1),
        arg(c, 2),
        arg(c, 3),
        arg(c, 4),
        arg(c, 5),
        arg(c, 6),
        arg(c, 7),
    );
    let t = vfs::drive_type(&root);
    if t == 1 {
        set_error(ERROR_PATH_NOT_FOUND);
        return ret_stdcall(c, 0, 8);
    }
    let (label, fsname) = if t == 5 {
        (vfs::get().cd_label, "CDFS")
    } else {
        ("SYSTEM".to_string(), "NTFS")
    };
    if name != 0 {
        put_str(m, name, name_cap, &label);
    }
    if serial != 0 {
        m.st32(serial, if t == 5 { 0x2004_0301 } else { 0x1234_5678 });
    }
    if maxlen != 0 {
        m.st32(maxlen, 255);
    }
    if flags != 0 {
        m.st32(flags, if t == 5 { 0x0008_0000 } else { 0x0000_0007 });
    }
    if fs != 0 {
        put_str(m, fs, fs_cap, fsname);
    }
    ret_stdcall(c, 1, 8);
}

// --- synchronisation -----------------------------------------------------------------------

fn CreateMutexA(c: &mut Cpu) {
    let owned = arg(c, 1);
    let h = handles::insert(Object::Mutex {
        owned: u32::from(owned != 0),
    });
    ret_stdcall(c, h, 3);
}

fn CreateSemaphoreA(c: &mut Cpu) {
    let (init, max) = (arg(c, 1) as i32, arg(c, 2) as i32);
    let h = handles::insert(Object::Semaphore { count: init, max });
    ret_stdcall(c, h, 4);
}

fn ReleaseSemaphore(c: &mut Cpu) {
    let m = c.mem();
    let (h, n, prev) = (arg(c, 0), arg(c, 1) as i32, arg(c, 2));
    let old = handles::with(h, |o| match o {
        Object::Semaphore { count, max } => {
            let old = *count;
            *count = (*count + n).min(*max);
            Some(old)
        }
        _ => None,
    })
    .flatten();
    if let (Some(o), true) = (old, prev != 0) {
        m.st32(prev, o as u32);
    }
    ret_stdcall(c, u32::from(old.is_some()), 3);
}

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x102;

/// Take `h` if it is free. The guest's threads run one at a time, so an object that is not
/// free can only become free when another guest thread runs: give way once, then report.
fn try_take(h: u32) -> Option<bool> {
    handles::with(h, |o| match o {
        Object::Mutex { owned } => {
            *owned += 1;
            true
        }
        Object::Semaphore { count, .. } if *count > 0 => {
            *count -= 1;
            true
        }
        Object::Event { set, manual } if *set => {
            if !*manual {
                *set = false;
            }
            true
        }
        Object::Thread(_) | Object::Process => true,
        _ => false,
    })
}

fn WaitForSingleObject(c: &mut Cpu) {
    let (h, timeout) = (arg(c, 0), arg(c, 1));
    let r = match try_take(h) {
        Some(true) => WAIT_OBJECT_0,
        Some(false) => {
            crate::yield_point(c);
            match try_take(h) {
                Some(true) => WAIT_OBJECT_0,
                _ if timeout != 0xffff_ffff => WAIT_TIMEOUT,
                _ => rc_runtime::dispatch::fatal(&format!(
                    "WaitForSingleObject({h:#x}, INFINITE) would never return"
                )),
            }
        }
        None => {
            set_error(ERROR_INVALID_HANDLE);
            0xffff_ffff
        }
    };
    ret_stdcall(c, r, 2);
}

/// A minimal log; the shell's `RC_TRACE` selects what is printed.
pub mod log {
    pub fn info(s: &str) {
        eprintln!("[kernel32] {s}");
    }
    pub fn warn(s: &str) {
        eprintln!("[kernel32] warning: {s}");
    }
    pub fn debug(s: &str) {
        if std::env::var("RC_TRACE").is_ok_and(|v| v.contains("debug")) {
            eprintln!("[kernel32] {s}");
        }
    }
    pub fn trace_files(s: &str) {
        if std::env::var("RC_TRACE").is_ok_and(|v| v.contains("files")) {
            eprintln!("[kernel32] {s}");
        }
    }
}

macro_rules! table {
    ($($f:ident),* $(,)?) => {
        &[$((stringify!($f), $f as rc_runtime::dispatch::GuestFn)),*]
    };
}

pub fn register() {
    imports::register(
        "kernel32.dll",
        table![
            GetModuleHandleA,
            GetCommandLineA,
            GetVersion,
            Sleep,
            QueryPerformanceCounter,
            RtlUnwind,
            GetEnvironmentStringsW,
            QueryPerformanceFrequency,
            WideCharToMultiByte,
            GetCurrentDirectoryA,
            GetFullPathNameA,
            IsBadWritePtr,
            HeapReAlloc,
            VirtualAlloc,
            VirtualFree,
            HeapCreate,
            HeapDestroy,
            GetEnvironmentVariableA,
            GetModuleFileNameA,
            GetOEMCP,
            GetACP,
            GetCPInfo,
            WriteFile,
            GetStringTypeW,
            GetStringTypeA,
            MultiByteToWideChar,
            SetFilePointer,
            GetFileType,
            GetStdHandle,
            SetHandleCount,
            TerminateProcess,
            ReadFile,
            ExitProcess,
            LCMapStringA,
            LCMapStringW,
            GetStartupInfoA,
            UnhandledExceptionFilter,
            FindFirstFileA,
            FindNextFileA,
            FileTimeToLocalFileTime,
            FileTimeToSystemTime,
            SetEnvironmentVariableA,
            CompareStringW,
            CompareStringA,
            RaiseException,
            SetEndOfFile,
            GetTimeZoneInformation,
            CreateFileA,
            SetStdHandle,
            HeapSize,
            FlushFileBuffers,
            SetPriorityClass,
            GetCurrentProcess,
            GetVolumeInformationA,
            GetDriveTypeA,
            GetLogicalDrives,
            SetErrorMode,
            SetFileAttributesA,
            OutputDebugStringA,
            FindClose,
            CreateSemaphoreA,
            SetCurrentDirectoryA,
            GetLastError,
            CreateMutexA,
            CloseHandle,
            ReleaseSemaphore,
            SetThreadPriority,
            GetPriorityClass,
            GetThreadPriority,
            WaitForSingleObject,
            GetVersionExA,
            GetCurrentThread,
            FreeLibrary,
            GetProcAddress,
            LoadLibraryA,
            CreateDirectoryA,
            DeleteFileA,
            HeapFree,
            HeapAlloc,
        ],
    );
    imports::register(
        "kernel32.dll",
        &[
            (
                "FreeEnvironmentStringsA",
                FreeEnvironmentStrings as rc_runtime::dispatch::GuestFn,
            ),
            ("FreeEnvironmentStringsW", FreeEnvironmentStrings),
            ("GetEnvironmentStrings", GetEnvironmentStrings),
            ("GetEnvironmentStringsA", GetEnvironmentStrings),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard("*.SBK", "canyon.sbk"));
        assert!(wildcard("save?.dat", "SAVE1.DAT"));
        assert!(!wildcard("*.tpl", "x.cmp"));
        assert!(wildcard("*.*", "README"));
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
