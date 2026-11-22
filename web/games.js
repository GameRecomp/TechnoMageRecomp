// SPDX-License-Identifier: MIT
// The game the converter makes builds of, as the page, the worker and the player name it. The id is
// the converter's (games/technomage/game.toml) and the directory the browser stores the game in.
// `languages` are the releases archived on archive.org (abandonware), each at a fixed URL: the page
// downloads the medium and checks it before converting it. They must match the manifest (a test in
// rc-manifest checks every medium's size and SHA-256). The page fetches them through archive.org's
// /cors/ path, which sends CORS headers (/download/ does not) but ignores Range: files come whole,
// so they are streamed, never held in memory. A file inside an archive on archive.org is served on
// its own (`<item>/<archive>/<member>`, the "view contents" link), so nothing is unpacked here.
const IA = 'https://archive.org/cors/';

export const GAMES = {
  technomage: {
    id: 'technomage',
    name: 'TechnoMage',
    full: 'TechnoMage: Return of Eternity (TechnoMage – Die Rückkehr der Ewigkeit)',
    stem: 'TechnoMage',
    web: 'technomage',
    icon: 'technomage.svg',
    languages: [
      { code: 'de', label: 'Deutsch', title: 'TechnoMage – Die Rückkehr der Ewigkeit', file: '2004-03.BIN',
        url: `${IA}SF2004-03/2004-03.BIN`, size: 845217072,
        sha256: 'c9db7f91ae3d7755b88f9921c0904ee9f335463c4dde700c779454bd918e8ed7',
        // The cover CD's image has 20 unreadable sound banks; they come from the retail CD
        // 1.00's cabinet, which archive.org serves out of its ISO (games/technomage/game.toml).
        supplements: [
          { file: 'TechnoMage-1.00-data1.hdr', size: 25875,
            url: `${IA}technomage-die-rueckkehr-der-ewigkeit-sunflowers-2000/TechnoMage.iso/data1.hdr`,
            sha256: '2fae071cf725561c667e5663976ffde96fef313e29846c3e92e71507cfb8899d' },
          { file: 'TechnoMage-1.00-data1.cab', size: 167969179,
            url: `${IA}technomage-die-rueckkehr-der-ewigkeit-sunflowers-2000/TechnoMage.iso/data1.cab`,
            sha256: 'a1c2ddb7cfbb92bfc1a17b71965a34031b4c5c3352066dfaf3e70cfac0d7cf1f' },
        ] },
      { code: 'en', flag: 'gb', label: 'English', title: 'TechnoMage: Return of Eternity', file: 'TECHNOMAGE.mdf',
        url: `${IA}TechnoMage_Return_of_Eternity_Europe/Disc.rar/TECHNOMAGE.mdf`, size: 840680064,
        sha256: '178a16bf56c4a2b65298be6202c25d8e0bf355b76018a7ab0d45da75e5786450' },
    ],
  },
};
