//! `tools.lock`: the build tools longshot-builder carries, a line each (docs/dev/reference/releases.md, "tools.lock"),
//! read and checked here, the one reader of the file.
//!
//! ```text
//! tool <name> <version> <system> <sha256> <size> <dest> <prefix> <url> [<url> ...]
//! ```
//!
//! Every tool comes from Longshot's store first: its first two URLs are the store's copy at the address its name,
//! version and SHA-256 give (the edge's, then the store's own); further URLs are mirrors. A line whose first URLs lead
//! anywhere else is refused: what CI made and published is what a release must carry.

use std::path::Path;

/// The systems a tool is for.
pub const SYSTEMS: [&str; 2] = ["windows-x64", "linux-x64"];
/// The tools each system's builder needs: a lock without one of them is refused by `check` (the release's first step).
/// The table [`super::TOOLS`] names the variable each is given to the compile by.
pub const REQUIRED: [(&str, &[&str]); 2] =
    [("windows-x64", &["llvm-mingw", "cpm", "sdl"]), ("linux-x64", &["llvm", "cpm", "ffmpeg", "sdl2", "sysroot"])];
/// The store's owner: each archive is a file of a generic package of this organization.
pub const OWNER: &str = "MagicBeanDev";
/// The store's own address.
pub const SERVER: &str = "https://git.magicbean.dev";
/// The same files from Cloudflare's edge (git-cdn serves the server's package paths from its R2 store, cached).
pub const CDN: &str = "https://git-cdn.magicbean.dev";
/// A tool archive's extension.
pub const EXT: &str = ".tar.zst";

/// One line of the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    /// Its name (`[a-z0-9][a-z0-9._-]{0,63}`).
    pub name: String,
    /// Its version, part of its store address.
    pub version: String,
    /// One of [`SYSTEMS`].
    pub system: String,
    /// The archive's SHA-256, lower-case hex.
    pub sha256: String,
    /// The archive's size in bytes.
    pub size: u64,
    /// Where it is unpacked, under the bundle (`tools/<name>`).
    pub dest: String,
    /// The archive's folder that becomes `dest` (`-`: its top).
    pub prefix: String,
    /// Where it is downloaded from, in order: the store's two ([`Tool::store_urls`]), then any mirrors.
    pub urls: Vec<String>,
}

impl Tool {
    /// The line as the lock has it.
    pub fn line(&self) -> String {
        format!(
            "tool {} {} {} {} {} {} {} {}",
            self.name,
            self.version,
            self.system,
            self.sha256,
            self.size,
            self.dest,
            self.prefix,
            self.urls.join(" ")
        )
    }

    /// What the archive's member names start with for what becomes `dest` (empty: all of it).
    pub fn lead(&self) -> String {
        if self.prefix == "-" { String::new() } else { format!("{}/", self.prefix.trim_end_matches('/')) }
    }

    /// Its copy in Longshot's store, at `server` (the address the store is reached by): the generic package
    /// `<name>-<system>`, its version the tool's, the file `<sha256>-<name>.tar.zst`. A package version's files never
    /// change (the store refuses a second upload of a name) and nothing prunes them.
    pub fn store_url(&self, server: &str) -> String {
        format!(
            "{server}/api/packages/{OWNER}/generic/{}-{}/{}/{}-{}{EXT}",
            self.name, self.system, self.version, self.sha256, self.name
        )
    }

    /// The lock's URLs for it in the store: the edge's copy, then the store's own address.
    pub fn store_urls(&self) -> Vec<String> {
        vec![self.store_url(CDN), self.store_url(SERVER)]
    }
}

fn name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(c))
}

fn version_ok(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(c))
}

/// A relative path with `/` that stays inside the folder it is under ([`build_core::fsx::relative_ok`]), each part of
/// `[0-9A-Za-z._+-]`: a tool's folders and its archive's members (the carried tree's members take any name the
/// shared rule does: a font of the tree has a `&` in its name).
pub fn path_ok(p: &str) -> bool {
    build_core::fsx::relative_ok(p) && p.bytes().all(|c| c == b'/' || c.is_ascii_alphanumeric() || b"._+-".contains(&c))
}

/// The lock's lines, each checked (name, version, system, SHA-256, size, a path that leaves the bundle, a link that is
/// not https or not a .tar.zst, first URLs that are not the store's, a tool twice for one system); comments and blank
/// lines skipped.
pub fn parse(text: &str) -> Result<Vec<Tool>, String> {
    let mut tools: Vec<Tool> = Vec::new();
    for (n, line) in text.lines().enumerate().map(|(i, l)| (i + 1, l)) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        if w[0] != "tool" || w.len() < 9 {
            return Err(format!(
                "tools.lock line {n}: not 'tool <name> <version> <system> <sha256> <size> <dest> <prefix> <url>...'"
            ));
        }
        let size = w[5].parse::<u64>().ok().filter(|_| w[5].bytes().all(|c| c.is_ascii_digit()));
        let checks = [
            (name_ok(w[1]), "name"),
            (version_ok(w[2]), "version"),
            (SYSTEMS.contains(&w[3]), "system"),
            (w[4].len() == 64 && w[4].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "sha256"),
            (size.is_some_and(|s| s > 0 && s < 1 << 34), "size"),
            (path_ok(w[6]), "dest"),
            (w[7] == "-" || path_ok(w[7]), "prefix"),
            (w[8..].iter().all(|u| u.starts_with("https://") && u.ends_with(EXT)), "url"),
        ];
        let problems: Vec<&str> = checks.iter().filter(|(ok, _)| !ok).map(|(_, why)| *why).collect();
        if !problems.is_empty() {
            return Err(format!("tools.lock line {n} ({}): bad {}", w[1], problems.join(", ")));
        }
        if tools.iter().any(|t| t.name == w[1] && t.system == w[3]) {
            return Err(format!("tools.lock line {n}: {} for {} twice", w[1], w[3]));
        }
        let t = Tool {
            name: w[1].into(),
            version: w[2].into(),
            system: w[3].into(),
            sha256: w[4].into(),
            size: size.unwrap_or_default(),
            dest: w[6].into(),
            prefix: w[7].into(),
            urls: w[8..].iter().map(|u| u.to_string()).collect(),
        };
        if t.urls.get(..2) != Some(&t.store_urls()[..]) {
            return Err(format!(
                "tools.lock line {n} ({}): its first URLs are not Longshot's store's, {}",
                t.name,
                t.store_urls().join(" ")
            ));
        }
        tools.push(t);
    }
    Ok(tools)
}

/// The lock at `path`, read and checked.
pub fn load(path: &Path) -> Result<Vec<Tool>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text)
}

/// The tool `name` for `system`.
pub fn named<'a>(tools: &'a [Tool], name: &str, system: &str) -> Result<&'a Tool, String> {
    tools
        .iter()
        .find(|t| t.name == name && t.system == system)
        .ok_or_else(|| format!("no {name} for {system} in tools.lock"))
}

/// The tools each system needs that the lock lacks, as `<name> (<system>)`.
pub fn missing_required(tools: &[Tool]) -> Vec<String> {
    REQUIRED
        .iter()
        .flat_map(|(s, names)| names.iter().map(move |n| (*s, *n)))
        .filter(|(s, n)| !tools.iter().any(|t| t.name == *n && t.system == *s))
        .map(|(s, n)| format!("{n} ({s})"))
        .collect()
}

/// `tool`'s line written into the lock at `path` in place of the one with its name and system, or added at the end;
/// the comments stay. The lock is checked whole before it is written.
pub fn write_line(tool: &Tool, path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() > 3 && w[0] == "tool" && w[1] == tool.name && w[3] == tool.system {
                found = true;
                tool.line()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(tool.line());
    }
    let new = out.join("\n") + "\n";
    parse(&new)?;
    build_core::fsx::write_atomic(path, new.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A line with the store's URLs, as `dev tools add` writes one.
    pub(crate) fn line(name: &str, version: &str, system: &str, sha: &str, size: u64) -> String {
        let mut t = Tool {
            name: name.into(),
            version: version.into(),
            system: system.into(),
            sha256: sha.into(),
            size,
            dest: format!("tools/{name}"),
            prefix: "-".into(),
            urls: vec![],
        };
        t.urls = t.store_urls();
        t.line()
    }

    #[test]
    fn the_repository_lock() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools.lock");
        let lock = load(&path).unwrap();
        for (s, names) in REQUIRED {
            let mut have: Vec<&str> = lock.iter().filter(|t| t.system == s).map(|t| t.name.as_str()).collect();
            let mut want = names.to_vec();
            have.sort();
            want.sort();
            assert_eq!(have, want, "{s}");
        }
        for t in &lock {
            // (at tools/<name> in the builder's bundle, where ToolVars finds it; the archive's files at its top; only
            // Longshot's store, the edge's copy first)
            assert_eq!((t.dest.as_str(), t.prefix.as_str()), (format!("tools/{}", t.name).as_str(), "-"), "{}", t.name);
            assert_eq!(t.urls, t.store_urls(), "{}", t.name);
            assert!(t.urls[0].starts_with(&format!("{CDN}/api/packages/")), "{}", t.name);
        }
        // (no emsdk, no third-party, no Python: llvm, the SDL DLLs from libsdl's releases, the builder in Rust; play
        // profiles are committed in assets/profiles/, not tools)
        for gone in ["emsdk", "third-party", "python", "profile"] {
            assert!(!lock.iter().any(|t| t.name == gone), "{gone}");
            assert!(!REQUIRED.iter().any(|(_, n)| n.contains(&gone)), "{gone}");
        }
    }

    #[test]
    fn a_bad_line_is_refused() {
        let good = line("zstd", "1.0", "windows-x64", &"a".repeat(64), 10);
        assert_eq!(parse(&good).unwrap()[0].prefix, "-");
        // (an indented comment is a comment, and an indented line a line)
        assert_eq!(parse(&format!("  # a note\n\t{good}")).unwrap().len(), 1);
        assert!(!path_ok("a:b") && !path_ok("a/../b") && !path_ok("a\\b") && path_ok("tools/llvm-22"));
        assert_eq!(parse(&format!("{good} https://mirror/z.tar.zst")).unwrap()[0].urls.len(), 3, "a mirror after");
        let a64 = "a".repeat(64);
        for bad in [
            good.replace("https://", "http://"),
            good.replace(".tar.zst", ".zip"),
            good.replace(&a64, &"A".repeat(64)),
            good.replace("https://git-cdn.magicbean.dev", "https://elsewhere.example"),
            good.replace(" tools/zstd - https", " tools/zstd - https://elsewhere.example/z.tar.zst https"),
            good.replace(" 10 ", " -1 "),
            good.replace("tools/zstd", "../zstd"),
            good.replace("tools/zstd", "/etc"),
            good.replace(" - ", " a/../b "),
            good.replace("windows-x64", "mac"),
            format!("{good}\n{good}"),
            "part x".into(),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn systems_are_apart_and_a_line_is_replaced_in_place() {
        let d = crate::scratch("tools-lock");
        let lock = d.join("tools.lock");
        let lines = [
            "# a comment".to_string(),
            line("cpm", "1", "windows-x64", &"a".repeat(64), 10),
            line("cpm", "1", "linux-x64", &"b".repeat(64), 11),
        ];
        std::fs::write(&lock, lines.join("\n") + "\n").unwrap();
        let both = load(&lock).unwrap();
        assert_eq!(
            (named(&both, "cpm", "windows-x64").unwrap().size, named(&both, "cpm", "linux-x64").unwrap().size),
            (10, 11)
        );
        let mut t = named(&both, "cpm", "linux-x64").unwrap().clone();
        t.version = "2".into();
        assert!(write_line(&t, &lock).is_err(), "a new version at the old version's URLs");
        t.urls = t.store_urls();
        write_line(&t, &lock).unwrap();
        let after: Vec<(String, String)> = load(&lock).unwrap().into_iter().map(|t| (t.system, t.version)).collect();
        assert_eq!(after, [("windows-x64".to_string(), "1".to_string()), ("linux-x64".into(), "2".into())]);
        assert!(std::fs::read_to_string(&lock).unwrap().starts_with("# a comment\n"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_store_is_the_package_registry() {
        let t = parse(&line("llvm", "23.1.5-x64-1", "linux-x64", &"a".repeat(64), 10)).unwrap().remove(0);
        assert_eq!(
            t.store_url(SERVER),
            format!(
                "https://git.magicbean.dev/api/packages/MagicBeanDev/generic/llvm-linux-x64/23.1.5-x64-1/{}-llvm.tar.zst",
                "a".repeat(64)
            )
        );
    }

    #[test]
    fn a_lock_without_a_needed_tool_says_which() {
        let mut lines = String::new();
        for (s, names) in REQUIRED {
            for n in names.iter().filter(|n| !(s == "linux-x64" && **n == "sysroot")) {
                lines += &(line(n, "1", s, &"a".repeat(64), 10) + "\n");
            }
        }
        assert_eq!(missing_required(&parse(&lines).unwrap()), ["sysroot (linux-x64)"]);
    }
}
