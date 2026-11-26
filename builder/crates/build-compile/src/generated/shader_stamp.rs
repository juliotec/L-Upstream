//! What the shader caches are good for: a stamp of the shader translator's sources,
//! compiled into the renderer (`LONGSHOT_SHADER_STAMP`). The program cache, the prepare record and the pipeline caches
//! are dropped whole when it changes, and every player translates every program again, so it holds the sources that can
//! change a translated program and nothing else:
//!
//! - `gpu/vk/eden`, `gpu/vk/sirit`, `gpu/vk/compat`, every file: the recompiler, the SPIR-V emitter, what they build with;
//! - `vk_shader.cpp` (what drives them), `vk_bank.cpp` (the constant banks they read), `vk_state.h` (what those read).
//!
//! A C or C++ file is stamped by its tokens ([`normal`]): its comments, blank lines and indentation leave the stamp as
//! it is, as they leave the compiled translator. Any other file, and one the scanner is not sure of, byte for byte, so a
//! doubt can only make the stamp move more often, never less. Windows line ends count as Unix ones.

use super::version::crlf_to_lf;
use build_core::{IoContext, Result};
use std::path::{Path, PathBuf};

const DIRS: [&str; 3] = ["eden", "sirit", "compat"];
const FILES: [&str; 3] = ["vk_shader.cpp", "vk_bank.cpp", "vk_state.h"];
const C_FAMILY: [&str; 9] = [".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".inc", ".inl"];
/// Before a normal form in the hash, so it is never taken for a file's bytes.
const NORMAL_TAG: &[u8] = b"\0tokens 1\0";
/// A comment over lines between two tokens: whether a `#` after it starts a directive is the compiler's choice, so it
/// is neither a space nor a line break.
const SPLIT: &[u8] = b" /*\n*/ ";

/// A text the scanner does not read with certainty.
struct Unsure;

fn ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// The end of the literal whose quote is at `i`; a line break or splice in it, or no end, is unsure.
fn literal(d: &[u8], i: usize) -> std::result::Result<usize, Unsure> {
    let q = d[i];
    let mut j = i + 1;
    while j < d.len() {
        match d[j] {
            c if c == q => return Ok(j + 1),
            b'\n' => return Err(Unsure),
            b'\\' => {
                if j + 1 >= d.len() || d[j + 1] == b'\n' {
                    return Err(Unsure);
                }
                j += 2;
            }
            _ => j += 1,
        }
    }
    Err(Unsure)
}

/// A header name in angle brackets holding a comment marker or a quote (the scanner would misread it).
fn angled_unsure(d: &[u8]) -> bool {
    let text = String::from_utf8_lossy(d);
    let bad = |inner: &str| inner.contains("//") || inner.contains("/*") || inner.contains('"') || inner.contains('\'');
    for line in text.lines() {
        let t = line.trim_start_matches([' ', '\t']);
        if let Some(rest) = t.strip_prefix('#') {
            let rest = rest.trim_start_matches([' ', '\t']);
            for kw in ["include_next", "include", "import"] {
                if let Some(r) = rest.strip_prefix(kw) {
                    let r = r.trim_start_matches([' ', '\t']);
                    if let Some(inner) = r.strip_prefix('<')
                        && bad(inner.split('>').next().unwrap_or(""))
                    {
                        return true;
                    }
                    break;
                }
            }
        }
    }
    let mut at = 0;
    while let Some(i) = text[at..].find("__has_include") {
        let r = text[at + i + 13..].trim_start_matches([' ', '\t']);
        if let Some(r) = r.strip_prefix('(')
            && let Some(inner) = r.trim_start_matches([' ', '\t']).strip_prefix('<')
        {
            let inner: String = inner.chars().take_while(|&c| c != '>' && c != '\n').collect();
            if bad(&inner) {
                return true;
            }
        }
        at += i + 13;
    }
    false
}

/// A backslash, blanks, a line end: GCC and clang splice it (and warn).
fn blank_splice(d: &[u8]) -> bool {
    d.iter().enumerate().any(|(i, &b)| {
        b == b'\\' && {
            let n = d[i + 1..].iter().take_while(|&&c| matches!(c, b' ' | b'\t' | 0x0c | 0x0b)).count();
            n > 0 && d.get(i + 1 + n) == Some(&b'\n')
        }
    })
}

fn width(s: Option<&[u8]>) -> u8 {
    match s {
        None => 0,
        Some(b" ") => 1,
        Some(b"\n") => 3,
        Some(_) => 2,
    }
}

/// A C or C++ file's tokens, as translation phase 3 leaves them: each comment a space, a run of white space a space or
/// one line break when it holds one, a comment over lines between tokens [`SPLIT`]; literals and splices as they are.
fn tokens(d: &[u8]) -> std::result::Result<Vec<u8>, Unsure> {
    if angled_unsure(d) || blank_splice(d) {
        return Err(Unsure);
    }
    let mut out: Vec<u8> = Vec::new();
    let mut space: Option<&[u8]> = None;
    let (n, mut i) = (d.len(), 0);
    fn widen<'a>(space: &mut Option<&'a [u8]>, to: &'a [u8]) {
        if width(Some(to)) > width(*space) {
            *space = Some(to);
        }
    }
    fn token(out: &mut Vec<u8>, space: &mut Option<&[u8]>, b: &[u8]) {
        if let Some(s) = space.take()
            && !out.is_empty()
        {
            out.extend_from_slice(s);
        }
        out.extend_from_slice(b);
    }
    while i < n {
        let c = d[i];
        if c == b' ' || c == b'\t' {
            widen(&mut space, b" ");
            i += 1;
        } else if c == b'\n' {
            widen(&mut space, b"\n");
            i += 1;
        } else if c == b'\\' && d[i..].starts_with(b"\\\n") {
            if (space.is_none() && out.last().is_some_and(|b| b"/*".contains(b)))
                || d.get(i + 2).is_some_and(|b| b"/*".contains(b))
            {
                return Err(Unsure);
            }
            token(&mut out, &mut space, b"\\\n");
            i += 2;
        } else if d[i..].starts_with(b"//") {
            let j = d[i..].iter().position(|&b| b == b'\n').map_or(n, |p| i + p);
            if d[j - 1] == b'\\' {
                return Err(Unsure);
            }
            widen(&mut space, b" ");
            i = j;
        } else if d[i..].starts_with(b"/*") {
            let j = find(&d[i + 2..], b"*/").map(|p| i + 2 + p).ok_or(Unsure)?;
            if find(&d[i + 2..j], b"\\\n").is_some() {
                return Err(Unsure);
            }
            widen(&mut space, if d[i + 2..j].contains(&b'\n') { SPLIT } else { b" " });
            i = j + 2;
        } else if c == b'"' || c == b'\'' {
            let j = literal(d, i)?;
            token(&mut out, &mut space, &d[i..j]);
            i = j;
        } else if c.is_ascii_digit() || (c == b'.' && d.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let mut j = i + 1;
            while j < n {
                let e = d[j];
                if b"eEpP".contains(&e) && d.get(j + 1).is_some_and(|b| b"+-".contains(b)) {
                    j += 2;
                } else if ident(e) || e == b'.' {
                    j += 1;
                } else if e == b'\'' && d.get(j + 1).is_some_and(|&b| ident(b)) {
                    j += 2;
                } else {
                    break;
                }
            }
            token(&mut out, &mut space, &d[i..j]);
            i = j;
        } else if ident(c) {
            let mut j = i + 1;
            while j < n && ident(d[j]) {
                j += 1;
            }
            let word = &d[i..j];
            if d.get(j) == Some(&b'"') && [b"R".as_slice(), b"u8R", b"uR", b"UR", b"LR"].contains(&word) {
                return Err(Unsure);
            }
            if d.get(j).is_some_and(|b| b"\"'".contains(b)) && [b"u8".as_slice(), b"u", b"U", b"L"].contains(&word) {
                let k = literal(d, j)?;
                token(&mut out, &mut space, &d[i..k]);
                i = k;
            } else {
                token(&mut out, &mut space, word);
                i = j;
            }
        } else {
            token(&mut out, &mut space, &d[i..i + 1]);
            i += 1;
        }
    }
    Ok(out)
}

/// What the stamp hashes of a file (`rel`, its path below gpu/vk; `data`, Unix line ends).
pub fn normal(rel: &str, data: &[u8]) -> Vec<u8> {
    let lower = rel.to_lowercase();
    if !C_FAMILY.iter().any(|e| lower.ends_with(e)) {
        return data.to_vec();
    }
    match tokens(data) {
        Ok(t) => [NORMAL_TAG, &t].concat(),
        Err(Unsure) => data.to_vec(),
    }
}

/// The stamped files below gpu/vk, in the stamp's order: the folders' files sorted, then [`FILES`] as listed.
pub fn files(vk: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = DIRS
        .iter()
        .flat_map(|d| {
            build_core::fsx::files_under(&vk.join(d)).unwrap_or_default().into_iter().map(move |r| vk.join(d).join(r))
        })
        .collect();
    found.sort();
    found.extend(FILES.iter().map(|f| vk.join(f)));
    found
}

/// `LONGSHOT_SHADER_STAMP`'s 16 hex digits for the translator below `vk`.
pub fn stamp(vk: &Path) -> Result<String> {
    let mut h = sha1_smol::Sha1::new();
    for f in files(vk) {
        let rel = f.strip_prefix(vk).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        let data = std::fs::read(&f).at(&f)?;
        h.update(rel.as_bytes());
        h.update(&normal(&rel, &crlf_to_lf(&data)));
    }
    Ok(h.digest().to_string()[..16].to_string())
}

/// The first place `needle` is in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_spacing_leave_the_tokens_as_they_are() {
        let a = normal("x.cpp", b"int  a = 1; // one\n\n\nint b;\n");
        let b = normal("x.cpp", b"int a = 1;\n    int b; /* two */\n");
        assert_eq!(a, b);
        assert_ne!(a, normal("x.cpp", b"int a = 2;\nint b;\n"));
        assert_eq!(normal("x.txt", b"a  b"), b"a  b", "not C: its bytes");
        assert_eq!(normal("x.cpp", b"auto s = R\"(x)\";"), b"auto s = R\"(x)\";", "a raw string: unsure");
        assert_eq!(&normal("x.h", b"#include <a//b>\n")[..], b"#include <a//b>\n");
        assert_eq!(&normal("x.h", b"a /* x\n y */ b")[NORMAL_TAG.len()..], b"a /*\n*/ b");
    }
}
