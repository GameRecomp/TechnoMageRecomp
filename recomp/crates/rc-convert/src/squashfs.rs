// SPDX-License-Identifier: MIT
//! A SquashFS 4.0 image written as a stream, for AppImages: uncompressed data, inodes and
//! directories (the games' files are mostly compressed already, and an uncompressed image needs
//! no compressor in a browser), no fragments, no extended attributes.
//!
//! An image starts with its superblock, which points to the tables at its end; the tables are
//! known only when every file's data is placed. So the writer takes the whole tree first (every
//! directory, and every file with its size): its tables' sizes follow from it, whatever order
//! the files' data then comes in, and the superblock is written before the data. At the end the
//! tables are written with the files' real places, and their sizes are checked against the
//! plan.
//!
//! Layout: superblock (96 bytes), the files' data (each file's blocks back to back, the last one
//! short), the inode table, the directory table, the id table (one metadata block holding uid
//! 0, then its pointer), padded to 4 KiB.

use std::collections::BTreeMap;
use std::io::{self, Write};

const BLOCK: u64 = 1 << 17;
const BLOCK_LOG: u16 = 17;
const META: usize = 8192;
/// Inodes, data, fragments, xattrs and ids uncompressed; no fragments, no xattrs.
const FLAGS: u16 = 0x0001 | 0x0002 | 0x0008 | 0x0010 | 0x0100 | 0x0200 | 0x0800;
const NONE: u64 = u64::MAX;
/// A data block's size word: stored uncompressed.
const BLOCK_UNCOMPRESSED: u32 = 1 << 24;
const TYPE_DIR: u16 = 1;
const TYPE_FILE: u16 = 2;
const TYPE_LDIR: u16 = 8;

/// An entry of the tree: a directory, or a file of a size with its mode.
#[derive(Debug, Clone)]
pub enum Node {
    Dir,
    File { size: u64, mode: u16 },
}

/// Metadata: 8 KiB blocks, each after a two-byte header (its length, bit 15: uncompressed).
#[derive(Default)]
struct Meta {
    out: Vec<u8>,
    cur: Vec<u8>,
}

impl Meta {
    /// Where the next byte goes: (its block's offset in the table, the offset in the block).
    fn pos(&self) -> (u64, u16) {
        (self.out.len() as u64, self.cur.len() as u16)
    }

    fn push(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let n = (META - self.cur.len()).min(data.len());
            self.cur.extend_from_slice(&data[..n]);
            data = &data[n..];
            if self.cur.len() == META {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        self.out
            .extend_from_slice(&(self.cur.len() as u16 | 0x8000).to_le_bytes());
        self.out.append(&mut self.cur);
    }

    fn finish(mut self) -> Vec<u8> {
        self.flush();
        self.out
    }
}

/// The tree's inodes and directories, given where each file's data starts.
struct Tables {
    inodes: Vec<u8>,
    dirs: Vec<u8>,
    root: u64,
    count: u32,
}

fn tables(tree: &BTreeMap<String, Node>, starts: &BTreeMap<String, u64>) -> Tables {
    // Inode numbers: the entries in path order, the root last (as mksquashfs gives it).
    let paths: Vec<&String> = tree.keys().collect();
    let index: BTreeMap<&str, u32> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i as u32 + 1))
        .collect();
    let root_number = paths.len() as u32 + 1;
    let number = |p: &str| -> u32 { if p.is_empty() { root_number } else { index[p] } };
    let parent = |p: &str| -> String {
        p.rsplit_once('/')
            .map_or_else(String::new, |(a, _)| a.to_string())
    };
    let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let is_dir = |p: &str| p.is_empty() || matches!(tree[p], Node::Dir);
    let mut children: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    children.insert(String::new(), Vec::new());
    for p in &paths {
        children.entry(parent(p)).or_default().push(p);
        if is_dir(p) {
            children.entry((*p).clone()).or_default();
        }
    }
    // Directory entries sorted by name, as readers search them.
    for list in children.values_mut() {
        list.sort_by_key(|p| name(p));
    }
    let all: Vec<String> = paths
        .iter()
        .map(|p| (*p).clone())
        .chain(std::iter::once(String::new()))
        .collect();

    // The inodes, in number order. A directory's inode has the same size whatever its listing's
    // place, so the first pass gives every inode's place and the second the listings'.
    let inode_table = |listing: &BTreeMap<String, (u64, u16, u32)>| {
        let mut m = Meta::default();
        let mut refs = BTreeMap::new();
        for p in &all {
            refs.insert(p.clone(), m.pos());
            let mut b = Vec::with_capacity(64);
            let (kind, mode) = match if p.is_empty() { &Node::Dir } else { &tree[p] } {
                Node::Dir => (TYPE_LDIR, 0o755),
                Node::File { mode, .. } => (TYPE_FILE, *mode),
            };
            b.extend_from_slice(&kind.to_le_bytes());
            b.extend_from_slice(&mode.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes()); // uid index
            b.extend_from_slice(&0u16.to_le_bytes()); // gid index
            b.extend_from_slice(&0u32.to_le_bytes()); // mtime
            b.extend_from_slice(&number(p).to_le_bytes());
            if let Some(Node::File { size, .. }) = tree.get(p) {
                let start = starts.get(p).copied().unwrap_or(0);
                b.extend_from_slice(&(start as u32).to_le_bytes());
                b.extend_from_slice(&u32::MAX.to_le_bytes()); // no fragment
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&(*size as u32).to_le_bytes());
                let mut left = *size;
                while left > 0 {
                    let n = left.min(BLOCK);
                    b.extend_from_slice(&(n as u32 | BLOCK_UNCOMPRESSED).to_le_bytes());
                    left -= n;
                }
            } else {
                let subdirs = children[p].iter().filter(|c| is_dir(c)).count() as u32;
                let (block, offset, size) = listing.get(p).copied().unwrap_or((0, 0, 3));
                let up = if p.is_empty() {
                    root_number + 1
                } else {
                    number(&parent(p))
                };
                b.extend_from_slice(&(2 + subdirs).to_le_bytes()); // links
                b.extend_from_slice(&size.to_le_bytes());
                b.extend_from_slice(&(block as u32).to_le_bytes());
                b.extend_from_slice(&up.to_le_bytes());
                b.extend_from_slice(&0u16.to_le_bytes()); // index count
                b.extend_from_slice(&offset.to_le_bytes());
                b.extend_from_slice(&u32::MAX.to_le_bytes()); // xattr index
            }
            m.push(&b);
        }
        (m, refs)
    };
    let (_, refs) = inode_table(&BTreeMap::new());

    // The listings: runs of up to 256 entries whose inodes share a metadata block, each after a
    // header (its count - 1, that block, the first inode number), the entries by name.
    let mut dirs = Meta::default();
    let mut listing = BTreeMap::new();
    for p in all.iter().filter(|p| is_dir(p)) {
        let (block, offset) = dirs.pos();
        let kids = &children[p];
        let mut bytes = Vec::new();
        let mut i = 0;
        while i < kids.len() {
            let (start, _) = refs[kids[i].as_str()];
            let base = number(kids[i]);
            let mut j = i;
            while j < kids.len()
                && j - i < 256
                && refs[kids[j].as_str()].0 == start
                && (i64::from(number(kids[j])) - i64::from(base)).abs() < 32767
            {
                j += 1;
            }
            bytes.extend_from_slice(&((j - i - 1) as u32).to_le_bytes());
            bytes.extend_from_slice(&(start as u32).to_le_bytes());
            bytes.extend_from_slice(&base.to_le_bytes());
            for k in &kids[i..j] {
                let n = name(k);
                bytes.extend_from_slice(&refs[k.as_str()].1.to_le_bytes());
                bytes.extend_from_slice(&((number(k) as i32 - base as i32) as i16).to_le_bytes());
                let t = if is_dir(k) { TYPE_DIR } else { TYPE_FILE };
                bytes.extend_from_slice(&t.to_le_bytes());
                bytes.extend_from_slice(&(n.len() as u16 - 1).to_le_bytes());
                bytes.extend_from_slice(n.as_bytes());
            }
            i = j;
        }
        listing.insert(p.clone(), (block, offset, bytes.len() as u32 + 3));
        dirs.push(&bytes);
    }
    let (inodes, refs) = inode_table(&listing);
    let (rb, ro) = refs[""];
    Tables {
        inodes: inodes.finish(),
        dirs: dirs.finish(),
        root: rb << 16 | u64::from(ro),
        count: all.len() as u32,
    }
}

/// The id table: one metadata block holding uid 0, then the pointer to it.
fn id_table(at: u64) -> (Vec<u8>, u64) {
    let mut m = Meta::default();
    m.push(&0u32.to_le_bytes());
    let mut b = m.finish();
    let start = at + b.len() as u64;
    b.extend_from_slice(&at.to_le_bytes());
    (b, start)
}

/// Writes an image of a planned tree into `out` (after whatever `out` holds already: offsets
/// are the image's own).
pub struct SquashWriter<W: Write> {
    out: W,
    tree: BTreeMap<String, Node>,
    starts: BTreeMap<String, u64>,
    /// Where the next data goes, from the image's start.
    pos: u64,
    data_end: u64,
    /// The file being written: its path and the bytes still expected.
    open: Option<(String, u64)>,
    tables_len: (usize, usize),
}

impl<W: Write> SquashWriter<W> {
    /// Plan `tree` (paths without a leading `/`, `a/b`; every directory listed) and write the
    /// superblock.
    pub fn new(mut out: W, tree: BTreeMap<String, Node>) -> io::Result<Self> {
        for p in tree.keys() {
            if let Some((parent, _)) = p.rsplit_once('/')
                && !matches!(tree.get(parent), Some(Node::Dir))
            {
                return Err(io::Error::other(format!(
                    "squashfs: {p} has no directory {parent}"
                )));
            }
        }
        let data: u64 = tree
            .values()
            .map(|n| match n {
                Node::File { size, .. } => *size,
                Node::Dir => 0,
            })
            .sum();
        let data_end = 96 + data;
        if data_end > u64::from(u32::MAX) {
            return Err(io::Error::other(
                "squashfs: the files are larger than 4 GiB",
            ));
        }
        let t = tables(&tree, &BTreeMap::new());
        let inode_start = data_end;
        let dir_start = inode_start + t.inodes.len() as u64;
        let (ids, id_start) = id_table(dir_start + t.dirs.len() as u64);
        let end = dir_start + t.dirs.len() as u64 + ids.len() as u64;
        let mut sb = Vec::with_capacity(96);
        sb.extend_from_slice(&0x7371_7368u32.to_le_bytes()); // "hsqs"
        sb.extend_from_slice(&t.count.to_le_bytes());
        sb.extend_from_slice(&0u32.to_le_bytes()); // mtime
        sb.extend_from_slice(&(BLOCK as u32).to_le_bytes());
        sb.extend_from_slice(&0u32.to_le_bytes()); // fragments
        sb.extend_from_slice(&1u16.to_le_bytes()); // gzip (nothing is compressed)
        sb.extend_from_slice(&BLOCK_LOG.to_le_bytes());
        sb.extend_from_slice(&FLAGS.to_le_bytes());
        sb.extend_from_slice(&1u16.to_le_bytes()); // ids
        sb.extend_from_slice(&4u16.to_le_bytes());
        sb.extend_from_slice(&0u16.to_le_bytes());
        sb.extend_from_slice(&t.root.to_le_bytes());
        sb.extend_from_slice(&end.to_le_bytes()); // bytes used
        sb.extend_from_slice(&id_start.to_le_bytes());
        sb.extend_from_slice(&NONE.to_le_bytes()); // xattrs
        sb.extend_from_slice(&inode_start.to_le_bytes());
        sb.extend_from_slice(&dir_start.to_le_bytes());
        sb.extend_from_slice(&NONE.to_le_bytes()); // fragments
        sb.extend_from_slice(&NONE.to_le_bytes()); // export
        out.write_all(&sb)?;
        Ok(SquashWriter {
            out,
            tree,
            starts: BTreeMap::new(),
            pos: 96,
            data_end,
            open: None,
            tables_len: (t.inodes.len(), t.dirs.len()),
        })
    }

    /// Start the file `path` (planned, not yet written).
    pub fn begin(&mut self, path: &str) -> io::Result<()> {
        if let Some((p, _)) = &self.open {
            return Err(io::Error::other(format!("squashfs: {p} is not finished")));
        }
        let size = match self.tree.get(path) {
            Some(Node::File { size, .. }) => *size,
            _ => {
                return Err(io::Error::other(format!(
                    "squashfs: {path} was not planned as a file"
                )));
            }
        };
        if self.starts.contains_key(path) {
            return Err(io::Error::other(format!("squashfs: {path} written twice")));
        }
        self.starts.insert(path.to_string(), self.pos);
        self.open = Some((path.to_string(), size));
        Ok(())
    }

    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let Some((p, left)) = &mut self.open else {
            return Err(io::Error::other("squashfs: data outside a file"));
        };
        if data.len() as u64 > *left {
            return Err(io::Error::other(format!(
                "squashfs: {p} is longer than planned"
            )));
        }
        *left -= data.len() as u64;
        self.pos += data.len() as u64;
        self.out.write_all(data)
    }

    pub fn end(&mut self) -> io::Result<()> {
        match self.open.take() {
            Some((_, 0)) => Ok(()),
            Some((p, left)) => Err(io::Error::other(format!(
                "squashfs: {p} is {left} bytes short"
            ))),
            None => Err(io::Error::other("squashfs: no file to end")),
        }
    }

    /// A whole file.
    pub fn file(&mut self, path: &str, data: &[u8]) -> io::Result<()> {
        self.begin(path)?;
        self.write(data)?;
        self.end()
    }

    /// Write the tables (every planned file must have been written) and pad to 4 KiB; returns
    /// the output and the image's length.
    pub fn finish(mut self) -> io::Result<(W, u64)> {
        if self.open.is_some() {
            return Err(io::Error::other("squashfs: a file is not finished"));
        }
        if let Some(p) = self
            .tree
            .iter()
            .find(|(p, n)| matches!(n, Node::File { .. }) && !self.starts.contains_key(*p))
            .map(|(p, _)| p.clone())
        {
            return Err(io::Error::other(format!("squashfs: {p} was never written")));
        }
        debug_assert_eq!(self.pos, self.data_end);
        let t = tables(&self.tree, &self.starts);
        if (t.inodes.len(), t.dirs.len()) != self.tables_len {
            return Err(io::Error::other(
                "squashfs: the tables differ from the plan",
            ));
        }
        let dir_start = self.data_end + t.inodes.len() as u64;
        let (ids, _) = id_table(dir_start + t.dirs.len() as u64);
        self.out.write_all(&t.inodes)?;
        self.out.write_all(&t.dirs)?;
        self.out.write_all(&ids)?;
        let end = dir_start + t.dirs.len() as u64 + ids.len() as u64;
        let padded = end.div_ceil(4096) * 4096;
        self.out.write_all(&vec![0; (padded - end) as usize])?;
        Ok((self.out, padded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image of a small tree, read back with unsquashfs where it is installed.
    #[test]
    fn unsquashfs_reads_it() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "AppRun".into(),
            Node::File {
                size: 5,
                mode: 0o755,
            },
        );
        tree.insert("data".into(), Node::Dir);
        tree.insert(
            "data/empty".into(),
            Node::File {
                size: 0,
                mode: 0o644,
            },
        );
        tree.insert("data/game".into(), Node::Dir);
        let big = 300_000u64;
        tree.insert(
            "data/game/big.bin".into(),
            Node::File {
                size: big,
                mode: 0o644,
            },
        );
        for i in 0..600 {
            tree.insert(
                format!("data/game/f{i:03}"),
                Node::File {
                    size: 3,
                    mode: 0o644,
                },
            );
        }
        let mut w = SquashWriter::new(Vec::new(), tree).unwrap();
        // Out of path order on purpose.
        let bigdata: Vec<u8> = (0..big).map(|i| (i * 7 % 251) as u8).collect();
        w.file("data/game/big.bin", &bigdata).unwrap();
        for i in (0..600).rev() {
            w.file(&format!("data/game/f{i:03}"), format!("{i:03}").as_bytes())
                .unwrap();
        }
        w.file("AppRun", b"#!sh\n").unwrap();
        w.file("data/empty", b"").unwrap();
        let (img, len) = w.finish().unwrap();
        assert_eq!(img.len() as u64, len);
        assert_eq!(len % 4096, 0);
        let dir = std::env::temp_dir().join(format!("rc-squashfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("img.sqfs");
        std::fs::write(&file, &img).unwrap();
        let out = dir.join("x");
        let run = std::process::Command::new("unsquashfs")
            .arg("-no-xattrs")
            .arg("-d")
            .arg(&out)
            .arg(&file)
            .output();
        let Ok(run) = run else {
            eprintln!("unsquashfs not installed: the image is not checked");
            return;
        };
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(std::fs::read(out.join("AppRun")).unwrap(), b"#!sh\n");
        assert_eq!(
            std::fs::read(out.join("data/game/big.bin")).unwrap(),
            bigdata
        );
        assert_eq!(std::fs::read(out.join("data/game/f123")).unwrap(), b"123");
        assert_eq!(std::fs::read(out.join("data/empty")).unwrap(), b"");
        assert_eq!(
            std::fs::read_dir(out.join("data/game")).unwrap().count(),
            601
        );
        // The modes as unpacked (only Unix keeps them).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(out.join("AppRun"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
