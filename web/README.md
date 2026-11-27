# web/

The converter page and the player (M8), after SackedRecomp's `web/`:

- `index.html`, `app.js`: pick a language (flag), the release is downloaded from its **fixed archive.org URL**
  (`games.js`, through archive.org's `/cors/` path), checked by SHA-256 and converted in a Web Worker
  (`worker.js`, the `rc-web` crate as wasm-bindgen module). Local copies of the same files are accepted too;
  "Or use copies you have" lists every language's files (built from `games.js`), each linked to its archive.org
  download.
- `play.html`, `play.js`: the player: the Rust runtime (`rc-runtime`, `rc-hle`, `rc-platform` for
  `wasm32-unknown-unknown`) plus the game's code as a second WebAssembly module (`rc-emit-wasm`), wgpu on WebGPU
  (WebGL2 fallback), Web Audio, the game's files from the browser's storage (OPFS).

`games.js` must match `games/technomage/game.toml`: a test in `rc-manifest` checks every medium's SHA-256.
