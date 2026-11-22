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
- English CD mastered 2001-05-03, after 1.04: its version and protection **(open)**, measured from `DATA.TAG` and
  the executable in M0.

## Media

- `2004-03.BIN`: raw MODE2/2352 (user data at offset 24), ISO 9660, volume `TECHNOMAGE`, created 2004-01-29.
  archive.org md5 `d14c2fbcb016725ac17ad7e45f05b06b`. Redump 38144 lists a different md5 (likely another write
  offset; not relevant, the archive.org file is pinned).
- Its setup: InstallShield 7 (`data1.hdr` version field `0x01007000`: major 7), `setup.inx`, `engine32.cab`,
  `data1.cab` 267 674, `data2.cab` 173 003 611. SackedRecomp's version-6 reader, with the version check widened,
  lists every file and extracts all but 20 `Sfx/*.SBK`, whose inflate fails part-way **(open)**: compare the
  descriptors (flags, volume, offsets) with version 6's; a file split across the two cabinets is the first
  suspect.
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
