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
- **French, Italian and Spanish run the same way** (checked 2026-11-27): their CDs were mastered like the English
  one (InstallShield 5, `DATA.TAG`, `Txt/<lang>`, `Spc/` in the language; French and Italian 2001-05-02), each
  `Technomage.EXE` is 885 379 bytes of UPX inside SecuROM, and each ships `TECHMAGE.CFG` with `LANGUAGE = 2`
  (`FRA`), `3` (`ITA`) or `4` (`SPA`). With the cover CD's executable a new game starts in the language with its
  dialogue. Their `TM_Setup.exe` and `TM_Diagnostic.exe` differ per language, which is how the converter tells
  them apart (their `binkw32.dll` and `Mss32.dll` are the German ones).
- **The Spanish CD is packed**: a tar split into two parts holding a single 7z (LZMA2 with a 16 MiB dictionary,
  one solid folder of five files, the header LZMA-compressed), which holds the ISO. archive.org's "view contents"
  does not open the tar parts, so the image cannot be fetched on its own; the 7z must be decoded from its start
  up to the end of the ISO (731 MB of output, about 40 s natively). `rc-media`'s `packed` reader joins the
  parts, finds the 7z in the tar and the largest CD image in the 7z, and keeps that window of the output.
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
- Other TechnoMage items on archive.org (search 2026-11-23, again 2026-11-27): `technomage` (German,
  `TECHNOMAGE.zip` holding the same 1.00 image as a `.bin`), `tecnomage` (Italian), `technomage_pc-fr` (French),
  `Videogame_hgftssa` (Spanish) — the three now variants (TARGETS.md) —, `TechnoMageRoE` (an English repack `Technomage_Setup.exe`, 520 MB, packed with Petite: contents
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
- **Widescreen.** Every frame of a level the main loop (in `WinMain`, `0x44fc50`) calls, among others, the
  functions that put the world's primitives into the ordering table: the level's geometry (`0x40148d`, most of
  them), objects and effects (`0x401ff0`, `0x4025ef`, `0x401721`); then the camera (`0x4010b9`: the projection's
  centre `0x14fc7a0`/`0x14fc7a4` is half the screen size less the camera's shake); `0x4021da`, `0x401357`,
  `0x40201d` read the screen size to tell which objects are on it and add no primitives. They cull against the
  screen size `0xf5b320`; the camera's distance is 416 at a width of 640 and 512 at any other (`0x40fac0`, into
  `0x5913b4`), which is why a larger display mode zooms out. The ordering table's draw (`0x530e60`) drops a
  primitive wholly beyond the screen size as well. The primitives are PlayStation GPU packets after an 8-byte
  link: the command byte at `+0xf` (`0x20`–`0x3f` polygons: bit 3 a quad, bit 2 textured, bit 4 Gouraud, the next
  vertex's colour before it; `0x40` lines; `0x60` rectangles), the first vertex at `+0x10` as two `i16`. Menus,
  texts and panels go to buckets 0 to `0x1b`, the world from `0xf8` on (seen in Dreamertown's first rooms,
  menus, inventory, map). The game never clears the depth buffer while playing: the frame's first primitives,
  four Gouraud quads of a dark gradient over 0 to 640 in bucket `0x3ff`, set it, so a wider picture needs them
  wider, or what lies beyond 640 keeps old depth and stays hidden. Widened, their gradient would show beside
  rooms whose black surroundings end before the wider picture's edge, so widescreen draws them black. The options panel's table has room for a
  fifth entry: the 36 bytes after its end (`0x564d34`) are zeros up to the next data. Videos: the handle of the
  one playing is at `0x808ce8` (width and height first), centred in 640×480 by `0x53c8d0`; the cut scenes are
  640×368, the logos and visions 640×480. The videos (German CD, all 16 measured over every frame) are of two
  kinds: `Intro`, `Outro` and `Cut1` to `Cut7` are 640×368 and the game letterboxes them on its 640×480 screen,
  with no black bars in their own frames (at most 2 to 8 dark rows); `SFLOGO`, `LOGO` and `Vision1` to `Vision5`
  are 640×480, full frame (`LOGO` black only in a strip 22 pixels wide on its left). So widescreen cuts only a
  letterboxed one (lower than the screen) to fill the window, by at most an eighth; a full-frame one is shown
  whole. Whether the other languages' videos are the same sizes is **(open)**. Whether every world keeps its primitives in those buckets is **(open)**.
  The interface's buckets: the life and mana tubes 12 to 15 (bottom left; in a dialogue they slide out below),
  a dialogue 24 to 27 (its text 25, its box 26 and 27, key pictures 24). In the character menu (state `0x40`)
  the attributes, the coins and the weapon slots are left of the middle (buckets 7, 23, 24, …), the rucksack's
  frame right of it (10, 11), the description below in 25 to 27; nothing crosses the middle. A camera move with
  a cut scene's black bars draws them over 0 to 640 like the depth background. Widescreen keeps the tubes at
  the left edge in every state (the pause menu too), splits the character menu to both edges while it is open
  and for 40 frames after it closes (it slides out while the game is played again; its description, with the
  key pictures in its text, stays in the middle), maps the pointer back (`screen_to_game`), and widens what
  covers the whole 640 pixels (bars, fades). The level's geometry is drawn for a square of tiles around the
  camera, its radius set per level (`0x4025a4`, argument 12 in the house, doubled into `0x55f420`, also
  `(r-3)<<11` into `0x97e18c`): on a wider screen the square's corner shows on the right as floor cut off in
  steps (nothing beyond x ≈ 700 reaches the ordering table). Widescreen enlarges the argument by the screen's
  share, at most by 4: from 17 on the level's geometry falls apart (its buffers are made for 16; from 19 on
  `(r-3)<<11` also overflows its 16 bits), so at 16:9 the room is complete but at 21:9 a corner on the right
  still shows. The tubes' buckets 12 to 15 also hold the pause menu's title and the load and save book's
  question (the book draws no world): only what lies in the bottom left corner keeps to the edge, and only
  while the world is drawn. A rectangle in the
  background bucket (the input page's dark tile over the 640 pixels) is widened by its width as well: beyond
  it nothing clears the screen, and the keyboard tab's long names stayed beside the gamepad tab's. Three functions the widened world functions call ask the width: texture
  scrolling (`0x401654`) and an object's sprite animation (`0x402135`) whether it is 640 (else the 320-pixel
  mode's half steps), and the sprite of the hero's body (`0x401910`) is sized by `320 / SCREEN_W`, so on the
  wider screen he shrank by its share, smaller than his cape; they run with the game's width. The input
  (`0x402504`) compares the pointer (the game's pixels) with the hero's place on the screen (`[0x59142c]` +0x38,
  +0x3a), which the widened projection puts a margin further right: the pointer's centre was right of the
  hero; the input sees his place a margin to the left. On the right a hard edge at about x 700 of the screen
  surface remains in the house at 16:9 whatever the tile radius (nothing beyond reaches the ordering table):
  its cause is **(open)** (a visibility table made for the 4:3 view?). Many functions also branch on
  `SCREEN_W == 640` (else the 320-pixel mode's halved steps, e.g. texture scrolling at `0x405f69`); inside the
  widened world functions they take the 320 path: harmless so far **(open)**. The hero off the middle could not
  be reproduced (in the middle at 16:9 and 16:10, standing, after walks) **(open)**.
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

## The controls

- **The PC version reads everything into a PlayStation pad.** `0x537df0` (once per frame, from the main loop at
  `0x44ff7d`; its result goes to `0xee93a4`, the buttons pressed with the game's repeat) turns the keyboard (by a
  table of DirectInput scan code and pad bits, `[0x8088dc]`), the mouse and a DirectInput joystick (`[0x8088cc]`,
  read as `DIJOYSTATE2`) into the PlayStation pad's bits: L2 `0x01`, R2 `0x02`, L1 `0x04`, R1 `0x08`, triangle
  `0x10`, circle `0x20`, cross `0x40`, square `0x80`, select `0x100`, L3 `0x200`, R3 `0x400`, start `0x800`, up
  `0x1000`, right `0x2000`, down `0x4000`, left `0x8000` (as PsyQ's `PADL2` … `PADLleft`). The actions are these
  bits: the input page's entries (`0x55e8f8`, 0x50 bytes: key, text, bits at `+8`, two bindings of device and code
  at `+0x2a` and `+0x4c`, their names at `+0xa` and `+0x2c`) are action `0x80`, jump `0x40`, spell `0x10`, tool
  `0x20`, camera left `0x01` and right `0x02`, rucksack `0x100`, map `0x08`, quest log `0x04`, the directions, and
  "walk to the pointer" `0x5000` (up and down at once, a value no pad gives). The keyboard's default table
  (`0x581908`) also has Escape for start, `^` and A for L3 and R3. Choosing an entry or a menu line takes square
  (`0x80`); Enter is bound to square and cross together (`0xc0`).
- **The joystick**: the game opens the first DirectInput joystick at its start (`0x538480` from `0x426a5e`): data
  format `c_dfDIJoystick2`, capabilities (with force feedback it makes an effect), `DIPROP_RANGE` of the X and Y
  axes, then `Acquire`, `Poll` and `GetDeviceState` each frame. The axes are the directions (beyond a third of the
  range from the centre); the POV is not read. Its buttons go through a table (`[0x8088e0]`, set by `0x538790`;
  entries of button, a byte, the pad bits) that `0x401195` builds when the input page's bindings are applied from
  those on device 3 ("Joy N", `KFG_JOY_BUTTON`): none by default, so a joystick gave only directions. The table
  the game had before that (`0x58194c`) numbers the buttons as square, triangle, cross, circle, L2, L1, R2, R1;
  the HLE's joystick reports the gamepads in that order, with select, start, L3 and R3 after them.
- **Force feedback**: with a joystick that has it (`DIDC_FORCEFEEDBACK`), the game sets `DIPROP_AUTOCENTER` and
  makes one effect, a constant force (`GUID_ConstantForce`), kept at `0x8088d0`. Each frame `0x518c8b` sums the
  forces of the moment (each axis clamped to -127 … 127, scaled by `[0x58046c]`) and `0x5387b0` sets the effect's
  direction and magnitude (their length, up to 10 000) with `DIEP_START` (`SetParameters`, flags `0x20000140`):
  zero in a quiet room (seen in a scripted run), more when the hero is hit or the ground shakes. The page's
  "Force Feedback" switch (`0x55b828`) turns it off. A gamepad vibrates with that magnitude.
- **Gamepads on Android** reach a NativeActivity only as input events on its one queue (there is no API to ask
  a pad's state, `/dev/input` is closed to apps, Google's GameController library is C++). Bevy's window (winit
  0.30 through android-activity 0.6) reads that queue: it passes the buttons on as keys without a code of its
  own (`AKEYCODE_BUTTON_*`) and turns every motion event into touches, the axes lost, and reports all of them
  handled, so Android does not make directional keys of the sticks either. android-activity takes each event
  with the NDK's `AInputQueue_getEvent` (through the `ndk` crate); the shell's library defines that function
  itself, which the call binds to when the library is linked `-Bsymbolic-functions` (otherwise a `JUMP_SLOT`
  the dynamic linker might resolve to libandroid's), takes the events whose source is a gamepad or joystick
  (`AINPUT_SOURCE_GAMEPAD`, `_JOYSTICK`: the buttons, `AXIS_X`/`_Y`, `AXIS_HAT_X`/`_Y`, the triggers
  `AXIS_LTRIGGER`/`_RTRIGGER` or `_BRAKE`/`_GAS`), finishes them as handled and gives winit the next one. Not
  tried on a device yet.
- **The PlayStation version** (SLES-03243, Italian, archive.org) offers four controller set-ups in its options; the
  manual (Spanish, `all-ps1-rpg-manuals`) shows A: triangle spell, circle tool, cross jump, square action, L2 and
  R2 the camera, L1 the quest log, R1 the map, select the character menu, start the game menu, the analog stick as
  the directional buttons. Its executable's table at `0x80097a70` has the four, by the button's bit: B swaps
  triangle with circle and cross with square, C swaps L2 with R2, D does both.
- **Key pictures in texts.** Dialogue and menu texts name keys with characters the text renderer (`0x4010d7`)
  draws as pictures: `§` the action, `=` the jump, `~` the rucksack, `_` the map, `>` the quest log, and more.
  `0x40105f` makes their ten sprites (`0x14dfa60`: action, spell, tool, jump, camera left and right, quest log,
  map, `CHECKBOARD`, rucksack, in the order of `0x55eefc`) from each action's first binding: a key as `BT_<name>`
  of `GFX\HI\KEY_<language>.TPL` (a key cap per key), a mouse button `MOUSE_L`/`_M`/`_R`, a joystick button
  `PADDLE_<n>`. The key sheets of the later CDs (English, French, Italian, Spanish) have `PADDLE_01` … `_16`, a
  generic pad with a number; the German cover CD's has none. The PlayStation's `GFX/KEY_PSX.TPL` holds its own.
- **Sprites.** `0x401465(name)` finds a loaded sprite: 12 bytes, four `0xff`, its texture page and mode (two
  bytes), its palette slot (after loading; in the file a page), u, v, width, height. The game draws one
  (`0x445830`, the input page's switches) as two primitives in a bucket: a mode primitive (`0x80`, the word
  `((mode & 0xfff8) | page) << 9 | 0xa0` at `+0x10`), then a sprite (`0x65`: x, y, u, v, palette `slot << 6`,
  width, height), from the arena at `[0x59de10]`; the tables are at `0x795840 + ([0x59cb1c] << 14)`. The level
  pictures (`GFX\HI\<level>.TPL`) hold the character menu's icons (`ICON_MAP`, `ICON_QUESTLOG`, `ICON_LSM`, the
  spells, tools and weapons); their `BT_ACTION` … `BT_INV` are key caps again. A `.TPL`: a page count, one
  palette (768 bytes), the 8-bit pages (256×256), the same pages again in 16 bits, then name tables (count,
  size, names each with a length byte before it) each followed by its records; the records' page numbers are not
  simply the file's pages (not needed: the game's own lookup and drawing are used).
- **The game's state** at `0x563c84` is `0x800` with `[0x59de34]` 0 while it is played; a dialogue adds `0x10`,
  the map is `0x880`, the rucksack `0x40` (both with flags 2), the pause menu flags 1.
- **The input page** (`0x401140`, called each frame while bit 2 of `0x563c84` is set) draws its entries in three
  columns (50, 300 and 630 thousandths of the width; lines `[0x59d6dc]` apart from 30 thousandths), two switches
  below, and has click regions named `OK`, `CANCEL`, `DEFAULT`, `WHEEL`, `FORCE`. `0x59d72c` is non-zero while it
  waits for a key to bind; it is left by the pressed button `0x100` (Escape and start reach it so) and applies the
  bindings (`0x401195`). Its font has no `/`, `&`, `<`, `>`, `[`, `]`, `=` or `*` (it has `+`, `(`, `)`, `:`,
  `#`, `-`). `0x59cb10` holds the buttons held. Its background, put first at the next free primitive
  (`0x59de10`): a dark tile over the screen (0x18 bytes, the background bucket) and two Gouraud quads (0x34
  bytes each) of a white-to-cyan and a white-to-yellow gradient behind the bindings' columns; the patches make
  the two quads cover no area (the maintainer's call: the page without them).

## Saved games

A saved game (`Saves\TMN_NN.TSG`, slots 1 to 10) **is** the game's persistent state block, 30,232 bytes
(`0x7618`) at the pointer in `0x558da0`: before saving, `0x451930` packs the live state into it; loading reads
the file whole (it only checks the length) and the game applies the block on entering the level (`0x451bb0`,
`0x4511xx`–`0x451709`). No checksum, no magic. The `tmsave` editor (`games/technomage-save`, DEVELOPMENT.md)
describes every byte; its schema (`src/schema.rs`, `src/worlds.rs`) holds the details, its checks (`src/check.rs`)
the rules below. Layout:

| Offset | Contents |
|---|---|
| `0x0000` | Header, only for the load/save book: picture width/height (160, 120: the book shows the picture only if they are the screen's quarter), world, stage, HP, max HP, mana, max mana, crystal bits (items 52..56 owned), play time; copies of the fields below |
| `0x0014` | The picture, 160 × 120, RGB 3-3-2 |
| `0x4b14` | 256 × u32: the count of each entry of the item table (`0x558f30`, 163 named entries: weapons, spells, defence, tools owned when not 0; ammunition, potions, keys, chalk 0..99; `GOLDCOIN` the gold; quest items). Loading adds each through the game's add function (so the rucksack hint shows), then sets the saved count unclamped |
| `0x4f14` | 27 × i16: the rucksack grid (3 × 9), item indices, −1 empty |
| `0x4f4a` | Heading, camera yaw (12-bit angles), position x/y/z (2048 units a tile), the `SPAWN` point, the torch (bit 31 lit, burn time in 1/480 s), the magic shield |
| `0x4f70` | A bit per level slot of the current world that holds state; 22 slot records of `0x18c` bytes: moving platforms, automap sections and markers, nine group counts and a 256-bit field (triggers fired, doors open, creatures alive, pickups present, levers, magic torches, stars, traps, webs; the counts are the level's), 64 crates, the return position, 20 chalk marks. The slot is byte 5 of the level table entry; changing worlds clears them all |
| `0x717c` | 16 creature records and 40 person records of the current level (applied only when loading) |
| `0x738c` | The character: experience (level L needs the table's L-th value: 10,000 for 2 … 1,000,000 for 20), pending experience, HP, max HP (= 5 CON + 6 level + 4), poison, mana, max mana (= 10 INT + 5), timers, level 1..20, STR/INT/CON/MYS 0..40 |
| `0x73b4` | The selected weapon, spell, tool and defence item (1-based in their ring, 0 none), the rucksack cursor, world 1..8, stage, entry point, map variant, the furthest story video seen, play time (1/3840 s) |
| `0x73c4` | The current world's script state, up to 256 bytes, its layout the world's own (DREAMER `0x94`, STEAMER `0xbc`, THEHIVE `0x17`, FOREST `0x100`, CANYON `0xbc`, TOWER `0x88`, VULCANO `0xd8`, RUINS `0xf8`): timer slots, shop stock, object snapshots, quest stages |
| `0x74c4` | The quest log: 16 × {key without `QUESTLOG_`, 15 bytes; done bit; a text pointer the game renews}, the count at `0x7604` |
| `0x7608` | 4 × u32 cheat markers (a bit per cheat used; never read) |

What breaks the game (the checks' errors): a world outside 1..8 or a stage without a level; level 0 (a division
by zero in the stats screen) or above 20 (past the experience table); max HP below 2 (divisions by zero); a
rucksack entry past the item table; a selected weapon, spell, tool or defence item that is not owned (the
loader looks it up without a bound). Checked in the game (2026-11-27): a saved game changed with `tmsave` (gold,
potions in the rucksack, a sword selected, level 5 with its attributes, HP and mana, a crystal) shows the new
values in the book, loads, and plays with them. **(open)**: the platform state bytes, the polarity of the lever
and magic torch bits, the pickups' second bit, the person records' last two bytes, the meaning of most world
script bytes beyond the code's use (named by the identifiers the scripts use beside them), and whether CANYON's
`+0x7c..0x9f` and VULCANO's `+0x4b..0x88` are read at all.

## Behaviour seen in scripted runs

- After a new game the first room opens with tutorial dialogues that `Strg` (Ctrl) scrolls and closes; the
  first ones teach `Z`/`X` to turn the view.
- `Esc` in the game opens a pause menu (resume, options, input devices, the logo animation, back to the main
  menu) without saving. The main menu offers a new game, loading and the way back to Windows.
- Loading and saving share one book: nine slots and a quick-save slot (`SCHNELLSPEICHERN`), with load, save,
  delete and back. A saved game is `Saves\TMN_NN.TSG` (30,232 bytes): a 20-byte header, then its picture (160 ×
  120, one byte a pixel, RGB 3-3-2, 19,200 bytes), taken a few frames after the book opens (`0x44fbe0`, counter
  `0x59de14`) by locking the *front* surface and taking every fourth pixel (`0x537390`). The HLE's `Flip` therefore
  copies the frame shown into the front surface, as a real flip leaves it there; before, the picture was black.
  With widescreen it is taken from the surface's left 640 pixels, so it sits a margin to the left **(open)**. In the game it is reached through the HUD's load/save entry (the control card,
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
