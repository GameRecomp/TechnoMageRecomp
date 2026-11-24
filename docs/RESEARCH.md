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
