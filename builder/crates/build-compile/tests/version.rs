//! Which Longshot a build is: where the release name comes from (`LONGSHOT_VERSION`, then a `VERSION` or
//! `version.txt` file, then `git describe` of a version tag, else "unknown"), and a fingerprint of the sources that
//! line ends, other files and build folders leave as it is.

use build_compile::generated::version::{describe, fingerprint, release};
use std::path::{Path, PathBuf};
use std::process::Command;

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("version").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn tree(root: &Path, eol: &str) {
    for rel in ["host/a.c", "gpu/vk/b.cpp", "builder/crates/x/src/lib.rs", "builder/crates/x/Cargo.toml"] {
        write(root, rel, &format!("line one{eol}line two{eol}"));
    }
}

#[test]
fn the_release_is_the_variable_then_the_file_then_unknown() {
    let d = dir("release");
    assert_eq!(release(&d, None), "unknown", "no variable, no file, no checkout");
    assert_eq!(release(&d, Some("  ")), "unknown", "a blank variable is none");
    write(&d, "version.txt", "from-txt\n");
    assert_eq!(release(&d, None), "from-txt");
    write(&d, "VERSION", "  movie-sync  \nignored second line\n");
    assert_eq!(release(&d, None), "movie-sync", "VERSION before version.txt, its first line trimmed");
    assert_eq!(release(&d, Some(" from-env\n")), "from-env", "LONGSHOT_VERSION comes first");
    write(&d, "VERSION", "\n");
    assert_eq!(release(&d, None), "from-txt", "an empty VERSION is passed over");
}

#[test]
fn a_checkout_is_named_by_its_version_tag() {
    let d = dir("git");
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&d)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) {
        panic!("git is needed for this check");
    }
    write(&d, "host/a.c", "int a;\n");
    assert!(git(&["add", "-A"]) && git(&["commit", "-q", "-m", "one"]));
    assert!(git(&["tag", "nightly-1"]) && git(&["tag", "v1.2.0"]));
    assert_eq!(release(&d, None), "v1.2.0", "a version tag, not another");
    write(&d, "host/a.c", "int b;\n");
    assert_eq!(release(&d, None), "v1.2.0-dirty");
    write(&d, "VERSION", "file\n");
    assert_eq!(release(&d, None), "file", "a file before the checkout");
}

#[test]
fn the_fingerprint_is_the_sources_and_not_their_line_ends() {
    let (lf, crlf) = (dir("lf"), dir("crlf"));
    tree(&lf, "\n");
    tree(&crlf, "\r\n");
    let one = fingerprint(&lf);
    assert_eq!(one.len(), 8);
    assert!(one.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(fingerprint(&crlf), one, "line ends changed the fingerprint");

    for (rel, why) in [
        ("host/notes.md", "not a source"),
        ("host/__pycache__/x.py", "a cache"),
        ("builder/crates/x/target/debug/y.rs", "a build folder"),
        ("docs/z.c", "outside host, gpu and builder/crates"),
    ] {
        write(&lf, rel, "anything\n");
        assert_eq!(fingerprint(&lf), one, "{rel}: {why}");
    }
    for rel in ["host/a.c", "gpu/vk/new.glsl", "builder/crates/x/Cargo.toml"] {
        let before = fingerprint(&lf);
        write(&lf, rel, "changed\n");
        assert_ne!(fingerprint(&lf), before, "{rel} changed and the fingerprint did not");
    }
    std::fs::rename(lf.join("host/a.c"), lf.join("host/a2.c")).unwrap();
    let renamed = fingerprint(&lf);
    std::fs::rename(lf.join("host/a2.c"), lf.join("host/a.c")).unwrap();
    assert_ne!(renamed, fingerprint(&lf), "a file's name counts");
}

#[test]
fn the_description_has_no_character_a_command_line_reads() {
    let d = dir("describe");
    tree(&d, "\n");
    let text = describe(&d, Some("v1 \"quoted\" \\ & | %PATH% ; $(x) `y`"));
    assert!(text.starts_with("v1 _quoted_ _ _ _ _PATH_ _ _(x) _y_ (source "), "{text}");
    assert!(!text.contains(['"', '\\', '&', '|', '%', ';', '$', '`']), "{text}");
    assert!(describe(&d, Some(&"v".repeat(200))).len() <= 80);
}
