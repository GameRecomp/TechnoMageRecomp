// SPDX-License-Identifier: MIT
// The converter page: choose the system and a language, whose release is
// downloaded from archive.org and checked, with the files it needs beside it
// (its supplements; or the user's own copies of them), then run the worker and
// write what it produces to a file. Nothing is uploaded. For the browser, the
// worker stores the game in this origin's private storage instead, and
// play.html runs it.

import { GAMES, gameOf, showIcon } from './games.js';

const $ = (id) => document.getElementById(id);

/* The game (there is one). */
const game = gameOf(window.location);

const STAGES = [
  { id: 'identify', label: 'Reading the CD', weight: 2 },
  { id: 'lift', label: 'Translating the game code', weight: 4 },
  { id: 'compile', label: 'Compiling to native code', web: 'Compiling to WebAssembly', weight: 14 },
  { id: 'package', label: 'Packing the game files', web: 'Storing the game files in this browser', weight: 80 },
];
let webMode = false;

/* The systems, with what to do with the result, for the chosen game. */
function systems() {
  const g = game;
  const app = g.stem;
  return {
    windows: { label: 'Windows', after: `Unzip it and start technomage.exe in the ${app} folder.` },
    linux: {
      label: 'Linux',
      ext: 'AppImage',
      type: { description: 'AppImage', accept: { 'application/vnd.appimage': ['.AppImage'] } },
      after: 'The AppImage holds the whole game. Make it executable (chmod +x, or tick "Allow executing" in '
        + 'its properties) and start it. Without FUSE, start it with --appimage-extract-and-run.',
    },
    macos: {
      label: 'macOS',
      after: `Unzip it and open ${app}.app. It is not signed: the first time, right-click it and choose Open, `
        + 'or allow it under System Settings → Privacy & Security.',
    },
    web: {
      label: 'Browser',
      web: true,
      after: 'The game is stored in this browser, for this site only. Play it now, or come back to this page '
        + 'any time. Saved games and settings are kept in the browser too.',
    },
    android: {
      label: 'Android',
      file: `${app}.apk`,
      type: { description: 'Android app', accept: { 'application/vnd.android.package-archive': ['.apk'] } },
      after: `Copy ${app}.apk to your phone or tablet and open it there to install it (allow installing from `
        + 'that app when asked). On its first start the game unpacks its files once, which takes a minute or two. '
        + 'A finger is the mouse: tap to click, a second finger is the right button; Back opens the menu.',
    },
  };
}

/* The processor chosen in a system's row (Windows and Linux have x86-64 and ARM64). */
function archOf(system) {
  const s = document.querySelector(`select.arch[data-system=${system}]`);
  return s ? s.value : 'x86_64';
}

/* The build a system and its processor name: the template's name (TechnoMage-windows-arm64). */
function buildOf(system) {
  return archOf(system) === 'arm64' ? `${system}-arm64` : system;
}

/* The processor, where the browser tells it (Chromium's client hints, Linux's platform string). */
async function guessArch() {
  try {
    const h = await navigator.userAgentData?.getHighEntropyValues?.(['architecture']);
    if (h && h.architecture) return /arm/i.test(h.architecture) ? 'arm64' : 'x86_64';
  } catch (_) { /* not told */ }
  return /aarch64|arm64|armv8/i.test(`${navigator.platform} ${navigator.userAgent}`) ? 'arm64' : 'x86_64';
}

function guessSystem() {
  const p = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent;
  if (/android/i.test(p) || /android/i.test(navigator.userAgent)) return 'android';
  if (/win/i.test(p)) return 'windows';
  if (/mac|iphone|ipad/i.test(p)) return 'macos';
  return 'linux';
}

/* What playing in this browser needs: stack switching (JSPI) and the
 * origin's private file system with synchronous access in workers. */
function browserCanPlay() {
  return typeof WebAssembly === 'object' && 'Suspending' in WebAssembly && 'promising' in WebAssembly
    && !!(navigator.storage && navigator.storage.getDirectory);
}

/* The game stored in this browser (its info.json), or null. */
async function storedGame() {
  try {
    const root = await navigator.storage.getDirectory();
    const top = await root.getDirectoryHandle(game.id);
    const f = await (await top.getFileHandle('info.json')).getFile();
    return JSON.parse(await f.text());
  } catch (_) {
    return null;
  }
}

let storedInfo = null;
let reconvert = false;

/* With Browser chosen and the game stored, the player takes the place of
 * the installer and convert steps (unless the user converts again). */
function updateView() {
  const web = document.querySelector('input[name=system]:checked')?.value === 'web';
  const player = web && !!storedInfo && !reconvert && !running;
  $('stored').hidden = !player;
  $('step-file').hidden = player;
  $('step-convert').hidden = player;
  document.body.classList.toggle('playing', player);
  if (storedInfo) {
    const when = new Date(storedInfo.converted).toLocaleDateString();
    $('stored-title').textContent = storedInfo.title;
    $('stored-info').textContent = `${storedInfo.release}, converted on ${when}. `
      + 'Saved games and settings are kept in this browser.';
  }
}

async function showStored() {
  storedInfo = await storedGame();
  updateView();
}

async function removeStored() {
  if (!window.confirm('Remove the game from this browser? Saved games and settings are kept.')) return;
  try {
    const root = await navigator.storage.getDirectory();
    await root.removeEntry(game.id, { recursive: true });
  } catch (err) {
    showError(`Cannot remove it: ${err}`);
  }
  reconvert = false;
  showStored();
}

function updateNote() {
  const web = document.querySelector('input[name=system]:checked')?.value === 'web';
  $('convert-note').textContent = web
    ? `The game (${game.size}) is stored in this browser. Converting takes a few minutes.`
    : `The result is a zip (for Android an APK) of ${game.size} with the game in it. You choose where it is saved.`;
  updateView();
}

function mib(n) {
  return `${(n / (1 << 20)).toFixed(0)} MiB`;
}

function clock(ms) {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${String(s % 60).padStart(2, '0')} s`;
}

let running = false;

/* The medium (a File) and the files its language needs beside it, in games.js's order. */
let file = null;
let extras = [];

/* The language whose medium has this size. */
function languageOf(f) {
  return game.languages.find((l) => l.size === f.size) || null;
}

/* What a language needs beside its medium: the medium's further parts, then the supplements. */
function besides(lang) {
  return [...(lang.parts || []), ...(lang.supplements || [])];
}

/* A medium and its further parts as one file (the converter reads them joined). */
function joined(first, parts) {
  return parts.length ? new File([first, ...parts], first.name) : first;
}

/* The files chosen or dropped: the medium, its parts and its supplements, told apart by their
 * sizes (a .cue only lists a CD image's tracks and is skipped). When languages share a file (the
 * English, French, Italian and Spanish releases need the German CD), the medium is the one whose
 * other files are there too. */
function setFiles(list) {
  const all = [...(list || [])].filter((f) => !/\.cue$/i.test(f.name));
  const media = all.filter((f) => languageOf(f));
  const complete = (m) => besides(languageOf(m)).every((s) => all.some((f) => f !== m && f.size === s.size));
  const first = media.find(complete) || media.find((m) => languageOf(m).code !== 'de') || media[0] || null;
  const chosen = [];
  extras = [];
  const missing = [];
  if (first) {
    const lang = languageOf(first);
    const parts = [];
    for (const s of besides(lang)) {
      const f = all.find((x) => x !== first && x.size === s.size);
      if (!f) missing.push(s.file);
      else if ((lang.parts || []).includes(s)) parts.push(f);
      else extras.push(f);
      if (f) chosen.push(f);
    }
    file = joined(first, parts);
  } else {
    file = null;
  }
  const names = [first, ...chosen].filter(Boolean).map((f) => `${f.name} (${mib(f.size)})`);
  let note = names.length ? names.join(' + ') : (all.length ? 'Not one of the archived files' : 'No file chosen');
  if (missing.length) note += ` — choose ${missing.join(' and ')} too (all at once)`;
  $('file-name').textContent = note;
  $('start').disabled = !file || missing.length > 0;
}
function setFile(f) { setFiles(f ? [f] : []); }

/* ---- the archived releases ---- */

let downloading = null; /* an AbortController while a download runs */

function hex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/* Download one archived file, check its size and SHA-256, and give it as a File. `shown` says
 * what it is in the note. */
async function download(item, shown, signal) {
  const note = $('download-note');
  note.textContent = `${shown}: connecting to archive.org…`;
  const r = await fetch(item.url, { signal });
  if (!r.ok || !r.body) throw new Error(`archive.org answered ${r.status} for ${item.file}`);
  const reader = r.body.getReader();
  const parts = [];
  let got = 0;
  const t0 = performance.now();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    parts.push(value);
    got += value.length;
    const rate = got / Math.max(1, performance.now() - t0);
    const left = rate > 0 ? (item.size - got) / rate : 0;
    note.textContent = `${shown}: ${mib(got)} of ${mib(item.size)} downloaded · about ${clock(left)} left`;
  }
  if (got !== item.size) throw new Error(`${item.file}: the download has ${got} bytes, not ${item.size}`);
  note.textContent = `${shown}: checking the download…`;
  const blob = new Blob(parts);
  parts.length = 0;
  const sum = hex(await crypto.subtle.digest('SHA-256', await blob.arrayBuffer()));
  if (sum !== item.sha256) throw new Error(`${item.file} is not the archived file (its SHA-256 differs)`);
  return new File([blob], item.file);
}

/* Download the release of `lang` and what it needs beside it, check them, and convert. */
async function fetchRelease(lang) {
  if (running || downloading) return;
  const note = $('download-note');
  $('error').hidden = true;
  $('result').hidden = true;
  for (const b of document.querySelectorAll('#languages button')) b.disabled = true;
  downloading = new AbortController();
  const items = [lang, ...besides(lang)];
  let current = lang;
  try {
    const files = [];
    for (const [k, item] of items.entries()) {
      current = item;
      const shown = items.length > 1 ? `${lang.title} (${k + 1} of ${items.length}, ${item.file})` : lang.title;
      files.push(await download(item, shown, downloading.signal));
    }
    const nparts = (lang.parts || []).length;
    file = joined(files[0], files.slice(1, 1 + nparts));
    extras = files.slice(1 + nparts);
    $('file-name').textContent = files.map((f) => `${f.name} (${mib(f.size)})`).join(' + ');
    $('start').disabled = false;
    // Converting for the browser starts at once. A zip asks where to save it, which a browser
    // allows only right after a click (not minutes later, at the end of a download).
    const web = document.querySelector('input[name=system]:checked')?.value === 'web';
    const total = items.reduce((n, i) => n + i.size, 0);
    note.textContent = `${lang.title}: downloaded and checked (${mib(total)}).`
      + (web ? '' : ' Press Convert to make the zip.');
    if (web) start();
  } catch (err) {
    if (err && err.name === 'AbortError') return;
    note.textContent = '';
    showError(`Cannot download ${lang.title}: ${err.message || err}. You can download it yourself from `
      + `${current.url.replace('/cors/', '/download/')} and choose it below.`);
  } finally {
    downloading = null;
    for (const b of document.querySelectorAll('#languages button')) b.disabled = running;
  }
}

/* The game's languages as flags. */
function showLanguages() {
  const box = $('languages');
  box.replaceChildren();
  for (const lang of game.languages) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'language';
    const total = [lang, ...besides(lang)].reduce((n, i) => n + i.size, 0);
    b.title = `${lang.title} (${lang.label}), ${mib(total)} from archive.org`;
    const img = document.createElement('img');
    img.src = `flags/${lang.flag || lang.code}.svg`;
    img.alt = '';
    img.height = 24;
    const label = document.createElement('span');
    label.textContent = lang.label;
    b.append(img, label);
    b.addEventListener('click', () => fetchRelease(lang));
    box.append(b);
  }
  showNeeds();
  // The footer names the releases from the same list.
  const titles = game.languages.map((l) => `${l.label} (${l.title})`);
  $('supported').textContent = titles.length > 1
    ? `${titles.slice(0, -1).join(', ')} and ${titles[titles.length - 1]}`
    : titles.join('');
}

/* The files of every language as a table, each linked where archive.org keeps it (its download
 * address, not the /cors/ one the page fetches through). */
function showNeeds() {
  const link = (item) => {
    const a = document.createElement('a');
    a.href = item.url.replace('/cors/', '/download/');
    a.textContent = item.file;
    a.rel = 'noopener';
    const code = document.createElement('code');
    code.append(a);
    const size = document.createElement('small');
    size.textContent = item.size < 1 << 20 ? ` ${Math.ceil(item.size / 1024)} KiB` : ` ${mib(item.size)}`;
    const line = document.createElement('div');
    line.append(code, size);
    return line;
  };
  const body = $('needs').querySelector('tbody');
  body.replaceChildren();
  for (const lang of game.languages) {
    const tr = document.createElement('tr');
    const name = document.createElement('th');
    name.scope = 'row';
    const img = document.createElement('img');
    img.src = `flags/${lang.flag || lang.code}.svg`;
    img.alt = '';
    img.height = 16;
    name.append(img, ` ${lang.label}`);
    const medium = document.createElement('td');
    for (const item of [lang, ...(lang.parts || [])]) medium.append(link(item));
    const beside = document.createElement('td');
    for (const item of lang.supplements || []) beside.append(link(item));
    if (!(lang.supplements || []).length) beside.textContent = '–';
    tr.append(name, medium, beside);
    body.append(tr);
  }
}

function buildStageList() {
  const list = $('stages');
  list.replaceChildren();
  for (const s of STAGES) {
    const li = document.createElement('li');
    li.id = `stage-${s.id}`;
    li.dataset.state = 'waiting';
    const label = document.createElement('span');
    label.textContent = (webMode && s.web) || s.label;
    const detail = document.createElement('span');
    detail.className = 'detail';
    li.append(label, detail);
    list.append(li);
  }
}

/* Where the zip goes: a file the user picked (streamed to disk), or parts
 * kept until the end and offered as a download. */
async function openOutput(name, type) {
  if (window.showSaveFilePicker) {
    const handle = await window.showSaveFilePicker({
      suggestedName: name,
      types: [type || { description: 'Zip archive', accept: { 'application/zip': ['.zip'] } }],
    });
    const stream = await handle.createWritable();
    let chain = Promise.resolve();
    return {
      write: (chunk) => { chain = chain.then(() => stream.write(chunk)); return chain; },
      close: async () => { await chain; await stream.close(); return null; },
      abort: async () => { try { await stream.abort(); } catch (_) { /* already closed */ } },
    };
  }
  const parts = [];
  return {
    write: (chunk) => { parts.push(new Blob([chunk])); return Promise.resolve(); },
    close: async () => URL.createObjectURL(new Blob(parts, { type: type ? Object.keys(type.accept)[0] : 'application/zip' })),
    abort: async () => { parts.length = 0; },
  };
}

function showError(text) {
  $('error').textContent = text;
  $('error').hidden = false;
}

async function start() {
  if (!file || running) return;
  const chosen = document.querySelector('input[name=system]:checked');
  if (!chosen) return; /* (every system is off on this site) */
  const system = chosen.value;
  const sys = systems()[system];
  const build = buildOf(system);
  const forGame = game;
  const arch = build === system ? '' : '-ARM64';
  const outName = sys.file || `${forGame.stem}-${sys.label.replace(/\s/g, '')}${arch}.${sys.ext || 'zip'}`;
  $('error').hidden = true;
  $('result').hidden = true;
  webMode = !!sys.web;

  let out;
  if (webMode) {
    // Nothing to save: the worker stores the game. Ask the browser to keep
    // it (and the saved games) rather than clear it when space runs low.
    out = { write: () => Promise.resolve(), close: async () => null, abort: async () => {} };
    // Not awaited: Firefox asks the user, who may not answer.
    navigator.storage?.persist?.().catch(() => {});
  } else {
    try {
      out = await openOutput(outName, sys.type);
    } catch (err) {
      if (err && err.name === 'AbortError') return;
      showError(`Cannot write the file: ${err}`);
      return;
    }
  }

  running = true;
  $('start').disabled = true;
  $('choose').disabled = true;
  $('progress-card').hidden = false;
  buildStageList();
  const t0 = performance.now();
  const timer = setInterval(() => { $('elapsed').textContent = clock(performance.now() - t0); }, 500);

  let template;
  if (!webMode) {
    try {
      const r = await fetch(`templates/${forGame.stem}-${build}.zip`);
      if (!r.ok) {
        const which = build === system ? sys.label : `${sys.label} ARM64`;
        throw new Error(`the ${which} build of ${forGame.name} is not available on this site (${r.status})`);
      }
      template = await r.arrayBuffer();
    } catch (err) {
      await out.abort();
      finish(false, String(err.message || err));
      return;
    }
  }

  const worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
  let current = null;
  let packTotal = 0;
  let packStart = 0;

  function stageDone(id) {
    const li = $(`stage-${id}`);
    if (li) { li.dataset.state = 'done'; li.querySelector('.detail').textContent = ''; }
  }

  function overall(stage, frac) {
    let before = 0;
    for (const s of STAGES) {
      if (s.id === stage) break;
      before += s.weight;
    }
    const w = STAGES.find((s) => s.id === stage).weight;
    const pct = Math.min(100, before + w * frac);
    $('bar').value = pct;
    $('bar-label').textContent = `${Math.floor(pct)} %`;
  }

  worker.onmessage = async (e) => {
    const m = e.data;
    if (m.type === 'progress') {
      if (m.stage !== current) {
        if (current) stageDone(current);
        current = m.stage;
        const li = $(`stage-${current}`);
        if (li) li.dataset.state = 'active';
      }
      const frac = m.total > 0 ? m.done / m.total : 0;
      overall(m.stage, frac);
      const detail = $(`stage-${m.stage}`)?.querySelector('.detail');
      if (detail) {
        if (m.stage === 'compile') detail.textContent = `${m.done} of ${m.total} functions`;
        else if (m.stage === 'package') {
          packTotal = m.total;
          if (!packStart) packStart = performance.now();
          const rate = m.done / Math.max(1, performance.now() - packStart);
          const left = rate > 0 ? (m.total - m.done) / rate : 0;
          detail.textContent = `${mib(m.done)} of ${mib(m.total)} · about ${clock(left)} left`;
        }
      }
    } else if (m.type === 'data') {
      out.write(m.chunk);
    } else if (m.type === 'done') {
      for (const s of STAGES) stageDone(s.id);
      overall('package', 1);
      const url = await out.close();
      worker.terminate();
      finish(true, m.summary, url, outName, packTotal);
    } else if (m.type === 'error') {
      worker.terminate();
      await out.abort();
      const li = current && $(`stage-${current}`);
      if (li) li.dataset.state = 'failed';
      finish(false, m.message);
    }
  };
  worker.onerror = async (e) => {
    worker.terminate();
    await out.abort();
    finish(false, e.message || 'the converter stopped');
  };
  if (webMode) worker.postMessage({ file, extras, mode: 'web', game: forGame.id });
  else worker.postMessage({ file, extras, template, game: forGame.id }, [template]);

  function finish(ok, text, url, name) {
    clearInterval(timer);
    running = false;
    $('start').disabled = !file;
    $('choose').disabled = false;
    if (!ok) {
      showError(text);
      return;
    }
    $('result-summary').textContent = text;
    $('result-after').textContent = sys.after;
    $('play').hidden = !webMode;
    $('result-warn').hidden = webMode;
    if (webMode) {
      reconvert = false;
      showStored();
    }
    const link = $('download');
    if (url) {
      link.href = url;
      link.download = name;
      link.hidden = false;
      link.click();
    } else {
      link.hidden = true;
    }
    $('result').hidden = false;
  }
}

window.addEventListener('beforeunload', (e) => {
  if (running) { e.preventDefault(); e.returnValue = ''; }
});

$('choose').addEventListener('click', () => $('file').click());
$('file').addEventListener('change', (e) => setFiles(e.target.files));
$('start').addEventListener('click', start);
const drop = $('drop');
drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
drop.addEventListener('dragleave', () => drop.classList.remove('over'));
drop.addEventListener('drop', (e) => {
  e.preventDefault();
  drop.classList.remove('over');
  if (!running && e.dataTransfer.files.length) setFiles(e.dataTransfer.files);
});
/* The systems' notes as the page has them, to put back when a system is
 * offered again. */
const NOTES = new Map([...document.querySelectorAll('input[name=system]')].map(
  (i) => [i.value, i.closest('label').querySelector('small').textContent],
));

/* Why each system cannot be chosen, for every game (the browser, the site)
 * and for the chosen one (a site without its web shell). */
const offAlways = new Map();
const offForGame = new Map(Object.keys(GAMES).map((id) => [id, new Map()]));

/* Grey out what cannot be chosen, with the reason, and keep a choice. */
function applyChoices() {
  for (const input of document.querySelectorAll('input[name=system]')) {
    const why = offAlways.get(input.value) || offForGame.get(game.id).get(input.value);
    input.disabled = !!why;
    input.closest('label').classList.toggle('off', !!why);
    input.closest('label').querySelector('small').textContent = why || NOTES.get(input.value);
    if (why && input.checked) input.checked = false;
    const arch = input.closest('label').querySelector('select.arch');
    if (arch) arch.disabled = !!why;
  }
  if (!document.querySelector('input[name=system]:checked')) {
    const first = document.querySelector('input[name=system]:not(:disabled)');
    if (first) first.checked = true;
  }
  updateNote();
}

/* Whether this site has the web shell for a game (built per game). */
const playerChecked = new Map();
function checkPlayer(g) {
  if (!playerChecked.has(g.id)) {
    playerChecked.set(g.id, fetch('player/technomage.wasm', { method: 'HEAD' })
      .then((r) => { if (!r.ok) offForGame.get(g.id).set('web', 'not available on this site yet'); })
      .catch(() => {}));
  }
  return playerChecked.get(g.id);
}

/* The page for the chosen game: its texts, tab, address and stored copy. */
function showGame() {
  document.title = `${game.name} Converter`;
  showIcon(game);
  $('game-name').textContent = game.full || game.name;
  $('download-note').textContent = '';
  showLanguages();
  for (const a of document.querySelectorAll('a.play')) a.href = `play.html?game=${game.id}`;
  const url = new URL(window.location.href);
  url.hash = '';
  url.searchParams.set('game', game.id);
  window.history.replaceState(null, '', url);
  applyChoices();
  storedInfo = null;
  reconvert = false;
  updateView();
  const g = game;
  Promise.all([storedGame(), checkPlayer(g)]).then(([info]) => {
    if (g !== game) return;
    applyChoices();
    storedInfo = info;
    const web = document.querySelector('input[name=system][value=web]');
    if (info && !web.disabled) {
      web.checked = true;
      updateNote();
    }
    updateView();
  });
}

const guessed = document.querySelector(`input[name=system][value=${guessSystem()}]`);
guessed.checked = true;
for (const r of document.querySelectorAll('input[name=system]')) r.addEventListener('change', updateNote);
// Choosing a processor chooses its system too.
for (const s of document.querySelectorAll('select.arch')) {
  s.addEventListener('change', () => {
    const radio = document.querySelector(`input[name=system][value=${s.dataset.system}]`);
    if (!radio.disabled) radio.checked = true;
    updateNote();
  });
}
guessArch().then((arch) => {
  for (const s of document.querySelectorAll('select.arch')) {
    const option = s.querySelector(`option[value=${arch}]`);
    if (option && !option.disabled) s.value = arch;
  }
});
$('remove').addEventListener('click', removeStored);
$('reconvert').addEventListener('click', () => { reconvert = true; updateView(); });
if (!browserCanPlay()) {
  offAlways.set('web', 'needs a browser with WebAssembly stack switching (JSPI), such as a current Chrome, Edge or Firefox');
}
// The systems this site is built for (platforms.json, the same file CI
// reads); a site without it offers all of them.
fetch('platforms.json', { cache: 'no-cache' })
  .then((r) => (r.ok ? r.json() : {}))
  .catch(() => ({}))
  .then((on) => {
    for (const system of ['windows', 'linux', 'macos', 'android', 'web']) {
      if (on[system] === false) offAlways.set(system, 'disabled on this site for now');
    }
    // ARM64 builds of Windows and Linux (windows-arm64, linux-arm64).
    for (const s of document.querySelectorAll('select.arch')) {
      const option = s.querySelector('option[value=arm64]');
      option.disabled = on[`${s.dataset.system}-arm64`] === false;
      if (option.disabled && s.value === 'arm64') s.value = 'x86_64';
    }
    showGame();
  });
setFile(null);
showGame();
