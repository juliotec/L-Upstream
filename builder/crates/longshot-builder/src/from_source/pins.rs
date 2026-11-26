//! What a commit's source pins for building its own builder, read from its files as its release workflow reads them
//! (scripts/longshot_builder.py, scripts/renhdrum.py): the Rust, `RUST_VERSION` and each part's SHA-256 in
//! scripts/renhdrum.py's `RUST` and `RUST_STD` (one Python literal per part: `("<name>-%s-<triple>" % RUST_VERSION,
//! "<sha256>")`), and mise.toml's `rust` version, which a developer's checkout uses (said when it is not the same).
//! These are the source's own pins: a pull request's are the pull request's.

use std::collections::BTreeMap;

/// The Rust target the builder is built for on this system: what the release builds (scripts/longshot_builder.py's
/// TARGETS: Linux against glibc, Windows with llvm-mingw's gnullvm).
pub fn triple(windows: bool) -> &'static str {
    if windows { "x86_64-pc-windows-gnullvm" } else { "x86_64-unknown-linux-gnu" }
}

/// One downloaded part of a Rust release (static.rust-lang.org/dist/`<name>`.tar.xz).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// Its name: `rustc-1.99.1-x86_64-unknown-linux-gnu`.
    pub name: String,
    /// Its archive's SHA-256, as the source pins it.
    pub sha256: String,
}

/// The Rust a source pins: its version and every part it names a SHA-256 for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustPins {
    /// `RUST_VERSION`.
    pub version: String,
    /// Each part's SHA-256, by its name (`%s` filled in).
    pub parts: BTreeMap<String, String>,
}

impl RustPins {
    /// The parts a builder for this system is built with: rustc and cargo for this system, and its own standard
    /// library (the builder is built for the system it runs on), in that order.
    pub fn for_host(&self, windows: bool) -> Result<Vec<Part>, String> {
        let t = triple(windows);
        ["rustc", "cargo", "rust-std"]
            .iter()
            .map(|p| {
                let name = format!("{p}-{}-{t}", self.version);
                let sha256 = self.parts.get(&name).ok_or_else(|| format!("scripts/renhdrum.py pins no {name}"))?;
                Ok(Part { name, sha256: sha256.clone() })
            })
            .collect()
    }
}

fn quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix('"')?;
    let end = s.find('"')?;
    Some((&s[..end], &s[end + 1..]))
}

/// The pins of scripts/renhdrum.py's text: `RUST_VERSION = "<v>"`, and each `("<part>-%s-<triple>" % RUST_VERSION,
/// "<sha256>")` (a line break may come after the comma). A name pinned twice with two SHA-256s is refused.
pub fn rust_pins(renhdrum_py: &str) -> Result<RustPins, String> {
    let version = renhdrum_py
        .lines()
        .find_map(|l| l.trim().strip_prefix("RUST_VERSION = ").and_then(quoted).map(|(v, _)| v.to_string()))
        .ok_or("scripts/renhdrum.py has no RUST_VERSION")?;
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("scripts/renhdrum.py's RUST_VERSION {version:?} is not a version"));
    }
    let mut parts = BTreeMap::new();
    for (i, _) in renhdrum_py.match_indices("(\"") {
        let Some((name, rest)) = quoted(&renhdrum_py[i + 1..]) else { continue };
        let ok = name.contains("%s") && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"%-_.".contains(&b));
        let Some(rest) = rest.trim_start().strip_prefix('%').map(str::trim_start) else { continue };
        let Some(rest) = rest.strip_prefix("RUST_VERSION").map(str::trim_start) else { continue };
        let Some((sha, _)) = rest.strip_prefix(',').map(str::trim_start).and_then(quoted) else { continue };
        if !ok || sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err(format!("scripts/renhdrum.py's pin of {name:?} is not a part's name and a SHA-256"));
        }
        let name = name.replace("%s", &version);
        if let Some(other) = parts.insert(name.clone(), sha.to_string()).filter(|o| o != sha) {
            return Err(format!("scripts/renhdrum.py pins {name} twice ({other}, {sha})"));
        }
    }
    if parts.is_empty() {
        return Err("scripts/renhdrum.py pins no part of Rust".into());
    }
    Ok(RustPins { version, parts })
}

/// mise.toml's `rust` version (`rust = "<v>"` or `rust = { version = "<v>", ... }` in its `[tools]`), if it has one.
pub fn mise_rust(mise_toml: &str) -> Option<String> {
    let mut tools = false;
    for line in mise_toml.lines().map(str::trim) {
        if line.starts_with('[') {
            tools = line == "[tools]";
            continue;
        }
        let Some(rest) = line.strip_prefix("rust").map(str::trim_start).and_then(|r| r.strip_prefix('=')) else {
            continue;
        };
        if !tools {
            continue;
        }
        let rest = rest.trim_start();
        if let Some((v, _)) = quoted(rest) {
            return Some(v.to_string());
        }
        let at = rest.find("version")?;
        let after = rest[at + "version".len()..].trim_start().strip_prefix('=')?.trim_start();
        return quoted(after).map(|(v, _)| v.to_string());
    }
    None
}

/// The string literals of the constant `name` (`pub const <name>: [&str; <n>] = [ "...", ... ];`) in a Rust source's
/// text: what the source's build-driver/src/sources.rs says a release carries of its tree (`TREE`) and leaves out of it
/// (`LEFT_OUT`), its builder's pack being made by this builder's writer. `None` when the text has no such constant.
pub fn str_list(rust: &str, name: &str) -> Option<Vec<String>> {
    let at = rust.find(&format!("pub const {name}: [&str; "))?;
    let body = &rust[at..];
    let open = body.find("= [")? + 3;
    let close = open + body[open..].find("];")?;
    let mut out = Vec::new();
    let mut rest = &body[open..close];
    while let Some(i) = rest.find('"') {
        let (s, after) = quoted(&rest[i..])?;
        out.push(s.to_string());
        rest = after;
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_a_source_carries_is_read_from_its_sources_rs() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../build-driver/src/sources.rs"),
        )
        .unwrap();
        let tree = str_list(&text, "TREE").unwrap();
        assert_eq!(tree, build_driver::sources::TREE.map(String::from), "this tree's own, read as a source's");
        let left = str_list(&text, "LEFT_OUT").unwrap();
        assert_eq!(left, build_driver::sources::LEFT_OUT.map(String::from));
        assert_eq!(
            str_list("pub const TREE: [&str; 2] = [\n    \"a\",\n    \"b/c\", // x\n];", "TREE").unwrap(),
            ["a", "b/c"]
        );
        assert_eq!(str_list("const X: u8 = 1;", "TREE"), None);
    }

    fn h(c: char) -> String {
        c.to_string().repeat(64)
    }

    /// scripts/renhdrum.py's shape: the parts on one line each, and one whose SHA-256 is on the next line.
    fn renhdrum(version: &str) -> String {
        format!(
            "RUST_VERSION = \"{version}\"\nDIST = \"https://static.rust-lang.org/dist/\"\nRUST = {{\n    \"linux\": [\n        \
             (\"rustc-%s-x86_64-unknown-linux-gnu\" % RUST_VERSION, \"{a}\"),\n        \
             (\"cargo-%s-x86_64-unknown-linux-gnu\" % RUST_VERSION, \"{b}\"),\n    ],\n    \"windows\": [\n        \
             (\"rustc-%s-x86_64-pc-windows-gnullvm\" % RUST_VERSION, \"{c}\"),\n        \
             (\"cargo-%s-x86_64-pc-windows-gnullvm\" % RUST_VERSION, \"{d}\"),\n    ],\n}}\nRUST_STD = {{\n    \
             \"windows\": (\"rust-std-%s-x86_64-pc-windows-gnullvm\" % RUST_VERSION,\n                \"{e}\"),\n    \
             \"linux\": (\"rust-std-%s-x86_64-unknown-linux-gnu\" % RUST_VERSION,\n              \"{f}\"),\n}}\n\
             EXPORTS = tuple(\"renhdrum_\" + n for n in (\"version\", \"x\"))\n",
            a = h('a'),
            b = h('b'),
            c = h('c'),
            d = h('d'),
            e = h('e'),
            f = h('f')
        )
    }

    #[test]
    fn the_rust_parts_are_read_per_system() {
        let p = rust_pins(&renhdrum("1.99.1")).unwrap();
        assert_eq!(p.version, "1.99.1");
        assert_eq!(p.parts.len(), 6);
        let linux = p.for_host(false).unwrap();
        assert_eq!(
            linux,
            [
                Part { name: "rustc-1.99.1-x86_64-unknown-linux-gnu".into(), sha256: h('a') },
                Part { name: "cargo-1.99.1-x86_64-unknown-linux-gnu".into(), sha256: h('b') },
                Part { name: "rust-std-1.99.1-x86_64-unknown-linux-gnu".into(), sha256: h('f') },
            ]
        );
        let windows = p.for_host(true).unwrap();
        assert_eq!(windows[2], Part { name: "rust-std-1.99.1-x86_64-pc-windows-gnullvm".into(), sha256: h('e') });
        assert_eq!(windows[0].sha256, h('c'));
    }

    #[test]
    fn the_real_renhdrum_py_is_read() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/renhdrum.py"),
        )
        .unwrap();
        let p = rust_pins(&text).unwrap();
        assert_eq!(p.for_host(false).unwrap().len(), 3);
        assert_eq!(p.for_host(true).unwrap().len(), 3);
        let mise = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mise.toml"))
            .unwrap();
        assert_eq!(mise_rust(&mise), Some(p.version), "mise.toml and scripts/renhdrum.py pin one Rust");
    }

    #[test]
    fn bad_pins_are_refused() {
        assert!(rust_pins("RUST = {}\n").is_err(), "no version");
        assert!(rust_pins("RUST_VERSION = \"1.99.1; rm\"\n").is_err());
        assert!(rust_pins("RUST_VERSION = \"1.99.1\"\n").is_err(), "no part");
        let short = format!("RUST_VERSION = \"1.0.0\"\n(\"rustc-%s-x\" % RUST_VERSION, \"{}\")\n", "a".repeat(63));
        assert!(rust_pins(&short).is_err());
        let twice = format!(
            "RUST_VERSION = \"1.0.0\"\n(\"rustc-%s-x\" % RUST_VERSION, \"{}\")\n(\"rustc-%s-x\" % RUST_VERSION, \"{}\")\n",
            h('a'),
            h('b')
        );
        assert!(rust_pins(&twice).unwrap_err().contains("twice"));
        let p = rust_pins(&format!("RUST_VERSION = \"1.0.0\"\n(\"rustc-%s-x\" % RUST_VERSION, \"{}\")\n", h('a')));
        assert!(
            p.unwrap().for_host(false).unwrap_err().contains("rustc-1.0.0-x86_64-unknown-linux-gnu"),
            "a part it needs"
        );
    }

    #[test]
    fn mise_s_rust_is_read_from_its_tools() {
        let t =
            "[tasks.x]\nrust = \"9\"\n[tools]\npython = \"3\"\nrust = { version = \"1.99.1\", components = \"x\" }\n";
        assert_eq!(mise_rust(t), Some("1.99.1".into()));
        assert_eq!(mise_rust("[tools]\nrust = \"1.98.0\"\n"), Some("1.98.0".into()));
        assert_eq!(mise_rust("[tools]\nrustfmt = \"1\"\n"), None);
        assert_eq!(mise_rust("[tasks.x]\nrust = \"1\"\n"), None, "not a tool");
    }
}
