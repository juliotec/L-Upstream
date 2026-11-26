//! A commit's source: Forgejo's archive of that exact commit (`<repository's API>/archive/<40 hex>.tar.gz`: git archive's
//! tar, gzipped, one folder holding the tracked files, the commit in its pax global header's `comment`), downloaded by
//! its full hash with a size cap, or a file given, then checked and unpacked by the rules the Longshot Launcher read a
//! pull request's source by (LongshotLauncher's Core/PullRequestSource.cs, `Files`): the archive must name the commit
//! asked for; plain files and folders only (a link, a device, anything else refused); each path, its top folder taken
//! off, relative and inside the folder (no `..`, `.` or empty part, no `\`, no drive). On top of those: no path twice,
//! and no more than [`MAX_UNPACKED`] bytes unpacked.

use build_core::{Error, Result};
use build_driver::tools::archive::Reader;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

/// Longshot's repository on the forge, as its API names it: `--url`'s default (`<it>/archive/<commit>.tar.gz` is the
/// archive; the web page's own `/archive/` is not served for a commit).
pub const DEFAULT_URL: &str = "https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot";
/// The largest archive taken (the launcher's cap; Longshot's is about 20 MB).
pub const MAX_ARCHIVE: u64 = 128 << 20;
/// The most an archive may unpack to (a gzip bomb stops here; Longshot's tree is about 150 MB).
pub const MAX_UNPACKED: u64 = 2 << 30;

/// Whether `s` is a commit's full hash: 40 hex digits.
pub fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The archive's address for `commit` in the repository at `base` (https only).
pub fn archive_url(base: &str, commit: &str) -> std::result::Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    if !base.starts_with("https://") || base.contains(char::is_whitespace) {
        return Err(format!("--url {base}: an https:// address of the repository"));
    }
    Ok(format!("{base}/archive/{}.tar.gz", commit.to_ascii_lowercase()))
}

fn unreadable(text: impl Into<String>) -> Error {
    Error::player("source_unreadable", text)
}

/// The archive of `url` downloaded to `to` (through `to.part`), refused past [`MAX_ARCHIVE`]: its size. Kept: the
/// same commit is not downloaded twice.
pub fn download(url: &str, to: &Path) -> Result<u64> {
    if let Some(m) = std::fs::metadata(to).ok().filter(|m| m.is_file() && m.len() > 0) {
        return Ok(m.len());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let part = PathBuf::from(format!("{}.part", to.display()));
    let got = build_driver::tools::store::download(url, &part, MAX_ARCHIVE);
    if let Err(e) = got {
        let _ = std::fs::remove_file(&part);
        return Err(Error::player(
            "source_download_failed",
            format!("The commit's source could not be downloaded ({e}). Check the connection and try again."),
        ));
    }
    std::fs::rename(&part, to).map_err(|e| Error::io(to, e))?;
    Ok(std::fs::metadata(to).map_err(|e| Error::io(to, e))?.len())
}

/// A path of the archive, its top folder taken off, when it may be unpacked (the launcher's rules, and no drive).
pub fn inside(name: &str) -> Option<&str> {
    let (_, path) = name.split_once('/')?;
    build_core::fsx::relative_ok(path).then_some(path)
}

/// What [`unpack`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpacked {
    /// The commit the archive names (lower case).
    pub commit: String,
    /// Its files.
    pub files: u64,
    /// Their bytes.
    pub bytes: u64,
}

/// The archive at `archive` (a tar.gz) checked and its files unpacked into `into` (replaced whole: through
/// `into.part`, nothing of it left when it is refused): the commit it names, which must be `want` when given.
pub fn unpack(archive: &Path, want: Option<&str>, into: &Path) -> Result<Unpacked> {
    let size = std::fs::metadata(archive).map_err(|e| unreadable(format!("{}: {e}", archive.display())))?.len();
    if size > MAX_ARCHIVE {
        return Err(unreadable(format!("{} is larger than {} MB.", archive.display(), MAX_ARCHIVE >> 20)));
    }
    let part = PathBuf::from(format!("{}.part", into.display()));
    let _ = build_core::fsx::remove_tree(&part);
    std::fs::create_dir_all(&part).map_err(|e| Error::io(&part, e))?;
    let done = read_into(archive, want, &part);
    let u = match done {
        Ok(u) => u,
        Err(e) => {
            let _ = build_core::fsx::remove_tree(&part);
            return Err(e);
        }
    };
    if into.exists() {
        build_core::fsx::remove_tree(into)?;
    }
    std::fs::rename(&part, into).map_err(|e| Error::io(into, e))?;
    Ok(u)
}

fn read_into(archive: &Path, want: Option<&str>, dir: &Path) -> Result<Unpacked> {
    let file = std::fs::File::open(archive).map_err(|e| unreadable(format!("{}: {e}", archive.display())))?;
    let gz = flate2::read::GzDecoder::new(BufReader::with_capacity(1 << 20, file));
    let mut t = Reader::new(BufReader::with_capacity(1 << 20, gz));
    let bad = |e: String| unreadable(format!("The commit's source can't be read: {e}."));
    let (mut files, mut bytes) = (0u64, 0u64);
    while let Some(e) = t.next().map_err(bad)? {
        if e.kind == b'5' {
            t.skip(e.size).map_err(bad)?;
            continue;
        }
        if e.kind != b'0' {
            return Err(unreadable(format!("The commit's source holds what is not a file: {}.", e.name)));
        }
        let Some(rel) = inside(&e.name) else {
            return Err(unreadable(format!("The commit's source names a file outside its folder: {}.", e.name)));
        };
        bytes = bytes.saturating_add(e.size);
        if bytes > MAX_UNPACKED {
            return Err(unreadable(format!("The commit's source unpacks to more than {} GB.", MAX_UNPACKED >> 30)));
        }
        let out = dir.join(rel);
        if std::fs::symlink_metadata(&out).is_ok() {
            return Err(unreadable(format!("The commit's source holds {rel} twice.")));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let mut f = std::fs::File::create(&out).map_err(|err| Error::io(&out, err))?;
        t.copy_to(e.size, &mut f).map_err(bad)?;
        f.flush().map_err(|err| Error::io(&out, err))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if e.mode & 0o100 != 0 { 0o755 } else { 0o644 };
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))
                .map_err(|err| Error::io(&out, err))?;
        }
        files += 1;
    }
    let named = t.globals.iter().rev().find(|(k, _)| k == "comment").map(|(_, v)| v.trim().to_ascii_lowercase());
    let Some(commit) = named.filter(|c| is_commit(c)) else {
        return Err(unreadable("The commit's source names no commit (it is not a git archive of one)."));
    };
    if let Some(want) = want.filter(|w| !w.eq_ignore_ascii_case(&commit)) {
        return Err(unreadable(format!(
            "The commit's source is not of the commit it was asked for ({}): it is {}.",
            &want[..want.len().min(8)],
            &commit[..8]
        )));
    }
    if files == 0 {
        return Err(unreadable("The commit's source is empty."));
    }
    Ok(Unpacked { commit, files, bytes })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const BLOCK: usize = 512;

    fn header(name: &str, kind: u8, size: usize, mode: u32, link: &str) -> Vec<u8> {
        let mut h = vec![0u8; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[156] = kind;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        h
    }

    fn pad(v: &mut Vec<u8>) {
        v.resize(v.len().div_ceil(BLOCK) * BLOCK, 0);
    }

    /// One tar entry: `(name, kind, data, mode)`; kind `g` a pax global header of `data`'s records.
    pub(crate) type Item<'a> = (&'a str, u8, &'a [u8], u32);

    /// A tar.gz of `items`, as git archive lays one out.
    pub(crate) fn tar_gz(items: &[Item<'_>]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, kind, data, mode) in items {
            let (data, link) = if *kind == b'2' { (&b""[..], std::str::from_utf8(data).unwrap()) } else { (*data, "") };
            tar.extend(header(name, *kind, data.len(), *mode, link));
            tar.extend_from_slice(data);
            pad(&mut tar);
        }
        tar.extend(vec![0u8; 2 * BLOCK]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    pub(crate) fn pax_comment(commit: &str) -> Vec<u8> {
        let rec = format!(" comment={commit}\n");
        let n = rec.len() + 2;
        format!("{n}{rec}").into_bytes()
    }

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("from-source-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const C: &str = "72beb786419f0123456789abcdef0123456789ab";

    fn try_unpack(d: &Path, items: &[Item<'_>], want: Option<&str>) -> Result<Unpacked> {
        let a = d.join("a.tar.gz");
        std::fs::write(&a, tar_gz(items)).unwrap();
        unpack(&a, want, &d.join("src"))
    }

    #[test]
    fn a_commit_s_archive_is_unpacked_without_its_top_folder() {
        let d = scratch("good");
        let c = pax_comment(C);
        let items: &[Item<'_>] = &[
            ("pax_global_header", b'g', &c, 0o666),
            ("longshot/", b'5', b"", 0o775),
            ("longshot/Cargo.toml", b'0', b"[workspace]\n", 0o664),
            ("longshot/scripts/", b'5', b"", 0o775),
            ("longshot/scripts/x.sh", b'0', b"#!/bin/sh\n", 0o775),
        ];
        let u = try_unpack(&d, items, Some(&C.to_uppercase())).unwrap();
        assert_eq!(u, Unpacked { commit: C.into(), files: 2, bytes: 22 });
        assert_eq!(std::fs::read(d.join("src/Cargo.toml")).unwrap(), b"[workspace]\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &str| std::fs::metadata(d.join(p)).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode("src/scripts/x.sh"), mode("src/Cargo.toml")), (0o755, 0o644));
        }
        // (again, into the same folder: replaced whole)
        std::fs::write(d.join("src/stale"), b"x").unwrap();
        try_unpack(&d, items, None).unwrap();
        assert!(!d.join("src/stale").exists() && !d.join("src.part").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_launcher_s_rules_refuse_what_they_refused() {
        let d = scratch("bad");
        let c = pax_comment(C);
        let g: Item<'_> = ("pax_global_header", b'g', &c, 0o666);
        let file: Item<'_> = ("l/a", b'0', b"a", 0o644);
        let cases: Vec<(Vec<Item<'_>>, &str)> = vec![
            (vec![file], "names no commit"),
            (vec![("pax_global_header", b'g', b"18 comment=abcdef\n", 0o666), file], "names no commit"),
            (vec![g, ("l/link", b'2', b"a", 0o777)], "not a file"),
            (vec![g, ("l/hard", b'1', b"a", 0o644)], "not a file"),
            (vec![g, ("l/dev", b'3', b"", 0o644)], "not a file"),
            (vec![g, ("l/../x", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, ("l//x", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, ("l/./x", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, ("l/a\\..\\x", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, ("l/C:x", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, ("top", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, ("l/x/", b'0', b"x", 0o644)], "outside its folder"),
            (vec![g, file, file], "twice"),
            (vec![g, ("l/", b'5', b"", 0o755)], "empty"),
        ];
        for (items, why) in cases {
            let e = try_unpack(&d, &items, None).unwrap_err();
            assert_eq!(e.code(), "source_unreadable", "{why}");
            assert!(e.to_string().contains(why), "{why}: {e}");
            assert!(!d.join("src").exists() && !d.join("src.part").exists(), "{why}: nothing left");
        }
        let e = try_unpack(&d, &[g, file], Some("0123456789012345678901234567890123456789")).unwrap_err();
        assert!(e.to_string().contains("not of the commit it was asked for (01234567)"), "{e}");
        std::fs::write(d.join("junk.tar.gz"), b"not gzip").unwrap();
        assert_eq!(unpack(&d.join("junk.tar.gz"), None, &d.join("src")).unwrap_err().code(), "source_unreadable");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn commits_and_addresses() {
        assert!(is_commit(C) && is_commit(&C.to_uppercase()));
        assert!(!is_commit(&C[..39]) && !is_commit(&format!("{}g", &C[..39])));
        assert_eq!(archive_url(&format!("{DEFAULT_URL}/"), C).unwrap(), format!("{DEFAULT_URL}/archive/{C}.tar.gz"));
        assert!(archive_url("http://git.magicbean.dev/x", C).is_err(), "https only");
        assert!(archive_url("https://x y", C).is_err());
        assert_eq!(inside("longshot/a/b"), Some("a/b"));
        assert_eq!(inside("longshot"), None);
    }
}
