//! What goes into a report, and how: every field and line cleaned ([`clean`]: no control characters, the device names,
//! the home folder and the user out, a length limit), the crash's lines as the relay's signature reads them
//! ([`report_line`]), the settings it holds ([`setting`], [`value_shown`]). Ported from host/hos_crash_dialog.h.

use crate::redact;

/// A crash's first line: "CRASH: exception 0x<hex> " anywhere in it.
pub fn starts(line: &str) -> bool {
    line.match_indices("CRASH: exception 0x").any(|(at, m)| {
        let rest = &line.as_bytes()[at + m.len()..];
        let n = rest.iter().take_while(|c| c.is_ascii_hexdigit()).count();
        n > 0 && rest.get(n) == Some(&b' ')
    })
}

/// A row of the game's memory: the registers ("CRASH:   x0 ..."), and what they point to ("CRASH: [x19+0x10] ...").
pub fn memory_row(crash: &str) -> bool {
    crash.match_indices("CRASH:").any(|(at, _)| {
        let p = &crash.as_bytes()[at + 6..];
        if p.len() > 3 && p[0] == b' ' && p[1] == b'[' && p[2] == b'x' && p[3].is_ascii_digit() {
            let n = p[3..].iter().take_while(|c| c.is_ascii_digit()).count();
            if p.get(3 + n) == Some(&b'+') {
                return true;
            }
        }
        if p.first() != Some(&b' ') {
            return false;
        }
        let k = p.iter().take_while(|&&c| c == b' ').count();
        p.get(k) == Some(&b'x') && p.get(k + 1).is_some_and(u8::is_ascii_digit)
    })
}

/// A log line's crash part ("CRASH: ..." at its start or after the log's "] "), as the report holds it: modules
/// without their folder ("at /usr/lib/libc.so.6+0x10" is "at libc.so.6+0x10") and, on the crash's first line, the
/// place's module in the relay's characters ("at ?+0x10" is "at unknown+0x10"). None for a line with none, or a row of
/// the game's memory.
pub fn report_line(line: &str) -> Option<String> {
    let c = if line.starts_with("CRASH: ") { line } else { &line[line.find("] CRASH: ")? + 2..] };
    if memory_row(c) {
        return None;
    }
    let first = starts(c);
    let mut module_done = false;
    let b = c.as_bytes();
    let stop = b.iter().position(|&x| x == b'\n' || x == b'\r').unwrap_or(b.len());
    let mut out: Vec<u8> = Vec::with_capacity(stop);
    let mut p = 0;
    while p < stop {
        if p >= 4 && &b[p - 4..p] == b" at " {
            // (p is a place: its token up to a space; the folders off it when a module+0x follows the last separator)
            let end = p + b[p..stop].iter().position(|&x| x == b' ' || x == b'\t').unwrap_or(stop - p);
            let mut name = b[p..end].iter().rposition(|&q| redact::sep(q)).map_or(p, |q| p + q + 1);
            if name != p {
                let mut plus = name + 1;
                while plus + 2 < end && &b[plus..plus + 3] != b"+0x" {
                    plus += 1;
                }
                if !(name < end && plus + 2 < end && &b[plus..plus + 3] == b"+0x") {
                    name = p;
                }
            }
            if first && !module_done {
                // (the place's module: [^\s+]* before +0x<hex>, then a space or the end)
                let plus = name + b[name..end].iter().position(|&x| x == b'+').unwrap_or(end - name);
                let mut hex = plus + 3;
                let ok = plus + 3 <= end && &b[plus..plus + 3] == b"+0x" && hex < end && b[hex].is_ascii_hexdigit();
                while ok && hex < end && b[hex].is_ascii_hexdigit() {
                    hex += 1;
                }
                if ok && hex == end {
                    module_done = true;
                    let kept: Vec<u8> = b[name..plus]
                        .iter()
                        .copied()
                        .filter(|&q| q.is_ascii_alphanumeric() || q == b'_' || q == b'.' || q == b'-')
                        .take(64)
                        .collect();
                    if kept.is_empty() {
                        out.extend_from_slice(b"unknown");
                    } else {
                        out.extend_from_slice(&kept);
                    }
                    name = plus;
                }
            }
            out.extend_from_slice(&b[name..end]);
            p = end;
            continue;
        }
        out.push(b[p]);
        p += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// `<device>` where the text names a device a player named (host/nn_audio.c's output, host/nn_audio_in.c's microphone).
fn devices_out(s: &str) -> String {
    let mut s = s.to_string();
    for (k, head) in ["audio: output on ", "audio in: "].iter().enumerate() {
        let Some(at) = s.find(head) else { continue };
        let name = at + head.len();
        let rest = &s[name..];
        let end = if k == 0 {
            // ("audio: output on <name> (3 devices)")
            rest.match_indices(" (").map(|(i, _)| i).find(|&i| {
                let d = &rest.as_bytes()[i + 2..];
                let n = d.iter().take_while(|c| c.is_ascii_digit()).count();
                n > 0 && d[n..].starts_with(b" device")
            })
        } else {
            // ("<name> open (48000 Hz", "<name> is back", "<name>: <why>, tried again from time to time")
            if let Some(open) = rest.find(" open (") {
                Some(open)
            } else if rest.find(" is back").is_some_and(|b| b + 8 == rest.len()) {
                rest.find(" is back")
            } else if let Some(again) = rest.find(", tried again from time to time") {
                rest.find(": ").filter(|&e| e <= again)
            } else {
                None
            }
        };
        if let Some(end) = end.filter(|&e| e > 0) {
            s.replace_range(name..name + end, "<device>");
        }
    }
    s
}

/// Words the report's own lines use, never taken for the user's name in any case (a Steam Deck's `deck`).
const COMMON: [&str; 25] = [
    "deck", "steam", "arch", "linux", "ubuntu", "fedora", "debian", "mint", "manjaro", "bazzite", "nobara", "cachyos",
    "windows", "wine", "vulkan", "intel", "amd", "nvidia", "radeon", "game", "games", "gamer", "user", "admin",
    "player",
];

/// Every place a home folder is by its shape, after the user's own: /home/<name>, /var/home/<name> and /Users/<name> as
/// `~`, C:\Users\<name> as `%USERPROFILE%`, a drive's \home\<name> (Wine's Z:\home\<name>) as `~`; and the user's
/// name, 3 letters or more, as a word of its own anywhere, in any case unless it is one of [`COMMON`].
fn homes_out(s: &str, user: &str) -> String {
    let b = s.as_bytes();
    let u = user.as_bytes();
    let any_case = u.len() >= 3 && !COMMON.iter().any(|c| c.eq_ignore_ascii_case(user));
    let word = |c: u8| c.is_ascii_alphanumeric();
    let ncase =
        |at: usize, what: &[u8]| at + what.len() <= b.len() && b[at..at + what.len()].eq_ignore_ascii_case(what);
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let n = b.len();
    let mut i = 0;
    'each: while i < n {
        let before = i == 0 || !word(b[i - 1]);
        // (a drive: a letter on its own, its colon, separators)
        if before && i + 2 < n && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && redact::sep(b[i + 2]) {
            let mut j = i + 2;
            while j < n && redact::sep(b[j]) {
                j += 1;
            }
            let mut k = j;
            if k + 3 < n && ncase(k, b"var") && redact::sep(b[k + 3]) {
                k += 3;
                while k < n && redact::sep(b[k]) {
                    k += 1;
                }
            }
            let (mut home, mut users) = (false, false);
            if k + 4 < n && ncase(k, b"home") && redact::sep(b[k + 4]) {
                home = true;
                k += 4;
            } else if k == j && k + 5 < n && ncase(k, b"users") && redact::sep(b[k + 5]) {
                users = true;
                k += 5;
            }
            if home || users {
                while k < n && redact::sep(b[k]) {
                    k += 1;
                }
                let mut e = k;
                while e < n && !redact::sep(b[e]) && !b[e].is_ascii_whitespace() {
                    e += 1;
                }
                if e > k {
                    out.extend_from_slice(if home { b"~" } else { b"%USERPROFILE%" });
                    i = e;
                    continue;
                }
            }
        }
        for h in [&b"/var/home/"[..], b"/home/", b"/Users/"] {
            if i + h.len() < n
                && &b[i..i + h.len()] == h
                && !redact::sep(b[i + h.len()])
                && !b[i + h.len()].is_ascii_whitespace()
            {
                let mut e = i + h.len();
                while e < n && b[e] != b'/' && !b[e].is_ascii_whitespace() {
                    e += 1;
                }
                out.push(b'~');
                i = e;
                continue 'each;
            }
        }
        if u.len() >= 3
            && before
            && i + u.len() <= n
            && (if any_case { b[i..i + u.len()].eq_ignore_ascii_case(u) } else { &b[i..i + u.len()] == u })
            && (i + u.len() == n || !word(b[i + u.len()]))
        {
            out.extend_from_slice(b"<user>");
            i += u.len();
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A field or line as the report holds it: no control characters, the device names out (the log's own and each of
/// `devices`, 3 characters or more), the home folder and user out ([`redact`], then by shape), the spaces around it
/// off, at most `limit` characters.
pub fn clean(s: &str, limit: usize, who: &redact::Who, devices: &[&str]) -> String {
    let mut s: String = s.chars().filter(|&c| !((c < ' ' && c != '\t' && c != '\n') || c == '\x7f')).collect();
    s = devices_out(&s);
    // (each name found from after the last one written: a name inside <device> itself is never found again)
    for d in devices.iter().filter(|d| d.len() >= 3) {
        let mut from = 0;
        while let Some(at) = s[from..].find(d).map(|i| i + from) {
            if in_device(&s, at) {
                from = at + 1;
                while !s.is_char_boundary(from) {
                    from += 1;
                }
            } else {
                s.replace_range(at..at + d.len(), "<device>");
                from = at + "<device>".len();
            }
        }
    }
    s = who.redact(&s);
    s = homes_out(&s, &who.user);
    s.trim().chars().take(limit).collect()
}

/// Whether `s[at]` is inside a `<device>` already written there.
fn in_device(s: &str, at: usize) -> bool {
    (0..8.min(at + 1)).any(|back| s.as_bytes()[at - back..].starts_with(b"<device>"))
}

/// `s` as a JSON string, quotes included.
pub fn json(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\x7f' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The version as the relay takes it: what is not `[0-9A-Za-z.+_-]` as `_`, at most 40; "unknown" for none.
pub fn report_version(version: &str) -> String {
    let v: String = version
        .bytes()
        .take(40)
        .map(|c| if c.is_ascii_alphanumeric() || b".+_-".contains(&c) { c as char } else { '_' })
        .collect();
    if v.is_empty() { "unknown".into() } else { v }
}

/// A setting the reports hold: key=value, the value a number, a size (1920x1080) or a pair (16,9), never the
/// bindings, the microphone's name or the hardware hashes. Its key and value.
pub fn setting(line: &str) -> Option<(String, String)> {
    let line = line.trim_start_matches([' ', '\t']);
    let eq = line.find('=').filter(|&e| e > 0)?;
    let key = &line[..eq];
    if !key.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_') {
        return None;
    }
    let rest = &line[eq + 1..];
    let mut v = &rest[..rest.find(['\r', '\n']).unwrap_or(rest.len())];
    // (the C trims one trailing blank short of the value's first character; a blank before the end is no number anyway)
    while v.ends_with([' ', '\t']) {
        v = &v[..v.len() - 1];
    }
    let b = v.as_bytes();
    let mut p = usize::from(b.first() == Some(&b'-'));
    let mut ok = digits(b, &mut p);
    if ok && p == b.len() {
    } else if ok && b[0] != b'-' && (b[p] == b'x' || b[p] == b',') {
        p += 1;
        ok = digits(b, &mut p) && p == b.len();
    } else {
        ok = false;
    }
    if !ok
        || key.starts_with("key_")
        || key.starts_with("pad_")
        || ["microphone_device", "auto_for", "auto_tuned"].contains(&key)
    {
        return None;
    }
    if key.len() + 1 > 64 || v.len() + 1 > 32 {
        return None;
    }
    Some((key.to_string(), v.to_string()))
}

fn digits(s: &[u8], p: &mut usize) -> bool {
    let from = *p;
    while *p < s.len() && s[*p].is_ascii_digit() {
        *p += 1;
    }
    *p > from
}

/// Whether a setting's value goes into a report: a number of at most 12 characters (60, -1, 1.5), a size (1920x1080),
/// on/off, true/false, yes/no, or empty; and, for a switch (`name`), never one that may hold a secret, a place or a
/// person (TOKEN, KEY, ROOM, SECRET, PASS, SERVER or USER in its name).
pub fn value_shown(name: Option<&str>, value: &str) -> bool {
    if let Some(name) = name {
        let upper = name.to_ascii_uppercase();
        if ["TOKEN", "KEY", "ROOM", "SECRET", "PASS", "SERVER", "USER"].iter().any(|w| upper.contains(w)) {
            return false;
        }
    }
    if value.len() <= 5 && ["", "on", "off", "true", "false", "yes", "no"].iter().any(|p| p.eq_ignore_ascii_case(value))
    {
        return true;
    }
    if value.len() > 12 {
        return false;
    }
    let b = value.as_bytes();
    let mut at = usize::from(b.first() == Some(&b'-'));
    if !digits(b, &mut at) {
        return false;
    }
    if at == b.len() {
        return true;
    }
    let mid = b[at];
    at += 1;
    if mid != b'.' && !(mid == b'x' && b[0] != b'-') {
        return false;
    }
    digits(b, &mut at) && at == b.len()
}
