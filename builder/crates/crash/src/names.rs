//! The small decisions around the records and the report: the records' names and times, which are shown at a start,
//! where reports go and whether they are offered, the server's answer, the issue form's link, the box after Report.
//! Ported from host/hos_crash_dialog.h.

/// The game-problem form, as the Insert menu's Bug report links it (gpu/vk/vk_bug_report.cpp kForm).
pub const ISSUE_FORM: &str =
    "https://git.magicbean.dev/MagicBeanDev/Longshot/issues/new?template=.forgejo/ISSUE_TEMPLATE/1-game-problem.yml";
/// The issue chooser.
pub const ISSUES: &str = "https://git.magicbean.dev/MagicBeanDev/Longshot/issues/new/choose";
/// The longest link the sign-in page's redirect keeps.
pub const LINK_MAX: usize = 2000;

/// A record's kind by its name, crash-<UTC>.txt or unclean-<UTC>.txt (maybe -<pid>-<n> before .txt): its kind
/// (1 crash, 2 unclean) and its UTC "YYYYMMDDTHHMMSSZ"; None for any other name.
pub fn record_name(name: &str) -> Option<(u8, &str)> {
    let (kind, p) =
        if let Some(p) = name.strip_prefix("crash-") { (1, p) } else { (2, name.strip_prefix("unclean-")?) };
    let b = p.as_bytes();
    if b.len() < 16 {
        return None;
    }
    for (i, &c) in b[..16].iter().enumerate() {
        let ok = match i {
            8 => c == b'T',
            15 => c == b'Z',
            _ => c.is_ascii_digit(),
        };
        if !ok {
            return None;
        }
    }
    let mut s = &p[16..];
    if let Some(rest) = s.strip_prefix('-') {
        let a = rest.bytes().take_while(u8::is_ascii_digit).count();
        let rest = rest[a..].strip_prefix('-').filter(|_| a > 0)?;
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        s = &rest[n..];
    }
    (s == ".txt").then(|| (kind, &p[..16]))
}

/// Seconds since 1970 of a record's UTC ("YYYYMMDDTHHMMSSZ").
pub fn epoch(when: &str) -> i64 {
    let n = |a: usize, k: usize| when.get(a..a + k).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    let (y, m, d) = (n(0, 4), n(4, 2), n(6, 2));
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400 + n(9, 2) * 3600 + n(11, 2) * 60 + n(13, 2)
}

/// A UTC "YYYYMMDDTHHMMSSZ" of seconds since 1970.
pub fn utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // (H. Hinnant's civil_from_days)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Whether `list` (bug-reports/shown.txt: a name a line) has `name`.
pub fn listed(list: &str, name: &str) -> bool {
    list.split('\n').any(|l| l.trim_end_matches('\r') == name)
}

/// Whether a start shows a record nobody showed (no builder watched its run, or no window could open): a crash
/// record, or an unclean end the out-of-memory killer made (`oom`); a day old at most, and not listed as shown.
pub fn show_at_start(name: &str, shown: &str, now: i64, oom: bool) -> bool {
    let Some((kind, when)) = record_name(name) else { return false };
    if !(kind == 1 || (kind == 2 && oom)) || listed(shown, name) {
        return false;
    }
    let t = epoch(when);
    t <= now + 300 && now - t <= 86_400
}

/// A key's https:// address in longshot_version.txt (`crash_reports=`, which longshot-builder writes from
/// assets/addresses.ini); None when the stamp has none, or it is not one https:// address on one line.
pub fn stamp_address(stamp: &str, key: &str) -> Option<String> {
    let line = stamp.split('\n').find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))?;
    let v = line.trim_end_matches('\r');
    let v = &v[..v.find('\r').unwrap_or(v.len())];
    (v.len() >= 9 && v.starts_with("https://") && v.len() < 512 && !v.contains(' ')).then(|| v.to_string())
}

/// Whether Report automatically is offered: a crash record, an address to send it to, and not sent already.
pub fn send_offered(record_name: Option<&str>, have_address: bool, sent_already: bool) -> bool {
    record_name.is_some_and(|n| self::record_name(n).is_some_and(|(k, _)| k == 1)) && have_address && !sent_already
}

/// Whether the player chose to send crash reports with no window: a line `send=1` in longshot_crash.ini (blanks
/// around it allowed).
pub fn auto_send(ini: &str) -> bool {
    ini.split('\n').any(|l| {
        l.trim_start_matches([' ', '\t'])
            .strip_prefix("send=1")
            .is_some_and(|r| r.trim_matches([' ', '\t', '\r']).is_empty())
    })
}

/// The answer to a send, from the HTTP status (0: no answer) and the relay's JSON (`{"error": why}` on a refusal): sent,
/// what the page says, and whether trying again could help.
pub fn answer(status: u16, body: &str) -> (bool, String, bool) {
    match status {
        200..=299 => (true, "Sent. Thank you: the Longshot team has it. Only the team can read it.".into(), false),
        429 => (false, "Not sent: the crash server is busy. Nothing was sent: try again in a minute.".into(), true),
        400..=499 => {
            let said = json_field(body, "error").unwrap_or_default();
            let said = crate::facts::cut(&said, 299);
            (
                false,
                format!(
                    "Not sent: the crash server refused the report ({status}{}{said}).",
                    if said.is_empty() { "" } else { ": " }
                ),
                false,
            )
        }
        0 => (
            false,
            "Not sent: couldn't reach the crash server (no internet, or curl is missing). Nothing was sent.".into(),
            true,
        ),
        _ => (false, format!("Not sent: the crash server answered {status}. Nothing was sent: try again later."), true),
    }
}

/// A string field of a JSON text, read plainly (the relay's `{"error": "..."}`).
fn json_field(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start_matches([' ', ':']);
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.extend(it.next()),
            c => out.push(c),
        }
    }
    None
}

/// The status of the last answer in curl's `--dump-header` file (a redirect's answers come first): 0 without one.
pub fn http_status(headers: &str) -> u16 {
    headers
        .split('\n')
        .rev()
        .filter(|l| l.starts_with("HTTP/"))
        .find_map(|l| l.split(' ').nth(1).and_then(|s| s.trim().parse().ok()))
        .unwrap_or(0)
}

/// `s` as a link's part: what is not plain escaped.
pub fn url_part(s: &str) -> String {
    let mut o = String::new();
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) {
            o.push(c as char);
        } else {
            o.push_str(&format!("%{c:02X}"));
        }
    }
    o
}

/// What the issue form is filled with.
#[derive(Clone, Debug, Default)]
pub struct IssueFields<'a> {
    /// The issue's title.
    pub title: &'a str,
    /// Longshot's version.
    pub version: &'a str,
    /// The graphics card.
    pub gpu: &'a str,
    /// The system.
    pub system: &'a str,
    /// What happened.
    pub what: &'a str,
    /// The channel.
    pub track: &'a str,
    /// Where in the game.
    pub where_: &'a str,
    /// The settings changed.
    pub settings: &'a str,
}

/// The game-problem form with its fields filled in, each only while the whole link stays within [`LINK_MAX`]. Never
/// the report itself: the player pastes that.
pub fn issue_url(f: &IssueFields) -> String {
    let parts = [
        ("&field:version=", f.version),
        ("&field:track=", f.track),
        ("&field:kind=", "Crash, freeze or black screen"),
        ("&field:gpu=", f.gpu),
        ("&field:system=", f.system),
        ("&title=", f.title),
        ("&field:where=", f.where_),
        ("&field:what=", f.what),
        ("&field:settings=", f.settings),
    ];
    let mut out = ISSUE_FORM.to_string();
    for (key, value) in parts {
        if value.is_empty() {
            continue;
        }
        let part = format!("{key}{}", url_part(value));
        if out.len() + part.len() <= LINK_MAX {
            out.push_str(&part);
        }
    }
    out
}

/// Where the report is after Report manually: on the clipboard for good, only while this program runs, or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copied {
    /// Not on the clipboard.
    No,
    /// Kept there by the system or a tool.
    Kept,
    /// There only while this program runs.
    WhileOpen,
}

/// The box after Report manually: whether the report is on the clipboard, whether the issue form opened, and the saved
/// report's path. Never says it was copied when it was not.
pub fn ready_text(copied: Copied, opened: bool, saved: Option<&str>) -> String {
    let mut s = String::from(match (copied, saved) {
        (Copied::Kept | Copied::WhileOpen, _) => "The report is on your clipboard. ",
        (Copied::No, Some(_)) => {
            "The report could not be put on the clipboard. Click Open the report, select all of it and copy it. "
        }
        (Copied::No, None) => "The report could not be put on the clipboard, nor saved. ",
    });
    if opened {
        s.push_str("In the issue form that opened in your browser, paste it into the longshot_log.txt box, say what you were doing, and fill in the rest.");
    } else {
        s.push_str(&format!("Open {ISSUES}, choose Game problem, and paste it into the longshot_log.txt box."));
    }
    if let Some(path) = saved {
        s.push_str(&format!("\n\nIt is saved as {path}."));
    }
    if copied == Copied::WhileOpen {
        s.push_str("\n\nKeep this window open until you have pasted it.");
    }
    s
}

/// Linux's clipboard tools, in turn: wl-copy on Wayland, then xclip and xsel on X11 (XWayland too). The text goes on
/// the tool's input.
pub fn clip_tools(wayland: bool, x11: bool) -> Vec<&'static [&'static str]> {
    let mut v: Vec<&'static [&'static str]> = Vec::new();
    if wayland {
        v.push(&["wl-copy"]);
    }
    if x11 {
        v.push(&["xclip", "-selection", "clipboard", "-in"]);
        v.push(&["xsel", "--clipboard", "--input"]);
    }
    v
}

/// Whether the builder runs under gamescope (the Steam Deck's Game Mode, a gamescope session), where a window of its
/// own may never be seen: GAMESCOPE_WAYLAND_DISPLAY set, or `gamescope` in XDG_CURRENT_DESKTOP's list.
pub fn under_gamescope(gamescope_display: Option<&str>, desktop: Option<&str>) -> bool {
    gamescope_display.is_some_and(|d| !d.is_empty())
        || desktop.is_some_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("gamescope")))
}
