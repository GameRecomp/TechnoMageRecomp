// SPDX-License-Identifier: MIT
// The browser player (web/player.js) in Node, headless: for tests and CI without a browser.
//
//   node --experimental-wasm-jspi tools/play-node.mjs RUNTIME.wasm GAME_DIR [USER_DIR]
//
// GAME_DIR holds what the converter stores for the browser: game/… and code.wasm. Settings
// come from the environment as for the native program (RC_FRAMES, RC_INPUT, RC_TRACE,
// RC_CLOCK); RC_SCREENSHOT with RC_SCREENSHOT_FRAME writes that frame as a PPM file.
import fs from 'node:fs';
import path from 'node:path';
import { run } from '../web/player.js';

const [runtimePath, gameDir, userDir = path.join(gameDir, 'user')] = process.argv.slice(2);
if (!runtimePath || !gameDir) {
  console.error('usage: node --experimental-wasm-jspi tools/play-node.mjs RUNTIME.wasm GAME_DIR [USER_DIR]');
  process.exit(2);
}
fs.mkdirSync(userDir, { recursive: true });

/* The player's paths onto the host's: /game and /user. */
function host(p) {
  if (p === '/game' || p.startsWith('/game/')) return path.join(gameDir, 'game', p.slice(5));
  if (p === '/user' || p.startsWith('/user/')) return path.join(userDir, p.slice(5));
  return null;
}

const files = new Map();
let nextFd = 1;
const nodeFs = {
  stat(p) {
    const h = host(p);
    try {
      const s = h && fs.statSync(h);
      return s ? { size: s.size, dir: s.isDirectory(), mtime: s.mtimeMs } : null;
    } catch (_) { return null; }
  },
  list(p) {
    const h = host(p);
    try { return h ? fs.readdirSync(h) : null; } catch (_) { return null; }
  },
  open(p, flags) {
    const h = host(p);
    if (!h) return -1;
    const exists = fs.existsSync(h);
    if (flags & 4 && exists) return -2;
    if (!exists && !(flags & 2)) return -1;
    if (flags & 1 && !p.startsWith('/user')) return -3;
    try {
      const fd = fs.openSync(h, flags & 1 ? (exists && !(flags & 8) ? 'r+' : 'w+') : 'r');
      files.set(nextFd, fd);
      return nextFd++;
    } catch (_) { return -3; }
  },
  read(fd, at, len) {
    const b = new Uint8Array(len);
    const n = fs.readSync(files.get(fd), b, 0, len, at);
    return b.subarray(0, n);
  },
  write: (fd, at, bytes) => fs.writeSync(files.get(fd), bytes, 0, bytes.length, at),
  size: (fd) => fs.fstatSync(files.get(fd)).size,
  truncate: (fd, size) => fs.ftruncateSync(files.get(fd), size),
  flush: (fd) => fs.fsyncSync(files.get(fd)),
  close: (fd) => { fs.closeSync(files.get(fd)); files.delete(fd); },
  mkdir(p) { const h = host(p); try { fs.mkdirSync(h, { recursive: true }); return true; } catch (_) { return false; } },
  remove(p) { const h = host(p); try { fs.unlinkSync(h); return true; } catch (_) { return false; } },
};

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => k.startsWith('RC_')));
let frames = 0;
const t0 = performance.now();
const player = await run({
  runtime: fs.readFileSync(runtimePath),
  code: fs.readFileSync(path.join(gameDir, 'code.wasm')),
  fs: nodeFs,
  env,
  present(rgba, w, h) {
    frames += 1;
    if (env.RC_SCREENSHOT && String(frames) === (env.RC_SCREENSHOT_FRAME || '1')) {
      const out = Buffer.alloc(w * h * 3);
      for (let i = 0; i < w * h; i += 1) {
        out[3 * i] = rgba[4 * i]; out[3 * i + 1] = rgba[4 * i + 1]; out[3 * i + 2] = rgba[4 * i + 2];
      }
      fs.writeFileSync(env.RC_SCREENSHOT, Buffer.concat([Buffer.from(`P6\n${w} ${h}\n255\n`), out]));
      console.error(`screenshot of frame ${frames} in ${env.RC_SCREENSHOT}`);
    }
    if (frames % 100 === 0) {
      console.error(`[play-node] ${frames} frames, ${(frames * 1000 / (performance.now() - t0)).toFixed(1)} frames/s overall`);
    }
  },
  frame: () => new Promise((r) => setImmediate(r)),
  exit(code) { console.error(`[play-node] exit ${code} after ${frames} frames`); process.exit(code); },
});
const code = await player.start();
console.error(`[play-node] the game returned ${code}`);
process.exit(code);
