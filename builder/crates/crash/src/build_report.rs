//! A build reported to the Longshot team, at Longshot-api's /telemetry/build: LongshotLauncher's Core/BuildReports.cs
//! and Core/Privacy.cs, ported, the builder now the one that builds and reports. Two ways:
//!
//! - **Send** on the error window (only when the player presses it, after the page has shown every field of it): the
//!   failed build in full, with the update log's last [`LOG_LINES`] lines. The server writes it as one comment per
//!   failure on the build issue, counting its repeats.
//! - **Automatic statistics** (the player's yes, which a launcher passes: `--send-build-statistics`; never on by
//!   default): every build's end on its own, the same fields but no log; a failed or terminated one says why it
//!   stopped and its last error line, every game file's name taken out ([`diagnostic`]).
//!
//! Every report names its client: `longshot-builder`. No user name ([`scrub`]: the home folder as `%USERPROFILE%` or
//! `~`, the user's name elsewhere in a path as `<user>`), no game file, no save. What the builder can't observe (the
//! builder itself dying) stays the launcher's to report.

use crate::clean::json;
use std::fmt::Write as _;

/// Where reports go (the launcher's BuildReports.Address).
pub const ADDRESS: &str = "https://data-services.magicbean.dev/telemetry/build";
/// Who sent it.
pub const CLIENT: &str = "longshot-builder";
/// The user agent it is posted with.
pub const AGENT: &str = "longshot-builder";
/// The log's lines a report carries.
pub const LOG_LINES: usize = 200;
/// The most a report may be, in UTF-8 bytes (the server answers a bigger one 413).
pub const MAX_BYTES: usize = 128 * 1024;

/// How a build ended.
pub const COMPLETED: &str = "completed";
/// make stopped on an error.
pub const FAILED: &str = "failed";
/// The player cancelled it.
pub const CANCELLED: &str = "cancelled";
/// A signal stopped it.
pub const TERMINATED: &str = "terminated";
/// What the build was.
pub const FIRST_BUILD: &str = "first build";
/// An update to a newer version.
pub const UPDATE: &str = "update";
/// An update back to an older one.
pub const GO_BACK: &str = "go back";
/// The same version built again.
pub const REBUILD: &str = "rebuild";

const VERSION_LEN: usize = 40;
const KIND_LEN: usize = 20;
const SYSTEM_LEN: usize = 200;
const CPU_LEN: usize = 120;
const PC_LEN: usize = 300;
const SETTINGS_LEN: usize = 400;
const TEXT_LEN: usize = 300;
const LOG_LINE_LEN: usize = 300;

/// What a report says of a build.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Build {
    /// The builder's version (the launcher's field: the program that built).
    pub launcher: String,
    /// The game's version built.
    pub version: String,
    /// Its channel.
    pub channel: String,
    /// [`FIRST_BUILD`], [`UPDATE`], [`GO_BACK`] or [`REBUILD`].
    pub kind: String,
    /// The OS.
    pub system: String,
    /// This PC's line (memory, threads, graphics cards).
    pub pc: String,
    /// The build's settings.
    pub settings: String,
    /// Why it stopped.
    pub why: String,
    /// Its last error line.
    pub error: Option<String>,
    /// The log's lines.
    pub log: Vec<String>,
    /// [`COMPLETED`], [`FAILED`], [`CANCELLED`] or [`TERMINATED`].
    pub outcome: String,
    /// The CPU.
    pub cpu: String,
    /// Each stage: its id, progress (0..1), seconds, and whether it kept an earlier build's work.
    pub stages: Vec<(String, f64, f64, bool)>,
    /// Sent on its own (statistics: no log).
    pub auto: bool,
}

/// `text` cut to at most `max` UTF-16 code units, on a whole character (the server's limits are .NET's).
pub fn cut(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &text[..i];
        }
    }
    text
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Where a folder's name ends at `rest` (Privacy.FolderEnd): a separator, a line's or the text's end, a character a
/// path ends at, a sentence's full stop, or a blank whose next word is no path.
fn folder_end(rest: &str) -> bool {
    let mut it = rest.chars();
    match it.next() {
        None => true,
        Some(c) if sep(c) || c == '\r' || c == '\n' || "\"'`<>|)]},;:?*".contains(c) => true,
        Some('.') => it.next().is_none_or(char::is_whitespace),
        Some(' ' | '\t') => {
            let next: String = it.take_while(|c| !c.is_whitespace() && !sep(*c) && !"\"'<>|".contains(*c)).collect();
            let after = rest[1 + next.len()..].chars().next();
            !after.is_some_and(sep)
        }
        _ => false,
    }
}

/// `pattern` at `text[at..]`: its parts (a home folder's names, or one name) with `seps` between them, a space also as
/// `%20`, in any case when `fold`. The length matched.
fn match_parts(text: &str, at: usize, parts: &[&str], seps: bool, fold: bool) -> Option<usize> {
    let mut i = at;
    for (k, part) in parts.iter().enumerate() {
        if k > 0 {
            let s = &text[i..];
            let n =
                s.chars().take_while(|c| if seps { sep(*c) } else { *c == '/' }).take(if seps { 2 } else { 1 }).count();
            if n == 0 {
                return None;
            }
            i += n;
        }
        for pc in part.chars() {
            let rest = &text[i..];
            if pc == ' ' && rest.starts_with("%20") {
                i += 3;
                continue;
            }
            let c = rest.chars().next()?;
            let same = if fold { c.to_lowercase().eq(pc.to_lowercase()) } else { c == pc };
            if !same {
                return None;
            }
            i += c.len_utf8();
        }
    }
    Some(i - at)
}

/// The 8.3 short form's first 6 characters of a long name (John Smith: JOHNSM), None for a name that is its own.
fn short_name(name: &str) -> Option<String> {
    let up = name.to_uppercase();
    let s: String = up
        .chars()
        .filter(|c| *c != ' ' && *c != '.')
        .map(|c| if (c as u32) < 128 && (c.is_alphanumeric() || "!#$%&'()-@^_`{}~".contains(c)) { c } else { '_' })
        .collect();
    let fits = name.chars().count() <= 8 && s == up;
    (!fits && !s.is_empty()).then(|| s.chars().take(6).collect())
}

/// `text` with no user name in its paths (Privacy.Scrub): the home folder as `%USERPROFILE%` (Windows) or `~` wherever a
/// path starts with it; then each of `users` and the home's last name (2 characters or more) wherever it is a whole
/// folder's name after a separator, as `<user>`; on Windows in any case and its 8.3 short form too.
pub fn scrub(text: &str, home: &str, users: &[&str]) -> String {
    let h = home.trim_end_matches(['/', '\\']);
    let windows = h.contains('\\') || h.as_bytes().get(1) == Some(&b':') || (h.is_empty() && cfg!(windows));
    let mut t = text.to_string();
    if h.chars().count() > 3 {
        let parts: Vec<&str> = if windows { h.split(['\\', '/']).collect() } else { h.split('/').collect() };
        let mut out = String::with_capacity(t.len());
        let mut i = 0;
        while i < t.len() {
            let before_ok = windows || t[..i].chars().next_back().is_none_or(|c| !(word(c) || c == '.' || c == '-'));
            if before_ok
                && let Some(n) = match_parts(&t, i, &parts, windows, windows)
                && folder_end(&t[i + n..])
            {
                out.push_str(if windows { "%USERPROFILE%" } else { "~" });
                i += n;
                continue;
            }
            let c = t[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
        t = out;
    }
    let last = if h.chars().count() > 3 { h.rsplit(['\\', '/']).next().unwrap_or("") } else { "" };
    let mut names: Vec<&str> = users.iter().copied().chain([last]).filter(|n| n.trim().chars().count() >= 2).collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
    names.dedup_by(|a, b| if windows { a.eq_ignore_ascii_case(b) } else { a == b });
    for n in names {
        let short = if windows { short_name(n) } else { None };
        let mut out = String::with_capacity(t.len());
        let mut i = 0;
        while i < t.len() {
            if t[..i].chars().next_back().is_some_and(sep) {
                let long = match_parts(&t, i, &[n], false, windows).filter(|&m| folder_end(&t[i + m..]));
                let short = short.as_ref().and_then(|s| {
                    let m = match_parts(&t, i, &[s.as_str()], false, true)?;
                    let tail = &t[i + m..];
                    let d = tail.strip_prefix('~')?.bytes().take_while(u8::is_ascii_digit).count().min(6);
                    (d > 0 && folder_end(&tail[1 + d..])).then_some(m + 1 + d)
                });
                if let Some(m) = long.or(short) {
                    out.push_str("<user>");
                    i += m;
                    continue;
                }
            }
            let c = t[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
        t = out;
    }
    t
}

/// The names a game file or one made from it ends in.
const GAME_EXTENSIONS: [&str; 23] = [
    "nup", "nx", "nsp", "nsz", "xci", "xcz", "nca", "nso", "keys", "romfs", "exefs", "bfres", "sbfres", "byml", "bntx",
    "pack", "bars", "sarc", "msbt", "bfstm", "bfsar", "zs", "dat",
];

fn ends_game(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| GAME_EXTENSIONS.iter().any(|g| g.eq_ignore_ascii_case(e)))
}

/// A link as a diagnostic keeps it: its scheme, host and path (no user, password, query or fragment).
fn link(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].rsplit('@').next().unwrap_or("");
    let path = &rest[end..];
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    format!("{}://{host}{}", scheme.to_ascii_lowercase(), if path.is_empty() { "/" } else { path })
}

/// A diagnostic as a summary sent on its own carries it (the player saw nothing before it went): every game file's
/// name as `[game file]` (a quoted one whole, spaces and all), a link's scheme, host and path only; cut to the limit.
pub fn diagnostic(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    let t = cut(t, 4 * TEXT_LEN);
    // (quoted texts whose last name is a game file's)
    let mut q = String::new();
    let mut i = 0;
    while i < t.len() {
        let c = t[i..].chars().next().unwrap_or(' ');
        if (c == '\'' || c == '"') && !t[..i].chars().next_back().is_some_and(word) {
            let body_at = i + 1;
            if let Some(len) = t[body_at..].find(['\'', '"', '\r', '\n'])
                && len > 0
                && t[body_at + len..].starts_with(c)
                && !t[body_at + len + 1..].chars().next().is_some_and(word)
            {
                let body = &t[body_at..body_at + len];
                let name = body.rfind(['\\', '/']).map_or(0, |p| p + 1);
                if ends_game(&body[name..]) {
                    let _ = write!(q, "{c}{}[game file]{c}", &body[..name]);
                } else {
                    let _ = write!(q, "{c}{body}{c}");
                }
                i = body_at + len + 1;
                continue;
            }
        }
        q.push(c);
        i += c.len_utf8();
    }
    // (links)
    let mut l = String::new();
    let mut i = 0;
    while i < q.len() {
        let rest = &q[i..];
        let at_word = !q[..i].chars().next_back().is_some_and(word);
        let lower = rest.get(..8).unwrap_or(rest).to_ascii_lowercase();
        if at_word && (lower.starts_with("http://") || lower.starts_with("https://")) {
            let end = rest.find(|c: char| c.is_whitespace() || "'\"<>".contains(c)).unwrap_or(rest.len());
            l.push_str(&link(&rest[..end]));
            i += end;
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        l.push(c);
        i += c.len_utf8();
    }
    // (a game file's name, a word of its own, not a call)
    let run = |c: char| !c.is_whitespace() && !"'\"<>|\\/".contains(c);
    let mut out = String::new();
    let mut i = 0;
    'scan: while i < l.len() {
        let prev_ok = !l[..i].chars().next_back().is_some_and(|c| word(c) || c == '.' || c == '-');
        if prev_ok {
            let len = l[i..].find(|c: char| !run(c)).unwrap_or(l.len() - i);
            let token = &l[i..i + len];
            // (the longest end within the run: ".ext" then a word's end, not "(")
            let mut ends: Vec<usize> = token.char_indices().map(|(k, c)| k + c.len_utf8()).collect();
            ends.reverse();
            for e in ends {
                let head = &token[..e];
                let after = l[i + e..].chars().next();
                if ends_game(head)
                    && head.rsplit_once('.').is_some_and(|(_, x)| !x.is_empty())
                    && !after.is_some_and(word)
                    && after != Some('(')
                {
                    out.push_str("[game file]");
                    i += e;
                    continue 'scan;
                }
            }
        }
        let c = l[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    cut(&out, TEXT_LEN).to_string()
}

fn number(v: f64) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

impl Build {
    /// The report as it is sent, and as the page shows it: JSON with no user name ([`scrub`] with `home` and `users`),
    /// each text cut to the server's limit, at most [`MAX_BYTES`] (the log's oldest lines left out until it fits).
    pub fn json(&self, home: &str, users: &[&str]) -> String {
        let clean = |t: &str| scrub(t, home, users);
        let kept = |t: &str, max: usize| cut(&clean(t), max).to_string();
        let failing = self.outcome == FAILED || self.outcome == TERMINATED;
        let reason = |t: &str| {
            if !self.auto {
                kept(t, TEXT_LEN)
            } else if failing {
                diagnostic(&clean(t))
            } else {
                String::new()
            }
        };
        let why = reason(&self.why);
        let error = reason(self.error.as_deref().unwrap_or(""));
        let error = if self.auto && error == why { String::new() } else { error };
        let log: Vec<String> = if self.auto {
            Vec::new()
        } else {
            self.log[self.log.len().saturating_sub(LOG_LINES)..].iter().map(|l| kept(l, LOG_LINE_LEN)).collect()
        };
        let written = |lines: &[String]| {
            let mut s = String::from("{\n");
            let _ = writeln!(s, "  \"v\": 1,\n  \"client\": {},", json(CLIENT));
            for (k, v, max) in [
                ("launcher", &self.launcher, VERSION_LEN),
                ("version", &self.version, VERSION_LEN),
                ("channel", &self.channel, VERSION_LEN),
                ("kind", &self.kind, KIND_LEN),
            ] {
                let _ = writeln!(s, "  \"{k}\": {},", json(&kept(v, max)));
            }
            let _ = writeln!(s, "  \"outcome\": {},\n  \"auto\": {},", json(&self.outcome), self.auto);
            for (k, v, max) in [
                ("os", &self.system, SYSTEM_LEN),
                ("cpu", &self.cpu, CPU_LEN),
                ("pc", &self.pc, PC_LEN),
                ("settings", &self.settings, SETTINGS_LEN),
            ] {
                let _ = writeln!(s, "  \"{k}\": {},", json(&kept(v, max)));
            }
            let _ = writeln!(s, "  \"why\": {},\n  \"error\": {},", json(&why), json(&error));
            let stages: Vec<String> = self
                .stages
                .iter()
                .map(|(id, p, sec, kept)| {
                    format!(
                        "    {}: {{\"progress\": {}, \"seconds\": {}, \"kept\": {kept}}}",
                        json(id),
                        number(*p),
                        number(*sec)
                    )
                })
                .collect();
            if stages.is_empty() {
                s.push_str("  \"stages\": {},\n");
            } else {
                let _ = writeln!(s, "  \"stages\": {{\n{}\n  }},", stages.join(",\n"));
            }
            if lines.is_empty() {
                s.push_str("  \"log\": []\n}");
            } else {
                let items: Vec<String> = lines.iter().map(|l| format!("    {}", json(l))).collect();
                let _ = write!(s, "  \"log\": [\n{}\n  ]\n}}", items.join(",\n"));
            }
            clean(&s)
        };
        let mut out = written(&log);
        let mut skip = 10;
        while out.len() > MAX_BYTES && skip - 10 < log.len() {
            out = written(&log[skip.min(log.len())..]);
            skip += 10;
        }
        out
    }
}

/// Posts a build report: sent, what the page says, whether trying again could help ([`crate::names::answer`]'s
/// words, the build's server).
pub fn send(json_text: &str, payload: &std::path::Path, address: &str) -> (bool, String, bool) {
    if std::fs::write(payload, json_text).is_err() {
        return (false, "Not sent: the report could not be saved for sending.".into(), false);
    }
    let (status, body) = crate::post::post(payload, address, AGENT);
    let _ = std::fs::remove_file(payload);
    let (sent, text, again) = crate::names::answer(status, &body);
    (sent, text.replace("the crash server", "the report server"), again)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_takes_the_users_name_out_of_paths() {
        assert_eq!(scrub("/home/ana/x and /home/ana", "/home/ana", &[]), "~/x and ~");
        assert_eq!(scrub("/home/ana2/x", "/home/ana", &[]), "/home/ana2/x");
        assert_eq!(scrub("/run/media/ana/Disk", "/home/ana", &[]), "/run/media/<user>/Disk");
        assert_eq!(scrub("Ana went home", "/home/ana", &["Ana"]), "Ana went home", "not a path");
        assert_eq!(scrub("/x/ana.old/y", "/home/ana", &[]), "/x/ana.old/y");
        assert_eq!(
            scrub(r"C:\Users\Ana Smith\x c:/users/ana smith/y", r"C:\Users\Ana Smith", &[]),
            r"%USERPROFILE%\x %USERPROFILE%/y"
        );
        assert_eq!(
            scrub(r"C:\\Users\\Ana Smith\\x", r"C:\Users\Ana Smith", &[]),
            r"%USERPROFILE%\\x",
            "as JSON writes it"
        );
        assert_eq!(
            scrub(r"D:\Users\ANA SMITH\x E:\b\ANASMI~1\y", r"C:\Users\Ana Smith", &[]),
            r"D:\Users\<user>\x E:\b\<user>\y"
        );
        assert_eq!(scrub("file:///C:/Users/Ana%20Smith/x", r"C:\Users\Ana Smith", &[]), "file:///%USERPROFILE%/x");
        assert_eq!(scrub(r"C:\Users\Ana Smith2\x", r"C:\Users\Ana Smith", &[]), r"C:\Users\Ana Smith2\x");
        assert_eq!(scrub("/home/ana is mine. /home/ana.", "/home/ana", &[]), "~ is mine. ~.");
        assert_eq!(scrub("~/x <user>", "/home/ana", &[]), "~/x <user>", "scrubbed already: as it is");
    }

    #[test]
    fn a_diagnostic_loses_game_files_and_link_secrets() {
        assert_eq!(
            diagnostic("could not read 'C:\\Games\\The Game v2.nup': bad"),
            "could not read 'C:\\Games\\[game file]': bad"
        );
        assert_eq!(diagnostic("Main.bfstm missing in [0100ABCD][v0].nx"), "[game file] missing in [game file]");
        assert_eq!(diagnostic("d.keys() failed, struct.pack(x)"), "d.keys() failed, struct.pack(x)", "calls stay");
        assert_eq!(diagnostic("GET https://u:p@x.example/a/b?token=1#f failed"), "GET https://x.example/a/b failed");
        assert_eq!(diagnostic("  "), "");
        assert_eq!(diagnostic(&"x".repeat(2000)).len(), 300);
    }

    #[test]
    fn the_report_as_sent() {
        let b = Build {
            launcher: "0.1.0".into(),
            version: "v3.1.0".into(),
            kind: FIRST_BUILD.into(),
            outcome: FAILED.into(),
            why: "lift failed in /home/ana/g/x.nup".into(),
            error: Some("error: /home/ana/y".into()),
            log: (0..300).map(|i| format!("line {i} /home/ana/z")).collect(),
            stages: vec![("lift".into(), 0.5, 12.0, false)],
            ..Build::default()
        };
        let j = b.json("/home/ana", &[]);
        assert!(
            j.contains("\"client\": \"longshot-builder\"")
                && j.contains("\"outcome\": \"failed\"")
                && j.contains("\"auto\": false")
        );
        assert!(!j.contains("ana") && j.contains("~/g/x.nup"), "{j}");
        assert!(j.contains("line 100 ") && !j.contains("line 99 "), "the last 200 lines");
        assert!(j.contains("\"lift\": {\"progress\": 0.5, \"seconds\": 12, \"kept\": false}"));
        let a = Build { auto: true, ..b.clone() }.json("/home/ana", &[]);
        assert!(a.contains("\"log\": []") && a.contains("[game file]") && a.contains("\"auto\": true"), "{a}");
        let done = Build { auto: true, outcome: COMPLETED.into(), ..b }.json("/home/ana", &[]);
        assert!(
            done.contains("\"why\": \"\"") && done.contains("\"error\": \"\""),
            "a completed build says no why on its own"
        );
        assert!(cut("é😀x", 2) == "é" && cut("é😀x", 3) == "é😀");
    }
}
