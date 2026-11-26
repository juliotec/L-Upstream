//! Which Longshot a build is: the release it came from and a fingerprint of its sources, given to
//! `host/version.c` and `gpu/vk/vk_version.cpp` alone (two one-line files: a new version compiles them and nothing
//! else). The game prints it first in its log and in the Insert menu, so a report says what was run.
//!
//! The release: `LONGSHOT_VERSION`, else a `VERSION` or `version.txt` file at the root (a release's tools have no
//! history), else `git describe` of version tags in a checkout. The fingerprint hashes the game's sources and the
//! builder's own (`host/`, `gpu/`, `builder/crates/`: the new builder's lifter changes the game's code as much as `host/`
//! does), line ends aside, so even an "unknown" build can be traced to the tree it came from.

use std::path::Path;
use std::process::Command;

/// The folders the fingerprint covers, under the root.
const SOURCES: [&str; 3] = ["host", "gpu", "builder/crates"];
/// The files it covers in them: what an `#include` can name, the shaders, the builder's Rust.
const EXTS: [&str; 16] =
    ["c", "cc", "cpp", "h", "hpp", "inc", "inl", "def", "py", "glsl", "comp", "vert", "frag", "spv", "rs", "toml"];

/// The release's name.
pub fn release(root: &Path, env: Option<&str>) -> String {
    if let Some(v) = env.map(str::trim).filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    for file in ["VERSION", "version.txt"] {
        if let Ok(t) = std::fs::read_to_string(root.join(file))
            && let Some(l) = t.lines().next().map(str::trim).filter(|l| !l.is_empty())
        {
            return l.to_string();
        }
    }
    if root.join(".git").exists() {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["describe", "--tags", "--always", "--dirty", "--match", "v[0-9]*"])
            .output();
        if let Some(o) = out.ok().filter(|o| o.status.success()) {
            let name = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "unknown".to_string()
}

/// The sources' fingerprint: 8 hex digits.
pub fn fingerprint(root: &Path) -> String {
    let mut h = sha1_smol::Sha1::new();
    for top in SOURCES {
        let base = root.join(top);
        let Ok(files) = build_core::fsx::files_under(&base) else { continue };
        for rel in files {
            let skip = rel.components().any(|c| matches!(c.as_os_str().to_str(), Some("__pycache__" | "target")));
            let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
            if skip || !EXTS.contains(&ext) {
                continue;
            }
            let Ok(data) = std::fs::read(base.join(&rel)) else { continue };
            let name = format!("{top}/{}", rel.to_string_lossy().replace('\\', "/"));
            h.update(name.as_bytes());
            h.update(b"\0");
            h.update(&crlf_to_lf(&data));
            h.update(b"\0");
        }
    }
    h.digest().to_string()[..8].to_string()
}

/// Windows line ends as Unix ones.
pub fn crlf_to_lf(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// `<release> (source <fingerprint>)`, safe as a C string literal on any command line (80 characters at most).
pub fn describe(root: &Path, env: Option<&str>) -> String {
    let text = format!("{} (source {})", release(root, env), fingerprint(root));
    text.chars().map(|c| if c.is_ascii_alphanumeric() || " ._+()/-".contains(c) { c } else { '_' }).take(80).collect()
}

/// The define the two version files are compiled with.
pub fn define(description: &str) -> String {
    format!("-DLONGSHOT_VERSION=\"{description}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_is_safe_on_a_command_line() {
        let d = crate::testdir("version");
        std::fs::create_dir_all(d.join("host")).unwrap();
        std::fs::write(d.join("host/a.c"), "int a;\r\n").unwrap();
        let one = fingerprint(&d);
        std::fs::write(d.join("host/a.c"), "int a;\n").unwrap();
        assert_eq!(fingerprint(&d), one, "line ends aside");
        let text = describe(&d, Some("v1.2 \"x\""));
        assert_eq!(text, format!("v1.2 _x_ (source {one})"));
        assert_eq!(define("v"), "-DLONGSHOT_VERSION=\"v\"");
    }
}
