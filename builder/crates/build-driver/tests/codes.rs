//! Every code the builder's crates can say (an error's or a warning's) is in build-events.md's table of codes, and
//! every code there is one they can say: a launcher keys its own words by them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The code-looking literal (`[a-z][a-z0-9_]*`) that starts `s`, after blanks.
fn literal(s: &str) -> Option<&str> {
    let s = s.trim_start().strip_prefix('"')?;
    let end = s.find('"')?;
    let code = &s[..end];
    (code.chars().next()?.is_ascii_lowercase()
        && code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
    .then_some(code)
}

/// The codes said in `text` (a source file's code, its tests cut off): the first argument of `Error::player(` and of
/// `.warn(` and `warn(`, the code argument of build-core's lock (`hold(`), an `Event::Error`'s `code:`, and
/// build-core's own codes (`=> "io"`, in error.rs).
fn said(text: &str, out: &mut BTreeSet<String>) {
    for marker in ["Error::player(", ".warn(", " warn(", "Event::Error { code: "] {
        for (i, _) in text.match_indices(marker) {
            if let Some(c) = literal(&text[i + marker.len()..]) {
                out.insert(c.to_string());
            }
        }
    }
    for (i, _) in text.match_indices("hold(") {
        let args = &text[i..text[i..].find(')').map_or(text.len(), |e| i + e)];
        if let Some(c) = args.split(',').filter_map(literal).find(|c| c.ends_with("_busy")) {
            out.insert(c.to_string());
        }
    }
    for (i, _) in text.match_indices("} => ") {
        if let Some(c) = literal(&text[i + 5..]) {
            out.insert(c.to_string());
        }
    }
}

#[test]
fn every_code_said_is_in_build_events_md_and_every_code_there_is_said() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for c in ["build-core", "build-driver", "build-compile", "build-assets", "game-files", "longshot-builder", "lift"] {
        sources(&crates.join(c).join("src"), &mut files);
    }
    let mut code = BTreeSet::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let text = text.split("#[cfg(test)]\nmod tests").next().unwrap();
        // (build-core's own codes are its error.rs's; elsewhere `} => "..."` is a match of something else)
        if f.ends_with("build-core/src/error.rs") {
            said(text, &mut code);
        } else {
            said(&text.replace("} => ", "}=>"), &mut code);
        }
    }
    let doc = std::fs::read_to_string(crates.join("../../docs/dev/reference/build-events.md")).unwrap();
    let section = &doc[doc.find("## Codes").unwrap()..doc.find("## Exit status").unwrap()];
    let mut listed = BTreeSet::new();
    for row in section.lines().filter(|l| l.starts_with("| ") && !l.starts_with("| Where")) {
        let codes = row.splitn(3, '|').nth(2).unwrap();
        // (a code's own words, in brackets after it, name no code)
        let mut depth = 0;
        let bare: String = codes
            .chars()
            .filter(|&c| {
                depth += i32::from(c == '(') - i32::from(c == ')');
                depth == 0 && c != ')'
            })
            .collect();
        for part in bare.split('`').skip(1).step_by(2) {
            listed.insert(part.to_string());
        }
    }
    assert!(code.len() > 40, "the sources' codes were not found: {code:?}");
    let missing: Vec<&String> = code.difference(&listed).collect();
    let stale: Vec<&String> = listed.difference(&code).collect();
    assert!(missing.is_empty(), "said by the builder, not in build-events.md: {missing:?}");
    assert!(stale.is_empty(), "in build-events.md, said by no crate: {stale:?}");
}
