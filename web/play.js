// SPDX-License-Identifier: MIT
// The player page: runs the game the converter stored in this browser (worker.js) with the
// player's core (player.js) on the runtime module (player/technomage.wasm).
//
// - The game's files are the origin's private file system's (technomage/game/…, listed in
//   info.json), read in blocks as the game reads them.
// - What the game writes (settings, saved games) is kept in memory and saved to
//   technomage-user/ in the same storage a moment after each change.
// - Before it starts, the game's code is made again from its stored files when this site's
//   converter makes different code (a new version: worker.js "update"); the CD is not needed.
// - Frames go to a canvas at the game's size, scaled to fit (drawn on the GPU: copied from the
//   renderer's WebGL canvas); audio through Web Audio.
// - The game's mouse pointer is the canvas's cursor (CSS), as natively the system's.
import { gameOf, showIcon } from './games.js';
import { run } from './player.js';

const GAME = gameOf(window.location);
const USER_DIR = `${GAME.id}-user`;
const $ = (id) => document.getElementById(id);
const PREFS = `${GAME.id}.player`;

/* DirectInput scan code and Windows virtual key of a KeyboardEvent.code. */
const KEYS = {
  Escape: [0x01, 0x1b], Digit1: [0x02, 0x31], Digit2: [0x03, 0x32], Digit3: [0x04, 0x33],
  Digit4: [0x05, 0x34], Digit5: [0x06, 0x35], Digit6: [0x07, 0x36], Digit7: [0x08, 0x37],
  Digit8: [0x09, 0x38], Digit9: [0x0a, 0x39], Digit0: [0x0b, 0x30], Minus: [0x0c, 0xbd],
  Equal: [0x0d, 0xbb], Backspace: [0x0e, 0x08], Tab: [0x0f, 0x09], KeyQ: [0x10, 0x51],
  KeyW: [0x11, 0x57], KeyE: [0x12, 0x45], KeyR: [0x13, 0x52], KeyT: [0x14, 0x54],
  KeyY: [0x15, 0x59], KeyU: [0x16, 0x55], KeyI: [0x17, 0x49], KeyO: [0x18, 0x4f],
  KeyP: [0x19, 0x50], BracketLeft: [0x1a, 0xdb], BracketRight: [0x1b, 0xdd], Enter: [0x1c, 0x0d],
  ControlLeft: [0x1d, 0x11], KeyA: [0x1e, 0x41], KeyS: [0x1f, 0x53], KeyD: [0x20, 0x44],
  KeyF: [0x21, 0x46], KeyG: [0x22, 0x47], KeyH: [0x23, 0x48], KeyJ: [0x24, 0x4a],
  KeyK: [0x25, 0x4b], KeyL: [0x26, 0x4c], Semicolon: [0x27, 0xba], Quote: [0x28, 0xde],
  Backquote: [0x29, 0xc0], ShiftLeft: [0x2a, 0x10], Backslash: [0x2b, 0xdc], KeyZ: [0x2c, 0x5a],
  KeyX: [0x2d, 0x58], KeyC: [0x2e, 0x43], KeyV: [0x2f, 0x56], KeyB: [0x30, 0x42],
  KeyN: [0x31, 0x4e], KeyM: [0x32, 0x4d], Comma: [0x33, 0xbc], Period: [0x34, 0xbe],
  Slash: [0x35, 0xbf], ShiftRight: [0x36, 0x10], NumpadMultiply: [0x37, 0x6a], AltLeft: [0x38, 0x12],
  Space: [0x39, 0x20], CapsLock: [0x3a, 0x14], F1: [0x3b, 0x70], F2: [0x3c, 0x71],
  F3: [0x3d, 0x72], F4: [0x3e, 0x73], F5: [0x3f, 0x74], F6: [0x40, 0x75],
  F7: [0x41, 0x76], F8: [0x42, 0x77], F9: [0x43, 0x78], F10: [0x44, 0x79],
  F11: [0x57, 0x7a], F12: [0x58, 0x7b], Numpad7: [0x47, 0x67], Numpad8: [0x48, 0x68],
  Numpad9: [0x49, 0x69], NumpadSubtract: [0x4a, 0x6d], Numpad4: [0x4b, 0x64], Numpad5: [0x4c, 0x65],
  Numpad6: [0x4d, 0x66], NumpadAdd: [0x4e, 0x6b], Numpad1: [0x4f, 0x61], Numpad2: [0x50, 0x62],
  Numpad3: [0x51, 0x63], Numpad0: [0x52, 0x60], NumpadEnter: [0x9c, 0x0d], ControlRight: [0x9d, 0x11],
  AltRight: [0xb8, 0x12], Home: [0xc7, 0x24], ArrowUp: [0xc8, 0x26], PageUp: [0xc9, 0x21],
  ArrowLeft: [0xcb, 0x25], ArrowRight: [0xcd, 0x27], End: [0xcf, 0x23], ArrowDown: [0xd0, 0x28],
  PageDown: [0xd1, 0x22], Insert: [0xd2, 0x2d], Delete: [0xd3, 0x2e], IntlBackslash: [0x56, 0xe2],};

function loadPrefs() {
  try { return JSON.parse(localStorage.getItem(PREFS)) || {}; } catch (_) { return {}; }
}
function savePrefs(p) {
  try { localStorage.setItem(PREFS, JSON.stringify(p)); } catch (_) { /* this visit only */ }
}

function describe(e) {
  if (!e) return 'unknown error';
  const msg = e.message || String(e);
  return e.stack && !e.stack.includes(msg) ? `${msg}\n\n${e.stack}` : (e.stack || msg);
}

function fail(text) {
  $('overlay').hidden = false;
  $('status').textContent = 'The game stopped.';
  $('error').textContent = text;
  $('error').hidden = false;
  $('start').hidden = true;
}

function canPlay() {
  return typeof WebAssembly === 'object' && 'Suspending' in WebAssembly && 'promising' in WebAssembly
    && !!(navigator.storage && navigator.storage.getDirectory);
}

/* ---- the game's files ---- */

/* A directory handle below `top`, made on the way when `create`. */
async function dirAt(top, path, create) {
  let d = top;
  for (const part of path.split('/').filter(Boolean)) d = await d.getDirectoryHandle(part, { create });
  return d;
}

/* The stored game: info.json, the tree of its files and its code. */
async function findGame() {
  const root = await navigator.storage.getDirectory();
  const top = await root.getDirectoryHandle(GAME.id);
  const info = JSON.parse(await (await (await top.getFileHandle('info.json')).getFile()).text());
  const code = await (await top.getFileHandle('code.wasm')).getFile();
  return { root, top, info, code };
}

/* Everything the game wrote before: path below /user → bytes. */
async function loadUser(root) {
  const out = new Map();
  const dir = await root.getDirectoryHandle(USER_DIR, { create: true });
  const walk = async (d, prefix) => {
    for await (const [name, h] of d.entries()) {
      const p = `${prefix}/${name}`;
      if (h.kind === 'directory') { out.set(p, null); await walk(h, p); } else {
        const f = await h.getFile();
        out.set(p, { data: new Uint8Array(await f.arrayBuffer()), mtime: f.lastModified });
      }
    }
  };
  await walk(dir, '');
  return { dir, files: out };
}

/* The player's file system: /game from the stored game (read-only), /user in memory and saved
 * to the browser's storage. */
function gameFs(game, user, onSaved) {
  const key = (p) => p.toLowerCase();
  const tree = new Map(); /* lower-case path → { name, size, dir, path } */
  const kids = new Map(); /* lower-case dir → Set of names */
  const add = (p, size, dir) => {
    tree.set(key(p), { name: p.slice(p.lastIndexOf('/') + 1), size, dir, path: p });
    const parent = p.slice(0, p.lastIndexOf('/')) || '/';
    if (!kids.has(key(parent))) kids.set(key(parent), new Set());
    kids.get(key(parent)).add(p.slice(p.lastIndexOf('/') + 1));
  };
  tree.set('/game', { dir: true, size: 0 });
  tree.set('/user', { dir: true, size: 0 });
  for (const d of game.info.dirs) if (d.startsWith('game/')) add(`/${d}`, 0, true);
  for (const [p, size] of game.info.files) if (p.startsWith('game/')) add(`/${p}`, size, false);
  for (const [p, f] of user.files) add(`/user${p}`, f ? f.data.length : 0, !f);

  const handles = new Map();
  let next = 1;
  let dirty = new Set();
  let timer = null;
  const BLOCK = 1 << 18;

  const save = async () => {
    timer = null;
    const paths = [...dirty];
    dirty = new Set();
    try {
      for (const p of paths) {
        const f = user.files.get(p);
        const cut = p.lastIndexOf('/');
        const d = await dirAt(user.dir, p.slice(0, cut), true);
        if (f === undefined) { await d.removeEntry(p.slice(cut + 1)).catch(() => {}); continue; }
        if (f === null) { await d.getDirectoryHandle(p.slice(cut + 1), { create: true }); continue; }
        const w = await (await d.getFileHandle(p.slice(cut + 1), { create: true })).createWritable();
        await w.write(f.data);
        await w.close();
      }
      onSaved(null);
    } catch (e) { onSaved(e); }
  };
  const changed = (p) => { dirty.add(p); if (!timer) timer = setTimeout(save, 1000); };

  return {
    save,
    stat(p) {
      const e = tree.get(key(p));
      if (!e) return null;
      if (p.startsWith('/user/')) {
        const f = user.files.get(e.path.slice(5));
        return { size: f ? f.data.length : 0, dir: !f, mtime: f ? f.mtime : 0 };
      }
      return { size: e.size, dir: e.dir, mtime: 0 };
    },
    list(p) {
      const k = key(p.replace(/\/+$/, '') || '/');
      if (!tree.has(k)) return null;
      return [...(kids.get(k) || [])];
    },
    open(p, flags) {
      const e = tree.get(key(p));
      if (p.startsWith('/user/') || p === '/user') {
        const rel = e ? e.path.slice(5) : p.slice(5);
        if (e && flags & 4) return -2;
        if (!e && !(flags & 2)) return -1;
        if (!e) { user.files.set(rel, { data: new Uint8Array(0), mtime: Date.now() }); add(`/user${rel}`, 0, false); changed(rel); }
        const f = user.files.get(rel);
        if (!f) return -3;
        if (flags & 8) { f.data = new Uint8Array(0); changed(rel); }
        handles.set(next, { user: rel });
        return next++;
      }
      if (!e || e.dir) return -1;
      if (flags & 1) return -3;
      const fd = next++;
      return dirAt(game.top, e.path.slice(1, e.path.lastIndexOf('/')), false)
        .then((d) => d.getFileHandle(e.name))
        .then((h) => h.getFile())
        .then((file) => { handles.set(fd, { file, block: -1, data: null }); return fd; })
        .catch(() => -1);
    },
    read(fd, at, len) {
      const h = handles.get(fd);
      if (h.user !== undefined) {
        const d = user.files.get(h.user).data;
        return d.slice(at, Math.min(d.length, at + len));
      }
      const end = Math.min(h.file.size, at + len);
      if (end <= at) return new Uint8Array(0);
      // Reads inside one block come from memory; others go to the file (and keep its block).
      const b = Math.floor(at / BLOCK);
      if (h.block === b && end <= b * BLOCK + h.data.length) return h.data.slice(at - b * BLOCK, end - b * BLOCK);
      if (end - at > BLOCK) return h.file.slice(at, end).arrayBuffer().then((x) => new Uint8Array(x));
      const from = b * BLOCK;
      return h.file.slice(from, Math.min(h.file.size, from + 2 * BLOCK)).arrayBuffer().then((x) => {
        // Two blocks are read, so that a read across their border is in memory too; the
        // first is kept for the reads that follow.
        h.block = b;
        h.data = new Uint8Array(x);
        return h.data.slice(at - from, end - from);
      });
    },
    write(fd, at, bytes) {
      const h = handles.get(fd);
      if (h.user === undefined) return -1;
      const f = user.files.get(h.user);
      if (at + bytes.length > f.data.length) {
        const g = new Uint8Array(at + bytes.length);
        g.set(f.data);
        f.data = g;
      }
      f.data.set(bytes, at);
      f.mtime = Date.now();
      changed(h.user);
      return bytes.length;
    },
    size(fd) {
      const h = handles.get(fd);
      return h.user !== undefined ? user.files.get(h.user).data.length : h.file.size;
    },
    truncate(fd, size) {
      const h = handles.get(fd);
      if (h.user === undefined) return;
      const f = user.files.get(h.user);
      f.data = f.data.slice(0, size);
      changed(h.user);
    },
    flush() {},
    close(fd) { handles.delete(fd); },
    mkdir(p) {
      if (!p.startsWith('/user/')) return false;
      let at = '';
      for (const part of p.slice(6).split('/').filter(Boolean)) {
        at += `/${part}`;
        if (!tree.has(key(`/user${at}`))) { user.files.set(at, null); add(`/user${at}`, 0, true); changed(at); }
      }
      return true;
    },
    remove(p) {
      const e = tree.get(key(p));
      if (!e || !p.startsWith('/user/') || e.dir) return false;
      const rel = e.path.slice(5);
      user.files.delete(rel);
      tree.delete(key(p));
      const parent = e.path.slice(0, e.path.lastIndexOf('/'));
      kids.get(key(parent))?.delete(e.name);
      changed(rel);
      return true;
    },
  };
}

/* ---- the page ---- */

let game = null;
let player = null;

/* The game's pointer: a small canvas of its own over the picture that follows the mouse (the
 * system's cursor hidden), as big as in the scaled picture (an enlarged image's pixels are a part
 * of the frame's), or none while the game shows none. The game draws its pointer anew every
 * frame as it sways and turns: written into that canvas it costs next to nothing, where a cursor
 * made of it (a PNG every frame) cost much of a frame. */
const pointer = { el: null, ctx: null, hx: 0, hy: 0, x: -1e4, y: -1e4, over: true, shown: false };
function pointerPlace() {
  const p = pointer;
  if (!p.el) return;
  p.el.style.transform = `translate(${p.x - p.hx}px, ${p.y - p.hy}px)`;
}
function setPointer(canvas, rgba, w, h, hx, hy, scaleMilli = 1000) {
  const p = pointer;
  canvas.style.cursor = 'none';
  if (!p.el) {
    p.el = document.createElement('canvas');
    p.el.style.cssText = 'position: fixed; left: 0; top: 0; pointer-events: none; z-index: 5';
    p.ctx = p.el.getContext('2d');
    document.body.appendChild(p.el);
    const at = (e) => { p.x = e.clientX; p.y = e.clientY; pointerPlace(); };
    canvas.addEventListener('pointermove', at);
    canvas.addEventListener('pointerdown', at);
    canvas.addEventListener('pointerenter', (e) => { p.over = true; at(e); p.el.hidden = !p.shown; });
    canvas.addEventListener('pointerleave', () => { p.over = false; p.el.hidden = true; });
  }
  p.shown = !!rgba;
  p.el.hidden = !rgba || !p.over;
  if (!rgba) return;
  // The image's pixels per CSS pixel: per frame pixel (`scaleMilli`), the frame as shown.
  const s = (canvas.clientWidth / canvas.width || 1) / Math.max(0.001, scaleMilli / 1000);
  if (p.el.width !== w || p.el.height !== h) { p.el.width = w; p.el.height = h; }
  p.ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
  p.el.style.width = `${w * s}px`;
  p.el.style.height = `${h * s}px`;
  p.el.style.imageRendering = s >= 1 ? 'pixelated' : 'auto';
  p.hx = hx * s;
  p.hy = hy * s;
  pointerPlace();
}

function layout() {
  const c = $('canvas');
  const f = Math.min(window.innerWidth / c.width, window.innerHeight / c.height);
  c.style.width = `${Math.floor(c.width * f)}px`;
  c.style.height = `${Math.floor(c.height * f)}px`;
}

/* Web Audio: the mixer's samples, pulled in blocks. */
function startAudio() {
  let ctx;
  try { ctx = new AudioContext({ sampleRate: 44100 }); } catch (_) { return; }
  const node = ctx.createScriptProcessor(2048, 0, 2);
  node.onaudioprocess = (e) => {
    const l = e.outputBuffer.getChannelData(0);
    const r = e.outputBuffer.getChannelData(1);
    if (!player) { l.fill(0); r.fill(0); return; }
    const s = player.mix(l.length * 2);
    for (let i = 0; i < l.length; i += 1) { l[i] = s[2 * i]; r[i] = s[2 * i + 1]; }
  };
  node.connect(ctx.destination);
  ctx.resume().catch(() => {});
}

/* The browser's gamepads (the standard mapping) as one PlayStation pad, sent when it changes:
 * the buttons in rc_platform::pad's order (square, triangle, cross, circle, L2, L1, R2, R1,
 * select, start, L3, R3), the direction from the directional buttons or the left stick. */
const PAD_BUTTONS = [2, 3, 0, 1, 6, 4, 7, 5, 8, 9, 10, 11];
let lastPad = '';

/* Whose buttons the gamepad has, from its id (the browser's: the name, often with the USB vendor
 * id): Sony's PlayStation, Nintendo's, else Xbox-like (0, 2, 1 for the player). */
let lastStyle = -1;
function padStyle(id) {
  const s = /054c|playstation|dualsense|dualshock|wireless controller/i.test(id) ? 0
    : /057e|nintendo|pro controller|joy-con/i.test(id) ? 2 : 1;
  if (s !== lastStyle) { lastStyle = s; player.padStyle(s); }
}
function pollPads() {
  if (!player || !navigator.getGamepads) return;
  let buttons = 0;
  let x = 0;
  let y = 0;
  for (const g of navigator.getGamepads()) {
    if (!g || g.mapping !== 'standard') continue;
    const before = buttons;
    PAD_BUTTONS.forEach((b, i) => { if (g.buttons[b] && g.buttons[b].pressed) buttons |= 1 << i; });
    if (buttons !== before) padStyle(g.id);
    const on = (b) => (g.buttons[b] && g.buttons[b].pressed ? 1 : 0);
    let dx = on(15) - on(14);
    let dy = on(13) - on(12);
    if (!dx && !dy) { dx = g.axes[0] || 0; dy = g.axes[1] || 0; }
    if (!x && !y) { x = dx; y = dy; }
  }
  const axis = (v) => Math.round(Math.max(-1, Math.min(1, v)) * 32767);
  const now = `${buttons} ${axis(x)} ${axis(y)}`;
  if (now !== lastPad) {
    lastPad = now;
    player.pad(buttons, axis(x), axis(y));
  }
  vibrate();
}

/* The game's force feedback as the gamepads' vibration: when it changes, and renewed while it
 * lasts (an effect has a duration). */
let lastRumble = 0;
let rumbleAt = 0;
function vibrate() {
  const s = player.rumble();
  const t = performance.now();
  if (Math.abs(s - lastRumble) < 0.02 && !(s > 0 && t - rumbleAt >= 100)) return;
  lastRumble = s;
  rumbleAt = t;
  for (const g of navigator.getGamepads()) {
    const v = g && g.vibrationActuator;
    if (!v) continue;
    if (s > 0) {
      v.playEffect('dual-rumble', { duration: 150, strongMagnitude: s, weakMagnitude: s }).catch(() => {});
    } else if (v.reset) {
      v.reset().catch(() => {});
    }
  }
}

function hookInput(canvas) {
  const key = (down) => (e) => {
    const k = KEYS[e.code];
    if (!k || !player) return;
    e.preventDefault();
    if (down && e.repeat) return;
    player.key(k[0], k[1], down);
  };
  window.addEventListener('keydown', key(true));
  window.addEventListener('keyup', key(false));
  const pos = (e) => {
    const r = canvas.getBoundingClientRect();
    const x = Math.floor(((e.clientX - r.left) * canvas.width) / r.width);
    const y = Math.floor(((e.clientY - r.top) * canvas.height) / r.height);
    return [Math.max(0, Math.min(canvas.width - 1, x)), Math.max(0, Math.min(canvas.height - 1, y))];
  };
  canvas.addEventListener('pointermove', (e) => {
    if (!player) return;
    const [x, y] = pos(e);
    player.cursor(x, y);
    player.mouse(Math.round(e.movementX), Math.round(e.movementY));
  });
  const button = (down) => (e) => {
    if (!player || e.button > 2) return;
    const [x, y] = pos(e);
    player.cursor(x, y);
    player.button([0, 2, 1][e.button], down);
    canvas.focus();
  };
  canvas.addEventListener('pointerdown', button(true));
  canvas.addEventListener('pointerup', button(false));
  canvas.addEventListener('wheel', (e) => {
    if (!player) return;
    e.preventDefault();
    player.wheel(e.deltaY < 0 ? 120 : -120);
  }, { passive: false });
}

/* The converter (worker.js) makes the game's code again from the stored game if this site's
 * version makes different code; resolves when the stored code is current. */
function updateCode() {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
    worker.onmessage = (e) => {
      const m = e.data;
      if (m.type === 'progress') {
        const pct = m.total ? ` ${Math.floor((100 * m.done) / m.total)} %` : '';
        $('status').textContent = `This version makes new code for the game (${m.stage})${pct}…`;
      } else if (m.type === 'done') {
        worker.terminate();
        resolve(m.updated);
      } else if (m.type === 'error') {
        worker.terminate();
        reject(new Error(`The game's code could not be made again: ${m.message}`));
      }
    };
    worker.onerror = (e) => { worker.terminate(); reject(new Error(e.message || 'the converter failed')); };
    worker.postMessage({ mode: 'update', game: GAME.id });
  });
}

async function start() {
  $('start').disabled = true;
  $('status').textContent = 'Starting…';
  try {
    if (await updateCode()) game = await findGame();
    $('status').textContent = 'Starting…';
    const r = await fetch('player/technomage.wasm');
    if (!r.ok) throw new Error(`This site has no player for ${GAME.name} yet (player/technomage.wasm: ${r.status}).`);
    const runtime = await r.arrayBuffer();
    const user = await loadUser(game.root);
    const fs = gameFs(game, user, (err) => {
      $('saved').textContent = err ? `Not saved: ${err.message || err}` : `Saved ${new Date().toLocaleTimeString()}`;
    });
    const canvas = $('canvas');
    const ctx = canvas.getContext('2d');
    let image = null;
    const params = new URLSearchParams(location.search);
    const env = {};
    // ?trace=imports,files,fps (as RC_TRACE) logs to the console; RC_… settings in the address
    // are the runtime's as natively (RC_INPUT, RC_FRAMES, RC_RENDERER=soft, for tests).
    if (params.get('trace')) env.RC_TRACE = params.get('trace');
    for (const [k, v] of params) if (k.startsWith('RC_')) env[k] = v;
    startAudio();
    player = await run({
      runtime,
      code: await game.code.arrayBuffer(),
      fs,
      env,
      present(rgba, w, h) {
        if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; layout(); image = null; }
        if (!image) image = new ImageData(w, h);
        image.data.set(rgba);
        ctx.putImageData(image, 0, 0);
      },
      // A frame drawn on the GPU: copied from the renderer's canvas, without leaving it.
      presentCanvas(src, w, h) {
        if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; layout(); image = null; }
        ctx.drawImage(src, 0, 0);
      },
      pointer: (rgba, w, h, hx, hy, scaleMilli) => setPointer(canvas, rgba, w, h, hx, hy, scaleMilli),
      frame: () => new Promise((res) => requestAnimationFrame(() => { pollPads(); res(); })),
      log: (t) => console.log(t),
      exit: (code) => {
        fs.save();
        $('overlay').hidden = false;
        $('status').textContent = code === 0 ? 'The game has ended.' : `The game stopped (${code}).`;
        $('start').hidden = true;
      },
    });
    hookInput(canvas);
    // In device pixels: native resolution draws as many as the screen shows.
    const output = () => {
      const r = window.devicePixelRatio || 1;
      player.output(Math.round(window.innerWidth * r), Math.round(window.innerHeight * r));
    };
    window.addEventListener('resize', output);
    output();
    $('overlay').hidden = true;
    document.body.classList.remove('idle');
    canvas.focus();
    document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') fs.save(); });
    window.addEventListener('pagehide', () => fs.save());
    await player.start();
  } catch (e) {
    fail(describe(e));
  }
}

async function main() {
  const prefs = loadPrefs();
  showIcon(GAME);
  document.title = `${GAME.name} Player`;
  $('title').textContent = GAME.full || GAME.name;
  $('sharp').checked = !!prefs.sharp;
  document.body.classList.toggle('sharp', !!prefs.sharp);
  $('sharp').addEventListener('change', () => {
    prefs.sharp = $('sharp').checked;
    document.body.classList.toggle('sharp', prefs.sharp);
    savePrefs(prefs);
  });
  window.addEventListener('resize', layout);
  layout();
  $('fullscreen').addEventListener('click', () => {
    document.documentElement.requestFullscreen?.().catch(() => {});
    $('canvas').focus();
  });
  $('start').addEventListener('click', start);
  if (!canPlay()) {
    $('status').textContent = 'This browser cannot run the game: it needs WebAssembly stack switching (JSPI), '
      + 'which current versions of Chrome, Edge and Firefox have.';
    return;
  }
  try {
    game = await findGame();
  } catch (_) {
    $('status').textContent = 'The game is not in this browser yet. Convert it for "Browser" on the converter page first.';
    $('convert').hidden = false;
    return;
  }
  $('title').textContent = game.info.title;
  $('status').textContent = `${game.info.release}, converted on ${new Date(game.info.converted).toLocaleDateString()}.`;
  $('start').hidden = false;
  navigator.storage?.persist?.().catch(() => {});
}

main();
