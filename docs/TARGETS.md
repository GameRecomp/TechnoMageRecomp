# Targets

The releases this project supports, pinned by hash. The manifest (`games/technomage/game.toml`) is the machine
source; this page is the overview. Both are abandonware archived on archive.org; the converter downloads them
from the fixed URLs below.

## Media

| Variant | Release | archive.org item | File | Size | SHA-256 | Version |
|---------|---------|------------------|------|------|---------|---------|
| `cd-de` | SCREENFUN 03/2004 cover CD (German full version), volume `TECHNOMAGE`, 2004-01-29 | [`SF2004-03`](https://archive.org/details/SF2004-03) | `2004-03.BIN` (raw MODE2/2352) | 845 217 072 | `c9db7f91ae3d7755b88f9921c0904ee9f335463c4dde700c779454bd918e8ed7` | **1.04** |
| `cd-en` | European English CD, volume `TECHNOMAGE`, 2001-05-03 | [`TechnoMage_Return_of_Eternity_Europe`](https://archive.org/details/TechnoMage_Return_of_Eternity_Europe) | `TECHNOMAGE.mdf` inside `Disc.rar` (raw MODE1/2352, Alcohol) | 840 680 064 | `178a16bf56c4a2b65298be6202c25d8e0bf355b76018a7ab0d45da75e5786450` | after 1.04 (linked 2001-04-25) |
| `cd-fr` | French CD (*En quête de l'éternité*), volume `TECHNOMAGE`, 2001-05-02 | [`technomage_pc-fr`](https://archive.org/details/technomage_pc-fr) | `TECHNOMAGE.ISO` (cooked) | 732 323 840 | `69d3d23341cab17970654f72b2d50d6a5ac30ce5ec91e0016c140df866d438df` | runs on the cover CD's 1.04 |
| `cd-it` | Italian CD (*Ritorno dell'eternità*), volume `TECHNOMAGE`, 2001-05-02 | [`tecnomage`](https://archive.org/details/tecnomage) | `tecnomage.img` (CloneCD, raw MODE1/2352) | 834 701 280 | `0e7bf89c76917e16abeb42ba2802bf9b844266b746068b29df708019e8133e98` | runs on the cover CD's 1.04 |
| `cd-es` | Spanish CD (*El retorno de la Eternidad*), volume `TECHNOMAGE` (image rebuilt 2023-05-17) | [`Videogame_hgftssa`](https://archive.org/details/Videogame_hgftssa) | `TechnoMage_PC_CD_2000_ES.tar.001` + `.002`: a tar holding a 7z (LZMA2) holding the ISO | 367 001 600 + 286 791 680 | `a6d4f801d0203b6a93f5b4314da7f7b3caff3e4d1a5444443d31396dc6320da3`, `2a1816fc0ee8844cbf28815feb763bf2c3c7dffe6f862e9abf6d7a0cbe9d475d` | runs on the cover CD's 1.04 |

Download URLs (fixed; the page uses `/cors/` instead of `/download/`):

- `https://archive.org/download/SF2004-03/2004-03.BIN`
- `https://archive.org/download/TechnoMage_Return_of_Eternity_Europe/Disc.rar/TECHNOMAGE.mdf`
- `https://archive.org/download/technomage_pc-fr/TECHNOMAGE.ISO`
- `https://archive.org/download/tecnomage/tecnomage.img`
- `https://archive.org/download/Videogame_hgftssa/TechnoMage_PC_CD_2000_ES.tar.001` and `….tar.002`

The `.cue` (`2004-03.CUE`, `MODE2/2352`) and the `.mds` are not needed.

### Supplement of `cd-en`

The executable: the German cover CD's `TechnoMage.exe` (1.04, no copy protection), taken from `2004-03.BIN`
(the same file as `cd-de`'s medium). The English CD's own executable is the same game inside SecuROM.

### Supplement of `cd-fr`, `cd-it`, `cd-es`

As for `cd-en`: the cover CD's `TechnoMage.exe`. Each CD's own `Technomage.EXE` (885 379 bytes) is inside SecuROM
like the English one; each ships `techmage.cfg` with its language (`LANGUAGE = 2`, `3`, `4`).

The Spanish medium is the two parts of one tar (joined, 653 791 744 bytes) whose only file is
`TechnoMage_PC_CD_2000_ES.7z` (653 791 698 bytes; one LZMA2 folder, header LZMA-compressed). It holds the CD as
`TechnoMage_PC_CD_2000_ES/TechnoMage-El retorno de la Eternidad.iso` (729 899 008 bytes, cooked; SHA-256
`429440fb115ad157eb7669fd861bf53606bced35f19a730089bd0d0e55c853ad`), two pictures, `Nocd.7z` and
`FIX_W10/DDrawCompat-v0.3.2.zip`. The ISO was rebuilt by the uploader with a `NOCD/` folder (a cracked
executable); the game's files are the CD's (InstallShield's MD5s match). `rc-media`'s `packed` reader takes the ISO
out into memory; nothing else in the archive is read. The item's `TechnoMage_ES_Portable.7z` is an installed copy
with a cracked executable and is not used.

### Supplement of `cd-de`

The archived cover CD has 20 unreadable sound banks (RESEARCH). They are taken from the German retail CD 1.00,
whose ISO archive.org serves file by file; `recomp download` fetches both files beside the cover CD's image and
`recomp extract` takes the 20 files from them, each checked by SHA-256 (the manifest lists them).

| File (local name) | URL | Size | SHA-256 |
|-------------------|-----|------|---------|
| `TechnoMage-1.00-data1.hdr` | `https://archive.org/download/technomage-die-rueckkehr-der-ewigkeit-sunflowers-2000/TechnoMage.iso/data1.hdr` | 25 875 | `2fae071cf725561c667e5663976ffde96fef313e29846c3e92e71507cfb8899d` |
| `TechnoMage-1.00-data1.cab` | `https://archive.org/download/technomage-die-rueckkehr-der-ewigkeit-sunflowers-2000/TechnoMage.iso/data1.cab` | 167 969 179 | `a1c2ddb7cfbb92bfc1a17b71965a34031b4c5c3352066dfaf3e70cfac0d7cf1f` |

Supplied: `Sfx/CMine.SBK`, `Crypta`, `DForest`, `Dreamer`, `Dreamr_N`, `FBoss`, `FFCamp`, `FForest`, `FRace`, `FTown`,
`HBoss`, `MBoss`, `Mine`, `Mountain`, `OldOnes`, `RuinBoss`, `Ruins`, `Ruins_in`, `Shach`, `Steamer` (`.SBK`). 19 are
identical to version 1.04's; **`HBoss.SBK` is version 1.00's** (patch 1.04 changed it; decision log).

The English CD is archived as `Disc.rar` (794 064 172 bytes: `TECHNOMAGE.mdf`, `TECHNOMAGE.mds`, `CHECKSUM.md5`).
archive.org serves the members of an archive on their own (the item's "view contents" list; `/download/<item>/
Disc.rar/<member>`, also through `/cors/`, with CORS headers), so the image is downloaded directly and **no RAR
reader is needed**. The `.mdf` is one Mode 1 track of 357 432 raw sectors, read by `rc-media`'s raw CD reader.

## Patches

| Patch | Language | Date | archive.org | Size | SHA-256 | Needed |
|-------|----------|------|-------------|------|---------|--------|
| 1.02 | German | by 2001-01-28 | not archived | ≈2.5 MB | – | no (superseded) |
| 1.04 | German | 2001-03-23 | [`TechnoMageRoE/technomage_patch104_ger.exe`](https://archive.org/download/TechnoMageRoE/technomage_patch104_ger.exe) | 7 392 269 | `5ccb0d8e97378f3578d02fbf21c718bb6b5447493b9d230ed6e80830322d2b36` | **no**: `cd-de` is 1.04 already (served through `/cors/` with CORS headers; a RAR SFX) |

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
| `Technomage.EXE` | `cd-en` | 885 379 | `0673cd9f85e27eb31f1461a2e1906b7272fd4eb40d90d6414d04c5c76c7ffe98` | linked 2001-04-25; UPX inside SecuROM: **not used** |
| `TechnoMage.exe` | `cd-en` | 574 976 | `6f2521de…` | the cover CD's 1.04 executable, a supplement of the English variant (it runs the English data with `LANGUAGE = 1`) |
| `binkw32.dll`, `Mss32.dll` | `cd-en` | | as `cd-de` | the same files |
| `TM_Setup.exe` | `cd-en` | 246 784 | `ac104d700e33392150c2b2eb1101d0e620ab9328b27a9bf58adc4cb598b7eca9` | skipped |
| `TM_Diagnostic.exe` | `cd-en` | 85 504 | `1c09c0162ad39a4218496a57b2c08595cd0f2313b1ad6732a570912a9d3d3165` | skipped |
| `TechnoMage.exe`, `binkw32.dll`, `Mss32.dll` | `cd-fr`, `cd-it`, `cd-es` | | as `cd-en` | the cover CD's executable (supplement), the CDs' own DLLs |
| `TM_Setup.exe` | `cd-fr` | 246 272 | `1c3efcd3ebfa4219274452f3166d39b860dabc17efd6967aa5c1d5eff161c070` | skipped; tells the CD apart |
| `TM_Diagnostic.exe` | `cd-fr` | 86 016 | `58cc3aa262b2dee5b764c3105bb585fdf1f09299c7d7606021a42ae601bc6162` | skipped |
| `TM_Setup.exe` | `cd-it` | 244 224 | `058c1a2f37b025d5b7fa0b49b2e344ab1b33fab585e561e3cb3aa1f27d2c18d7` | skipped; tells the CD apart |
| `TM_Diagnostic.exe` | `cd-it` | 86 016 | `0dbb71be1a1e050a46c1f4d8ba3e64bcec8820bba640831164285296e68153d4` | skipped |
| `TM_Setup.exe` | `cd-es` | 245 760 | `57c30c83c2023f91a2dafb04f07f7ff5967799cb6b08869e48b144490cf71b9e` | skipped; tells the CD apart |
| `TM_Diagnostic.exe` | `cd-es` | 85 504 | `77c58c12473f6f87be9414fa668576c227427da58c6ceddcb06da97d40961a4b` | skipped |

## Not targeted

- German retail CD 1.00 (2000-11-07, SecuROM 4.10, redump 7367) and its patches: the cover CD is the same game at
  1.04 without SecuROM.
- The repack `TechnoMageRoE/Technomage_Setup.exe` and the Spanish `TechnoMage_ES_Portable.7z` (installed copies
  with changed executables). No Dutch release is archived on archive.org (searched 2026-11-27), although the
  executable knows Dutch (`NL`). The warez and magazine collections that hold the game, the PlayStation version.
