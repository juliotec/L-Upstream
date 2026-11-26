//! Synthetic game files for tests: a .nup package built in memory, of titles whose exefs and romfs files a test
//! chooses (none of them the game's). The read stage's own tests use it, and with the `testkit` feature another crate's
//! tests do too (builder/crates/synthetic-game: the builder end to end on a synthetic game).

use sha2::{Digest, Sha256};

/// A file of a synthetic package: its path, its bytes and its codec (0 stored as it is, 1 one zstd frame; 2 codec 1
/// too, its frame [`wide_window_frame`]'s).
pub type NupFile = (String, Vec<u8>, u8);

/// A package built in memory: titles of (id, version, exefs files, romfs files).
pub struct NupBuilder {
    /// The titles: (title id, version, exefs files, romfs files).
    pub titles: Vec<(u64, u32, Vec<NupFile>, Vec<NupFile>)>,
}

/// `data` as one zstd frame (the way a .nup stores a file of codec 1, and the way the game's `.zs` files are).
pub fn zstd_frame(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    ruzstd::encoding::compress(data, &mut out, ruzstd::encoding::CompressionLevel::Fastest);
    out
}

/// `data` as one zstd frame of raw blocks whose header asks for a window of 256 MB: what zstd's streaming decoder
/// refuses by default (over 128 MB), though the content is small.
pub fn wide_window_frame(data: &[u8]) -> Vec<u8> {
    raw_frame(data, 28)
}

/// `data` as one zstd frame of raw blocks (128 KB each) whose header asks for a window of 2^`window_log` bytes.
pub fn raw_frame(data: &[u8], window_log: u8) -> Vec<u8> {
    let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0x80, (window_log - 10) << 3]; // (4-byte content size, the window)
    out.extend((data.len() as u32).to_le_bytes());
    let mut at = 0;
    loop {
        let k = (data.len() - at).min(1 << 17);
        let last = at + k == data.len();
        let bh = ((k as u32) << 3) | u32::from(last);
        out.extend(&bh.to_le_bytes()[..3]);
        out.extend(&data[at..at + k]);
        at += k;
        if last {
            return out;
        }
    }
}

impl NupBuilder {
    /// The package's bytes.
    pub fn build(&self) -> Vec<u8> {
        // nodes: per title a root dir, an exefs dir, a romfs dir and its sub dirs; files
        let mut dirs: Vec<(String, Vec<u32>)> = Vec::new(); // (name, children as node ids to fix up)
        let mut files: Vec<(String, Vec<u8>, u8)> = Vec::new();
        let mut title_dirs = Vec::new();
        // node ids: dirs first, files after: children encoded as tagged (dir i) or (file i | 1<<31) and fixed later
        fn add_tree(
            dirs: &mut Vec<(String, Vec<u32>)>,
            files: &mut Vec<(String, Vec<u8>, u8)>,
            name: &str,
            list: &[(String, Vec<u8>, u8)],
        ) -> u32 {
            let me = dirs.len() as u32;
            dirs.push((name.to_string(), Vec::new()));
            let mut subdirs: Vec<(String, Vec<NupFile>)> = Vec::new();
            for (path, data, codec) in list {
                match path.split_once('/') {
                    Some((d, rest)) => match subdirs.iter_mut().find(|s| s.0 == d) {
                        Some(s) => s.1.push((rest.to_string(), data.clone(), *codec)),
                        None => subdirs.push((d.to_string(), vec![(rest.to_string(), data.clone(), *codec)])),
                    },
                    None => {
                        files.push((path.clone(), data.clone(), *codec));
                        let f = files.len() as u32 - 1;
                        dirs[me as usize].1.push(f | 1 << 31);
                    }
                }
            }
            for (d, l) in subdirs {
                let child = add_tree(dirs, files, &d, &l);
                dirs[me as usize].1.push(child);
            }
            me
        }
        for (_, _, exefs, romfs) in &self.titles {
            let root = dirs.len() as u32;
            dirs.push((String::new(), Vec::new()));
            let e = add_tree(&mut dirs, &mut files, "exefs", exefs);
            let r = add_tree(&mut dirs, &mut files, "romfs", romfs);
            dirs[root as usize].1 = vec![e, r];
            title_dirs.push((root, e, r));
        }
        let nd = dirs.len() as u32;
        let mut strings = Vec::new();
        let name = |s: &str, strings: &mut Vec<u8>| {
            let off = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            (off, s.len() as u32)
        };
        let mut kids = Vec::new();
        let mut dir_recs = Vec::new();
        for (n, children) in &dirs {
            let (no, nl) = name(n, &mut strings);
            let first = kids.len() as u32;
            for &c in children {
                kids.push(if c & 1 << 31 != 0 { nd + (c & !(1 << 31)) } else { c });
            }
            dir_recs.push((no, nl, first, children.len() as u32));
        }
        let nt = self.titles.len();
        let tables = 0x30 + 64 * nt + 24 * dirs.len() + 80 * files.len() + 4 * kids.len();
        let file_names: Vec<(u32, u32)> = files.iter().map(|f| name(&f.0, &mut strings)).collect();
        let soff = tables;
        let toc_size = soff + strings.len();
        let data_at = (0x80 + toc_size).div_ceil(16) * 16;
        let mut data = Vec::new();
        let mut recs = Vec::new();
        for (_, bytes, codec) in &files {
            let stored = match codec {
                1 => zstd_frame(bytes),
                2 => wide_window_frame(bytes),
                _ => bytes.clone(),
            };
            let off = if bytes.is_empty() { 0 } else { (data_at + data.len()) as u64 };
            if !bytes.is_empty() {
                data.extend_from_slice(&stored);
                data.resize(data.len().div_ceil(16) * 16, 0);
            }
            let hash: [u8; 32] = Sha256::digest(bytes).into();
            recs.push((
                bytes.len() as u64,
                off,
                if bytes.is_empty() { 0 } else { stored.len() as u64 },
                hash,
                if bytes.is_empty() { 0 } else { (*codec).min(1) },
            ));
        }
        let mut toc = vec![0u8; 0x30];
        toc[..4].copy_from_slice(b"TOC\x01");
        toc[4..8].copy_from_slice(&(nt as u32).to_le_bytes());
        toc[8..16].copy_from_slice(&u64::from(nd).to_le_bytes());
        toc[16..24].copy_from_slice(&(files.len() as u64).to_le_bytes());
        toc[24..32].copy_from_slice(&(soff as u64).to_le_bytes());
        toc[32..40].copy_from_slice(&(strings.len() as u64).to_le_bytes());
        for (i, (id, version, _, _)) in self.titles.iter().enumerate() {
            let mut t = vec![0u8; 64];
            let (root, e, r) = title_dirs[i];
            t[..8].copy_from_slice(&id.to_le_bytes());
            for (k, v) in [root, e, r, *version].into_iter().enumerate() {
                t[8 + 4 * k..12 + 4 * k].copy_from_slice(&v.to_le_bytes());
            }
            toc.extend_from_slice(&t);
        }
        for (no, nl, first, count) in dir_recs {
            for v in [no, nl, first, count] {
                toc.extend_from_slice(&v.to_le_bytes());
            }
            toc.extend_from_slice(&[0; 8]);
        }
        for (k, (size, off, stored, hash, codec)) in recs.iter().enumerate() {
            toc.extend_from_slice(&file_names[k].0.to_le_bytes());
            toc.extend_from_slice(&file_names[k].1.to_le_bytes());
            for v in [*size, *off, *stored] {
                toc.extend_from_slice(&v.to_le_bytes());
            }
            toc.extend_from_slice(hash);
            toc.push(*codec);
            toc.extend_from_slice(&[0; 15]);
        }
        for k in &kids {
            toc.extend_from_slice(&k.to_le_bytes());
        }
        toc.extend_from_slice(&strings);
        let total = data_at + data.len();
        let mut out = vec![0u8; 0x80];
        out[..4].copy_from_slice(b"NUP\x01");
        out[8..16].copy_from_slice(&0x80u64.to_le_bytes());
        out[16..24].copy_from_slice(&(toc.len() as u64).to_le_bytes());
        out[24..56].copy_from_slice(&Sha256::digest(&toc));
        out[0x38..0x40].copy_from_slice(&(data_at as u64).to_le_bytes());
        out[0x40..0x48].copy_from_slice(&(total as u64).to_le_bytes());
        out.extend_from_slice(&toc);
        out.resize(data_at, 0);
        out.extend_from_slice(&data);
        out
    }
}
