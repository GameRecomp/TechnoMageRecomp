# Research

Findings with their sources. Open questions are marked **(open)**.

## The game

*TechnoMage – Die Rückkehr der Ewigkeit* (English: *TechnoMage: Return of Eternity*), action role-playing game by
Sunflowers, released in Germany on 2000-11-24, in PAL regions only (no North American release found). Eight
worlds: Dreamertown, Steamertown, The Hive, the fairy forest, the Canyon, the Tower, the Volcano, the Ruins.

## Versions and patches

- German retail CD: `DATA.TAG` `Version=1.00.000`, mastered 2000-11-07, SecuROM 4.10.00.0054
  ([redump 7367](http://redump.org/disc/7367/)).
- Patch **1.02** (German), on the official site by 2001-01-28
  ([page](https://web.archive.org/web/20010128001100/http://www.technomage.de:80/german/downloads/patch.html)):
  smaller polygon gaps, secondary graphics cards in the setup tool, 7 MB less memory, no needless CD access on
  level change, movies from the hard disk on a full install, quest and crash fixes. Not archived.
- Patch **1.04** (German), 2001-03-23
  ([page](https://web.archive.org/web/20010406082413/http://www.technomage.de:80/german/downloads/patch2.html)):
  a state bug (vanishing items, wrong dialogue, doors), many level data fixes, the checker `TM_Diagnostic.exe`;
  old saves are converted (back to the start of the current world). On archive.org: item `TechnoMageRoE`.
  The patch is a RAR SFX (RAR 2.0 method, solid) with an InstallShield 5 setup and the changed files:
  `TechnoMage.exe`, `TM_Diagnostic.exe`, `TM_Setup.exe`, `DATA.OVL`, `resource.cmp`, 23 `Lev/*.cmp`,
  `Sfx/HBoss.SBK`, `History.txt`.
- No English patch: the English download page (2001-04-27) offered only the German 1.02.
- **The SCREENFUN 03/2004 cover CD is version 1.04.** Its `TechnoMage.exe` has the same link time as the patch's
  (2001-03-19 18:43:50), and every level and data file the patch carries is byte-identical to the cover CD's
  (`Sfx/HBoss.SBK` not yet compared: see the InstallShield 7 note below). The patch's executable is the same
  build wrapped in SecuROM (sections `.cms_t`, `.cms_d` after UPX's); the cover CD's has no SecuROM. So no patch is
  applied for German, and the copy protection is gone.
- English CD (`TECHNOMAGE.mdf` SHA-256 `178a16bf…`): `Technomage.EXE` 885 379 bytes, linked 2001-04-25, a later
  build than the German 1.04. Like patch 1.04's executable it is **UPX inside SecuROM**: sections `UPX0`, `UPX1`,
  `UPX2`, then `.cms_t` and `.cms_d`, the entry point in `.cms_t`. Comparing patch 1.04's wrapped executable with
  the cover CD's plain one (the same build) shows what SecuROM changes: the UPX pack header is gone and the last
  12 KiB of `UPX1` (file offsets `0x89400`–`0x8c3fe`, the end of the compressed data and UPX's loader) differ, so
  they are encrypted. For German this does not matter (the cover CD's executable is plain); the English one
  cannot be restored without undoing SecuROM's encryption, which this project does not do (decision log).
- **The English data run on the German cover CD's executable.** The 1.04 executable reads `TECHMAGE.CFG`;
  `LANGUAGE = 1` (the English CD ships that file) selects English (`ENG`, `GER` and the texts of `Txt/Eng`). Run
  with the English CD's files and the cover CD's `TechnoMage.exe`, a new game starts in English with English
  dialogue (checked 2026-11-23). So the English variant takes its executable from the cover CD (a supplement).
- Other settings `TECHMAGE.CFG` knows: `SCREENSHOTS`, `HIGH-PRIORITY`, `GSM-SCALE`, `ZBUFFER`, `WINDOWED`,
  `RECORD-PATH`, `LO-RES`, `MOVIE-PATH`, `GORE`, `MOVIES`, `PROFILER`, `STAGE`, `LEVEL`, `SOUND`.

## Media

- `2004-03.BIN`: raw MODE2/2352 (user data at offset 24), ISO 9660, volume `TECHNOMAGE`, created 2004-01-29.
  archive.org md5 `d14c2fbcb016725ac17ad7e45f05b06b`. Redump 38144 lists a different md5 (likely another write
  offset; not relevant, the archive.org file is pinned).
- Its setup: InstallShield 7 (`data1.hdr` version field `0x01007000`: major 7), `setup.inx`, `engine32.cab`,
  `data1.cab` 267 674, `data2.cab` 173 003 611. Version 7 keeps version 6's layout: the reader extracts every file
  and checks its MD5, except 20 `Sfx/*.SBK`.
- **The archived image is damaged there.** The EDC of every sector was checked: 1 608 of the 359 361 sectors fail
  (sectors 332 689–346 317, all inside `DATA2.CAB`, mostly in pairs every 22 sectors; some have a damaged
  subheader that marks them Form 2). Each has 30–76 bad P and 39–48 bad Q codewords, far beyond what the
  sector's ECC can correct. The damaged range holds exactly the 20 sound banks `CMine.SBK` … `Steamer.SBK`
  (file indices 154–173). Everything else on the disc (the other cabinet files, `Mov/`, `Mus/`, `Spc/`) is
  intact.
- **Where the 20 sound banks come from instead.** Sound banks are language-neutral: all 29 `Sfx/*.SBK` of the
  English CD match the MD5s the German cabinet records. The German retail CD 1.00 (archive.org item
  `technomage-die-rueckkehr-der-ewigkeit-sunflowers-2000`, `TechnoMage.bin` 819 039 312 bytes and a cooked
  `TechnoMage.iso` 713 177 088 bytes) holds 28 of them byte-identical; only `HBoss.SBK` differs, which patch 1.04
  changed. archive.org's "view contents" serves the ISO's files one by one, also through `/cors/` with CORS
  headers (checked 2026-11-23): `…/TechnoMage.iso/data1.hdr` (25 875 bytes) and `…/TechnoMage.iso/data1.cab`
  (167 969 179 bytes, InstallShield 5, `DATA.TAG` `Version=1.00.000`) are all that is needed, 167 MB instead of
  two more CD images. Patch 1.04's `HBoss.SBK` matches the German cabinet's MD5, but the patch is a RAR SFX (RAR
  2.0 method, solid), from which archive.org serves no single files: **decided** to take `HBoss.SBK` from 1.00
  (decision log). The sound bank of the Hive's boss is therefore 1.00's in the German build.
- Other TechnoMage items on archive.org (search 2026-11-23): `technomage` (German, `TECHNOMAGE.zip` holding
  the same 1.00 image as a `.bin`), `tecnomage` (Italian), `technomage_pc-fr` (French), `Videogame_hgftssa`
  (Spanish), `TechnoMageRoE` (an English repack `Technomage_Setup.exe`, 520 MB, packed with Petite: contents
  **(open)**, and the German patch 1.04), `isbn_9785699398751` (Russian), `24_20240222` (soundtrack MP3s), the
  PlayStation version, magazine CDs with demos (PC Action 03, 05, 06/2001, Game On! 06/2001), and the scene
  and patch collections.
- The CD folders `Mov/` (17 Bink files), `Mus/` (≈70 WAV), `Spc/` (8 `.PCK`, one per world) are not installed;
  the game reads them from the CD (`MOV\%s.BIK`, `MUS\%s.WAV`, `SPC\%s.PCK`).
- `Disc.rar` (English): RAR 4 format, not solid, holds an Alcohol image (`TECHNOMAGE.mdf` raw MODE1/2352,
  `TECHNOMAGE.mds`, `CHECKSUM.md5`). archive.org serves each member on its own ("view contents"):
  `https://archive.org/download/TechnoMage_Return_of_Eternity_Europe/Disc.rar/TECHNOMAGE.mdf` redirects to
  `view_archive.php?archive=…&file=TECHNOMAGE.mdf`; through `/cors/` the answer carries
  `Access-Control-Allow-Origin` (checked 2026-11-22). The body is unpacked on the fly (no `Content-Length`, no
  Range), so the page streams it. No RAR reader is needed.
  The disc inside: InstallShield 5 (`DATA1.HDR`, `DATA1.CAB` 167 505 706; already read by `rc-media`), `DIRECTX\`,
  `MOV\`, `MUS\`, `SPC\` (English speech).
- archive.org CORS: `/download/` redirects to a data node without CORS headers; `/cors/<item>/<file>` answers
  200 with `Access-Control-Allow-Origin`, preflight allows `Range`, but the response ignores it (full body).

## The executable

- UPX 1.07 (`$Id: UPX 1.07` string, `UPX!` block: version 12, format 9 = win32/pe, method 5 = NRV2D_LE32
  **(to confirm)**). `upx -d` 4.2.4 restores it.
- Restored: linker 6.0 (MSVC 6), static CRT, sections `.text` (0x152998), `_GMCODE` (0x1abf), `.rdata`, `.data`
  (0x3a000 in the file, a virtual extent to `0x014fe000`), `.idata`, `_GMDATA` (0xb08e), `.reloc` (0x26778).
- `_GMCODE`/`_GMDATA`: a library linked into its own sections **(open)**.
- Imports: kernel32 81, user32 32, gdi32 2 (`GetDeviceGammaRamp`, `SetDeviceGammaRamp`), ole32 3
  (`CoInitialize`, `CoCreateInstance`, `CoUninitialize`), winmm 1 (`timeGetTime`), mss32 45, binkw32 10.
- DirectX: `ddraw.dll` and `dinput.dll` by `LoadLibraryA`; error strings for DirectDraw and Direct3D (immediate
  mode, execute buffers included: the D3DERR table of the SDK), "You need DirectX Version >= 7.0". The GUIDs of
  every DirectX 7 interface are present (the SDK's GUID library), so the interface level is confirmed by the
  `QueryInterface` calls **(open)**.
- Fog: strings `F_FOGPATCH`, `F_FOGPATCH2`, `F_FOGPATCH_N`, `FFOGPATCH` (level data names) **(open)**.
- CD check: `GetLogicalDrives`, `GetDriveTypeA`, `GetVolumeInformationA` are imported; the readme's FAQ has
  "Die TechnoMage CD-ROM befindet sich nicht im Laufwerk". The volume label is `TECHNOMAGE` on both CDs.
  Exact check **(open)**.
- Settings: `TechnoMage.ini` (binary, magic `M.K.`, holds the Miles provider name "DirectSound3D 7+ Software -
  Pan and Volume"), written by `TM_Setup.exe`. `TECHMAGE.CFG`, `MUSIC.CFG`. Debug paths `C:\TM_RECORD\TM`,
  `C:\TECHMAGE.REC`, `c:\error.log`.
- Threads: `CreateSemaphoreA`, `CreateMutexA`, `SetThreadPriority`, but no `CreateThread` import: threads may come
  from the CRT's `_beginthread` (static) or from Miles/Bink **(open)**.

## Data formats (only as far as the HLE needs them)

`.TPL` graphics, `.cmp` levels/maps, `.LVL`, `.RUL`, `.SBK` sound banks, `.PCK` speech, `.dlg` dialogue text,
`DATA.OVL`, `resource.cmp`, `default.pal`: all read by the game's own code; nothing to implement unless a
format needs a host service **(open)**.

## The pause menu, the options panel and the texts (for the project's patches)

- **Menus.** One menu engine draws the pause menu, the main menu and the load/save book. A menu is a table of
  16-byte entries (key of the text, type and flags, value, maximum) that ends with a NULL key, at most 16 entries;
  opening it (the function behind the thunk at `0x401393`, table first) looks every key up in the string table and
  asks an optional value-text callback for each entry. When an entry is chosen the engine calls the menu's second
  callback with the entry's index; an action entry closes the menu first. Both callbacks work by index.
- **The pause menu** is the table at `0x563ee8`: resume, options, input devices, the logo animation and back to
  the main menu; its callbacks are `0x450a60` (the animation's name as value text) and `0x450b80` (options sets
  bit 1 of the pause state `0x563c84`, input devices bit 2, the animation plays, back ends the level). After the
  end entry follow a debug line and two cheat entries (invulnerability, start in level) that a developer flag puts
  in by writing the end entry's key. The game reads some of the table's values by absolute address (the
  animation's choice), so entries cannot be moved in place.
- **The options panel** is not a menu of that engine but its own panel with 36-byte entries at `0x564c80` (sound,
  music and speech volume, gamma): position, label offset, a type (1: label, number and slider; other types draw
  nothing), selectable, value and maximum, a callback on change, the label text and a 4-byte number buffer. It is
  set up once per level (behind the thunk at `0x4025f9`) and drawn in every frame of the pause (`0x4011b8`),
  sliding in while bit 1 of the pause state is set; entries are drawn by `0x460a30`. Its title is the text of
  `OPT_OPTIONS` (pointer at `0x59fb80`). Escape in the panel resumes the game.
- **Texts** come from `Txt/<language>/GLOBALS.DLG` (and one file per world): a header (size, count) and pairs of
  NUL-terminated key and text in Windows-1252. The lookup by key is behind the thunk at `0x402252`. The language
  is the index at `0x59de18` into `GER`, `ENG`, `FRA`, `ITA`, `SPA`, `NL`. Text is drawn by `0x4010d7` (font,
  text, x, y, size, colour, …, flags whose low bits align it: 0 from x on, 1 ending at x).
- **Thunks.** The executable was linked incrementally: most calls go to a jump thunk near `0x401000`, and the code
  map merges a thunk with its target into one function entered at the thunk. Hooks therefore name the thunk's
  address (`[binary.patch]` refuses an address that is not a function entry).

- **Resolution.** The game's screen size is data (`0x55b7bc`/`0x55b7c0`, 640×480; 320×240 with a developer flag);
  the graphics start (`0x52e0f0`) takes it with the depth, and the device's viewport is copied to `0xf5b318`
  (width and height at `0xf5b320`/`0xf5b324`, which the menus lay out on in thousandths). The projection's centre
  is half of it, its scale fixed in pixels (`0x14fc7a8` and on); Bink videos are centred in 640×480 (`0x53c9e3`).
  Run at 1024×768 (set before the start), the camera shows more of the level, the videos sit top left and the
  fonts and HUD keep their pixel size. The HLE offers 640×480, 800×600 and 1024×768; another size ends the start
  with a DirectX error.
- **2D with Direct3D.** The dialog windows (frames, text, portraits) and other panels are Direct3D triangles drawn
  pre-transformed, axis-aligned, one texel to one screen pixel (some 1.005 to 1.013); the game locks the whole
  back buffer several times a frame for its other 2D work, never `Blt`s to it. At native resolution such triangles
  are sampled at the centre of the screen pixel each drawn pixel belongs to (`Vertex::exact`), else texel edges
  show as lines and the colour key (cyan) bleeds in.
- **Whole-pixel vertices.** The 3D renderer's batches (flushed at `0x531851`/`0x531ef5` from a buffer at
  `0x8055a0`, FVF `0x144`) take their screen positions from 16-bit integers (`0x5320f0` and on): every vertex
  lies on a whole pixel. Meshes that meet do not quite (a wall's black base and the floor's, a pixel apart): at
  640×480 the gap hits a pixel centre now and then, at native resolution it is a line through which what lies
  behind shows. The renderer then draws a rim half a screen pixel wide around every opaque triangle that writes
  depth, its depth pushed back, so it only fills such gaps. And with pixel centres on whole coordinates, what is
  drawn from 0 on starts half a screen pixel into the larger picture: its first column and row are copied from
  the next.
- **The options panel's keys.** Up and down choose among the selectable entries (`0x810c74`), left and right
  change the selected one's value between 0 and its maximum. Enter and Space do nothing there; the press that
  chose the panel in the pause menu is still down for a few frames when it opens. The pause menu's mouse click
  arrives (DirectInput) a frame before the game's own mouse position (`0x59cb14`/`0x59cb16`, from `WM_MOUSEMOVE`).
- **The mouse pointer** is a small 3D object (`MOUSEPOINTER`, with the glow `FX_SHOCKWAVE_02`), put into the frame
  once per frame at the mouse position (`0x401dc0` called at `0x42818c`) while bit 1 of `0x55b7b8` is set and bit
  31 is not: the game sets bit 1 after more than five mouse moves when the mouse is in use (bit 0); what sets bit
  31 is **(open)**. In the menus it sways (a sine of a time at `0x5a8268`) and has its tip at the mouse; in a
  level it turns towards the hero and is centred on the mouse. The game's mouse position (`0x59cb14`/`0x59cb16`)
  comes from `WM_MOUSEMOVE`, absolute, so a system cursor and the game agree.
- **The ordering table.** 3D objects are not drawn when they are placed: their screen-space primitives (0x3c
  bytes, from an arena whose free pointer is `0x59de10`) are chained into an ordering table of 0x400 buckets by
  depth (`0x795840`, 0x4000 bytes per frame buffer, the buffer at `0x59cb1c`; each bucket a 16-byte empty node,
  chained from the last to the first). `0x530e60` draws the table from its last node once per frame and `0x530d10`
  empties it. The renderer groups the primitives of a run by kind (untextured, textured, additive) before it draws
  them. The pointer is three groups: a black outline (the silhouette moved by a pixel), the textured stone at the
  same depth, the additive glow just behind it.
- **Software rasterizer, fixed with the pointer:** a pixel on the edge two triangles share was drawn by both (no
  top-left rule; a seam in additive quads), and depth was stored truncated and compared unquantised, so a second
  pass at the same depth (outline, then stone) failed the less-or-equal test.
- **(open)** Once in a scripted run in a window on the VMware VM, the guest thread panicked inside `khronos-egl`
  (an `unwrap` on `None`), i.e. in the Direct3D renderer's OpenGL path; not seen again in four runs of the same
  script. The renderer picks its adapter as the window did before (all backends, SVGA3D's OpenGL here).

## Behaviour seen in scripted runs

- After a new game the first room opens with tutorial dialogues that `Strg` (Ctrl) scrolls and closes; the
  first ones teach `Z`/`X` to turn the view.
- `Esc` in the game opens a pause menu (resume, options, input devices, the logo animation, back to the main
  menu) without saving. The main menu offers a new game, loading and the way back to Windows.
- Loading and saving share one book: nine slots and a quick-save slot (`SCHNELLSPEICHERN`), with load, save,
  delete and back. In the game it is reached through the HUD's load/save entry (the control card,
  `Steuerung.pdf`, shows it beside the quest log). Which key opens it, and whether saving is free or tied to
  places, is **(open)**: the card's key legend is a picture.
- "Zurück zu Windows" ends the process through `ExitProcess(0)` and writes `TechnoMage.ini` (378 bytes) into the
  user directory; the next start reads it.
- Files on the CD: videos from `D:\MOV\`, music from `D:\MUS\` (by `Music.cfg`), speech as `SPC\<WORLD>.PCK`
  in the install directory first and then `D:\SPC\<WORLD>.PCK` (one archive per world), which the virtual
  drive answers; so the speech question is settled. `TECHMAGE.CFG` is optional (the German release has none),
  `C:\TECHMAGE.REC` and `MATRIX.TGA` are looked for and not needed.
- A soak of 20,000 frames (clicks and keys every 15 to 40 frames in the first room) ran without a trap or an
  unimplemented call, natively at about 65 frames a second.
- The window's backend: Bevy 0.20 by default tries every wgpu backend at high performance. In a virtual machine
  with VMware's SVGA3D OpenGL driver and Mesa's lavapipe (software Vulkan) it chose OpenGL, whose surface for the
  X11 window offered no present modes, and Bevy panicked creating it ("Fallback system failed to choose present
  mode"); lavapipe works. The platform layer therefore takes the primary backends (Vulkan, Direct3D 12, Metal)
  when one of them has an adapter, OpenGL only otherwise, and `WGPU_BACKEND` overrides it. Whether Bevy's OpenGL
  window works on real hardware under X11 is **(open)**. The Direct3D renderer (`rc_platform::gpu`) has a wgpu
  instance of its own, on all backends and deliberately deaf to `WGPU_BACKEND`: in that virtual machine it takes
  OpenGL, about three times as fast as lavapipe there (60 against 20 frames a second in the first room), also when
  the window needs `WGPU_BACKEND=vulkan`.
- Where a frame's time goes (2026-11-26, first room, the virtual machine above, gdb stack samples): at `Flip` the
  collected draws render on the GPU over the back buffer and come back, so each frame converts the 16-bit back
  buffer to RGBA8, uploads it, reads the result back (synchronously), packs it to 16 bits again and converts it once
  more to show it. The conversions were four divisions per channel and pixel and about half the frame; lookup
  tables (a 64 Ki-entry table per set of 16-bit masks, 256 entries per channel back) and sharing decoded textures
  instead of copying them every draw took the run from 35 to 63 frames a second (CPU time −56 %, frames
  byte-identical). What remains is mostly the submit and the read back (half the samples) and the game's own code.
