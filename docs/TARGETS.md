# Targets

The releases this project supports, pinned by hash. The manifest (`games/technomage/game.toml`) is the machine
source; this page is the overview. Both are abandonware archived on archive.org; the converter downloads them
from the fixed URLs below.

## Media

| Variant | Release | archive.org item | File | Size | SHA-256 | Version |
|---------|---------|------------------|------|------|---------|---------|
| `cd-de` | SCREENFUN 03/2004 cover CD (German full version), volume `TECHNOMAGE`, 2004-01-29 | [`SF2004-03`](https://archive.org/details/SF2004-03) | `2004-03.BIN` (raw MODE2/2352) | 845 217 072 | `c9db7f91ae3d7755b88f9921c0904ee9f335463c4dde700c779454bd918e8ed7` | **1.04** |
| `cd-en` | European English CD, volume `TECHNOMAGE`, 2001-05-03 | [`TechnoMage_Return_of_Eternity_Europe`](https://archive.org/details/TechnoMage_Return_of_Eternity_Europe) | `TECHNOMAGE.mdf` inside `Disc.rar` (raw MODE1/2352, Alcohol) | 840 680 064 | **(open)** (md5 `4c863d6e4e45f5c8f5deffd2e8999e48`) | **(open)**, after 1.04 by date |

Download URLs (fixed; the page uses `/cors/` instead of `/download/`):

- `https://archive.org/download/SF2004-03/2004-03.BIN`
- `https://archive.org/download/TechnoMage_Return_of_Eternity_Europe/Disc.rar/TECHNOMAGE.mdf`

The `.cue` (`2004-03.CUE`, `MODE2/2352`) and the `.mds` are not needed.

The English CD is archived as `Disc.rar` (794 064 172 bytes: `TECHNOMAGE.mdf`, `TECHNOMAGE.mds`, `CHECKSUM.md5`).
archive.org serves the members of an archive on their own (the item's "view contents" list; `/download/<item>/
Disc.rar/<member>`, also through `/cors/`, with CORS headers), so the image is downloaded directly and **no RAR
reader is needed**. The `.mdf` is one Mode 1 track of 357 432 raw sectors, read by `rc-media`'s raw CD reader.

## Patches

| Patch | Language | Date | archive.org | Size | SHA-256 | Needed |
|-------|----------|------|-------------|------|---------|--------|
| 1.02 | German | by 2001-01-28 | not archived | ≈2.5 MB | – | no (superseded) |
| 1.04 | German | 2001-03-23 | [`TechnoMageRoE/technomage_patch104_ger.exe`](https://archive.org/download/TechnoMageRoE/technomage_patch104_ger.exe) | 7 392 269 | `5ccb0d8e97378f3578d02fbf21c718bb6b5447493b9d230ed6e80830322d2b36` | **no**: `cd-de` is 1.04 already |

There is no English patch (the official English download page listed only the German 1.02). The English CD was
mastered after 1.04; whether it carries 1.04's fixes is measured in M0 (its `DATA.TAG`, the executable's link
time). The patch 1.04 is kept as a **reference** (it identifies the German build), not as an input.

## Binaries

| File | Variant | Size | SHA-256 | Notes |
|------|---------|------|---------|-------|
| `TechnoMage.exe` (packed) | `cd-de` | 574 976 | `6f2521de92e4fb88fb1b22130154f32566a63a560884061b4e7e3b703a9d5f0a` | UPX 1.07, linked 2001-03-19 18:43:50 |
| `TechnoMage.exe` (restored) | `cd-de` | 1 863 680 | `a1201ab89542093b3ca312840fc44c216d62cdb6146e3ca3b29925ceac68ba1a` | what `upx -d` writes; MSVC 6, base `0x400000`, `.reloc` |
| `TechnoMage.exe` (patch 1.04) | reference | 885 891 | `224e72465b4d62aa91e07548e0989f034ae24cb9b61feb84fa77bbadd7280a48` | same link time, plus SecuROM sections `.cms_t`/`.cms_d`: not a target |
| `binkw32.dll` | `cd-de` | 261 120 | `545f402be018bc905c03305322072e194d77b1a5ec20b62c34cdbc4c81c961fa` | Bink 1 (2000-05-04), lifted |
| `Mss32.dll` | `cd-de` | 349 184 | `24c0a90dbff552102c09f671853e7247cf20de6e845254d5db625de961e4ae94` | Miles 6 (2000-10-23), HLE |
| `TM_Setup.exe` | `cd-de` | 246 784 | `37ad65019922b2d448e45d2c0bef77724ad09adea1b25f0f6b05cf1e4203cacb` | options program, skipped |
| `TM_Diagnostic.exe` | `cd-de` | 85 504 | `ea52a8f440d5e0601448510ce17237fbc2900b5a98847f5474de62475e37fd65` | install checker (new in 1.04), skipped |
| English binaries | `cd-en` | **(open)** | **(open)** | measured in M0 |

## Not targeted

- German retail CD 1.00 (2000-11-07, SecuROM 4.10, redump 7367) and its patches: the cover CD is the same game at
  1.04 without SecuROM.
- Other languages on archive.org (Italian `tecnomage`, French `technomage_pc-fr`, Spanish `Videogame_hgftssa`),
  the repack `TechnoMageRoE/Technomage_Setup.exe`: not requested; they can be added by hash later.
