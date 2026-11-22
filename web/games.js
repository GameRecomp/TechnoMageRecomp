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
        sha256: 'c9db7f91ae3d7755b88f9921c0904ee9f335463c4dde700c779454bd918e8ed7' },
      { code: 'en', flag: 'gb', label: 'English', title: 'TechnoMage: Return of Eternity', file: 'TECHNOMAGE.mdf',
        url: `${IA}TechnoMage_Return_of_Eternity_Europe/Disc.rar/TECHNOMAGE.mdf`, size: 840680064,
        sha256: '' /* M0: measure */ },
    ],
  },
};
