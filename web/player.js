// SPDX-License-Identifier: MIT
// The player's core: runs the game on the runtime module (technomage-web, Rust for
// wasm32-unknown-unknown) with the game's code module (code.wasm from the converter, rc-emit-wasm).
// The page (play.js) and the Node test (tools/play-node.mjs) give it their host: files, frames,
// time, settings.
//
// - The code module shares the runtime's memory and function table: its functions go into the
//   table at `table_base`, and it calls the runtime through the helper functions, which are
//   table slots too (rc_helpers).
// - Waiting is JSPI: the imports that wait (opening and reading files, sleeping, a frame, a
//   thread switch) are WebAssembly.Suspending, the runtime's entries WebAssembly.promising.
// - Direct3D's batches are drawn by WebGL2 (gl.js), read back into guest memory.
// - Each guest thread has a JSPI stack and a shadow stack (Rust's `__stack_pointer`, in linear
//   memory) of its own. The shadow stack pointer is saved and set again around every wait, as
//   another thread may run meanwhile.

/* The host:
 *   fs: { stat(path) -> {size, dir, mtime} | null, list(path) -> [name] | null,
 *         open(path, flags) -> fd | -1 | -2 | -3 (or a promise of it; flags: 1 write, 2 create,
 *         4 create new, 8 truncate), read(fd, at, len) -> Uint8Array (or a promise),
 *         write(fd, at, bytes) -> n, size(fd), truncate(fd, size), flush(fd), close(fd),
 *         mkdir(path) -> bool, remove(path) -> bool }
 *   present(rgba, width, height): a frame (the bytes are only valid during the call)
 *   frame(): a promise that resolves when the game may go on after a frame
 *   env: { RC_…: value }
 *   log(text), exit(code)
 */
import { createGL } from './gl.js';

export async function run({ runtime, code, fs, present, frame, env = {}, log = console.log, exit }) {
  let ex = null;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const u8 = () => new Uint8Array(ex.memory.buffer);
  const str = (p, n) => dec.decode(u8().subarray(p, p + n));
  const SP = () => ex.__stack_pointer;

  /* An import that may wait: the shadow stack pointer is kept across the wait. */
  const waits = (f) => new WebAssembly.Suspending((...a) => {
    const r = f(...a);
    if (!(r instanceof Promise)) return r;
    const sp = SP().value;
    return r.then((v) => { SP().value = sp; return v; });
  });

  // ---- threads ----
  const threads = new Map([[1, { started: true, sp: 0, wake: null, top: 0 }]]);
  let threadMain = null;
  let failed = null;
  const fail = (e) => {
    if (failed) return;
    failed = e;
    log(`technomage: ${e && e.stack ? e.stack : e}`);
    exit(3);
  };
  const runThread = (to) => queueMicrotask(() => {
    const t = threads.get(to);
    if (!t) return fail(new Error(`no guest thread ${to}`));
    if (!t.started) {
      t.started = true;
      SP().value = t.top;
      threadMain(to).catch(fail);
    } else {
      const w = t.wake;
      t.wake = null;
      if (w) w();
    }
  });
  let current = 1;

  // ---- files: handles hold a position-free view; reads may wait ----
  const outF64 = (p, a, b) => { const v = new Float64Array(ex.memory.buffer, p, 2); v[0] = a; v[1] = b; };

  // Direct3D on WebGL2 (none in Node: the runtime draws in software there).
  const gl = createGL(() => ex.memory);
  const imports = {
    rc_js: {
      ...gl,
      fs_stat: (p, n, out) => {
        const s = fs.stat(str(p, n));
        if (!s) return 0;
        outF64(out, s.size || 0, s.mtime || 0);
        return s.dir ? 2 : 1;
      },
      fs_list: (p, n, buf, cap) => {
        const names = fs.list(str(p, n));
        if (!names) return -1;
        const b = enc.encode(names.join('\n'));
        if (b.length <= cap) u8().set(b, buf);
        return b.length;
      },
      fs_open: waits((p, n, flags) => fs.open(str(p, n), flags)),
      fs_read: waits((fd, at, buf, len) => {
        const put = (b) => { u8().set(b, buf); return b.length; };
        const r = fs.read(fd, at, len);
        return r instanceof Promise ? r.then(put) : put(r);
      }),
      fs_write: (fd, at, buf, len) => fs.write(fd, at, u8().slice(buf, buf + len)),
      fs_size: (fd) => fs.size(fd),
      fs_truncate: (fd, size) => fs.truncate(fd, size),
      fs_flush: (fd) => fs.flush(fd),
      fs_close: (fd) => fs.close(fd),
      fs_mkdir: (p, n) => (fs.mkdir(str(p, n)) ? 0 : -1),
      fs_remove: (p, n) => (fs.remove(str(p, n)) ? 0 : -1),
      now_ms: () => performance.now(),
      wall_ms_js: () => Date.now(),
      sleep_js: waits((ms) => (ms > 0 ? new Promise((r) => setTimeout(r, ms)) : undefined)),
      env_get: (p, n, buf, cap) => {
        const v = env[str(p, n)];
        if (v === undefined || v === null) return -1;
        const b = enc.encode(String(v));
        u8().set(b.subarray(0, cap), buf);
        return b.length;
      },
      log_js: (p, n) => log(str(p, n)),
      exit_js: waits((c) => { exit(c); return new Promise(() => {}); }),
      frame_js: waits(() => frame()),
      present_js: (p, w, h) => present(u8().subarray(p, p + w * h * 4), w, h),
      thread_new: (id, top) => { threads.set(id, { started: false, sp: 0, wake: null, top }); },
      thread_switch: new WebAssembly.Suspending((to) => {
        const me = current;
        const t = threads.get(me);
        t.sp = SP().value;
        const p = new Promise((r) => { t.wake = r; });
        current = to;
        runThread(to);
        return p.then(() => { current = me; SP().value = t.sp; });
      }),
      thread_end: new WebAssembly.Suspending((to) => {
        threads.delete(current);
        current = to;
        runThread(to);
        return new Promise(() => {});
      }),
    },
  };

  const rt = await WebAssembly.instantiate(runtime, imports);
  ex = rt.instance.exports;
  threadMain = WebAssembly.promising(ex.rc_thread_main);

  // The game's code: its tables from the custom section, its functions into the table.
  const codeModule = await WebAssembly.compile(code);
  const section = WebAssembly.Module.customSections(codeModule, 'rc.code')[0];
  if (!section) throw new Error('code.wasm has no rc.code section: not from this converter');
  const tables = new Uint8Array(section);
  const nfuncs = new DataView(section).getUint32(24, true);
  const table = ex.__indirect_function_table;
  const tableBase = table.grow(nfuncs);
  const helpers = new Uint32Array(ex.memory.buffer, ex.rc_helpers(), 11);
  const names = ['dispatch', 'trap', 'cpuid', 'rdtsc', 'fpu', 'fstsw', 'point', 'mmx',
    'dispatch_jump', 'unwind_claim', 'unwind_begin'];
  const rc = Object.fromEntries(names.map((n, i) => [n, table.get(helpers[i])]));
  await WebAssembly.instantiate(codeModule, {
    rc,
    env: { memory: ex.memory, table, table_base: new WebAssembly.Global({ value: 'i32' }, tableBase) },
  });
  const at = ex.rc_alloc(tables.length);
  u8().set(tables, at);
  if (ex.rc_load_code(at, tables.length, tableBase) !== 0) throw new Error('the runtime did not take the game\'s code');

  const main = WebAssembly.promising(ex.rc_main);
  const audio = { buf: 0, n: 0 };
  return {
    exports: ex,
    /* Run the game; resolves with its exit code when its main thread returns. */
    start: () => main().catch((e) => { fail(e); return 3; }),
    key: (scan, vk, down) => ex.rc_key(scan, vk, down ? 1 : 0),
    mouse: (dx, dy) => ex.rc_mouse(dx, dy),
    button: (b, down) => ex.rc_button(b, down ? 1 : 0),
    wheel: (dz) => ex.rc_wheel(dz),
    cursor: (x, y) => ex.rc_cursor(x, y),
    /* `n` interleaved stereo samples (44.1 kHz) as a Float32Array (valid until the next call). */
    mix: (n) => {
      if (audio.n < n) { audio.buf = (ex.rc_alloc(n * 4 + 4) + 3) & ~3; audio.n = n; }
      ex.rc_mix(audio.buf, n);
      return new Float32Array(ex.memory.buffer, audio.buf, n);
    },
  };
}
