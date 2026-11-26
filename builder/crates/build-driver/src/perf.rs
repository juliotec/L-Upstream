//! The launcher's side of the performance statistics (`gpu/vk/perf_report.cpp` is the game's): the tick, and sending
//! what the game left in the game folder's `perf_pending/` (`longshot-builder perf-report`).
//!
//! - The launcher's "Send performance statistics" tick is `send=1` in the game folder's `longshot_perf.ini`, off by
//!   default. The game reads it at its start: off, it measures nothing and writes nothing.
//! - On, the game writes a summary (JSON, the relay's version 1) every 5 minutes and at exit into `perf_pending/`. The
//!   launcher sends them at its next start, in a process of its own (`perf-report <folder> --send`, left running), to
//!   the relay the builder's own tree names (its `assets/addresses.ini`'s `perf_reports`, a release's alone: a
//!   developer's checkout names none, and nothing is sent; never the game folder's `longshot_version.txt`, which
//!   anyone who can write the folder can change), oldest first, and deletes each one the relay took or refused; one it
//!   couldn't reach stays for next time. A file there that is not a summary as the game writes one ([`schema`]) is
//!   dropped unsent. What is sent is the game's summary as it is, `version` and `release` filled in.
//! - The sender fills in `version` and `release` (the release flag as the crash report has it: a release's version,
//!   built from the release's own sources), which the game can't know.
//! - Each summary the relay took is also kept, as it was sent, in `perf_sent/` (the 48 sent last). The launcher's
//!   "Keep a copy of what was sent" (`keep_sent=0` in `longshot_perf.ini` when it is off), or
//!   `LONGSHOT_PERF_KEEP_SENT`, turns that off, and empties `perf_sent/`.
//! - Turning the tick off deletes what is waiting, and what `perf_sent/` kept: the player said no.
//! - Polite by default: at most `LONGSHOT_PERF_MAX_SENDS` (5: under the perf route's ingress, 6 a minute with a burst
//!   of 6) a start, one sender at a time (`perf_pending/.sending`, a lease its holder renews), and a relay that answers
//!   busy (429 or 5xx) is left alone until the time it names (`Retry-After`), else an hour, doubled each time it is busy
//!   again up to a day (`perf_pending/.next`). A status that is not the relay saying the summary will never be taken
//!   (only 400, 413, 415, 422) keeps the summary for the next start. The switches: docs/dev/reference/profiling.md,
//!   "Performance statistics".
//!
//! Each summary is sent by curl (every Windows 10 and 11 has it, as most Linux systems: runtimes.rs downloads with it
//! too), its body on its standard input, a redirect never followed (its 3xx is the answer: a redirect's 200 at the far
//! end is not the relay taking the summary). curl takes the proxy the environment names (`HTTPS_PROXY`).

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The tick and the copies' choice (the game reads it: `gpu/vk/perf_report.cpp`).
pub const CHOICE: &str = "longshot_perf.ini";
/// What waits to be sent.
pub const PENDING: &str = "perf_pending";
/// A copy of each summary the relay took, as sent: the newest [`MAX_PENDING`].
pub const SENT: &str = "perf_sent";
const VERSION_FILE: &str = "longshot_version.txt";
/// The newest kept when the launcher hasn't been back for long: older ones go unsent.
pub const MAX_PENDING: usize = 48;
/// The relay's limit is 64 KiB: a bigger one is refused there anyway (`gpu/vk/perf_report.h`'s, the same).
pub const MAX_BODY: usize = 60 * 1024;
/// One start's share, oldest first: the perf route's ingress takes 6 a minute with a burst of 6 per address (the
/// deploy repository's `deploy/telemetry/40-ingress.yaml`); if you raise one, raise the other in the same change.
pub const MAX_SENDS: usize = 5;
/// In `perf_pending/`: when the relay may be asked again after it answered busy, and the wait it took.
const NEXT: &str = ".next";
/// In `perf_pending/`: the send lock while a sender runs; its body names the holder's pid and its lease's end.
const LOCK: &str = ".sending";
/// A lock's lease, in seconds.
const LEASE: u64 = 900;
/// The heartbeat's period: the holder renews its lease this often while it sends.
const BEAT: u64 = 30;
/// A lease may run out this far before the lock stops holding (a heartbeat mid-beat on a stalled file system).
const STALL: u64 = 4 * BEAT;
/// A body with no lease (an older sender's, or one being made): this old means its sender is gone.
const LEGACY: u64 = 60;
/// A busy relay's first wait without a `Retry-After`, and the longest, in seconds.
const BUSY_FIRST: u64 = 3600;
const BUSY_MOST: u64 = 86_400;
/// The statuses that mean "this summary will never be taken": only those are deleted.
const REFUSED: [u16; 4] = [400, 413, 415, 422];
/// The game's logs, newest first: in `logs/`, else at the top of folders made before it moved there.
const LOG_NAMES: [&str; 5] =
    ["longshot_log.txt", "longshot_log.1.txt", "longshot_log.2.txt", "longshot_log.3.txt", "longshot_log.4.txt"];

/// Where a run stands (the `perf_report` event).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// The tick (`send=1`).
    pub tick: bool,
    /// Whether copies of what was sent are kept in `perf_sent/`.
    pub keep_sent: bool,
    /// Summaries waiting in `perf_pending/` (after a send: those still there).
    pub waiting: usize,
    /// Copies in `perf_sent/`.
    pub kept: usize,
    /// Sent now (the relay took them).
    pub sent: usize,
    /// Dropped now (the relay refused them, or they did not read).
    pub refused: usize,
    /// The game folder's version (`longshot_version.txt`, else the log's first line).
    pub version: String,
    /// Whether it is a release's, built from the release's own sources.
    pub release: bool,
    /// Where summaries go; empty: nowhere.
    pub relay: String,
    /// While the relay asked to be left alone: until when (Unix seconds).
    pub busy_until: Option<u64>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The process's environment as read here (tests give theirs).
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

fn env_number(env: Env<'_>, name: &str, default: u64, low: u64, high: u64) -> u64 {
    let Some(raw) = env(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else { return default };
    match raw.parse::<u64>() {
        Ok(v) => v.clamp(low, high),
        Err(_) => default,
    }
}

/// A switch set on or off; `None` unset or not a word [`build_core::switch`] knows (then the default). Set but empty is
/// off, as the game reads it (the stricter of the two rules this had: it took empty for unset).
fn env_switch(env: Env<'_>, name: &str) -> Option<bool> {
    build_core::switch(&env(name)?)
}

fn choices(folder: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read(folder.join(CHOICE)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    text.lines().filter_map(|l| l.trim().split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The tick: on only with `send=1` in the game folder.
pub fn ticked(folder: &Path) -> bool {
    choices(folder).get("send").map(String::as_str) == Some("1")
}

/// Whether a copy of each summary sent is kept: `LONGSHOT_PERF_KEEP_SENT`, else the launcher's choice (`keep_sent=0`:
/// off), on by default.
pub fn keeping_sent(folder: &Path, env: Env<'_>) -> bool {
    env_switch(env, "LONGSHOT_PERF_KEEP_SENT")
        .unwrap_or_else(|| choices(folder).get("keep_sent").map(String::as_str) != Some("0"))
}

/// The launcher's choice of copies as the folder has it (`keep_sent=0`: off; on by default): what `--tick` keeps when
/// `--keep-sent` is not given.
pub fn keep_sent_choice(folder: &Path) -> bool {
    choices(folder).get("keep_sent").map(String::as_str) != Some("0")
}

/// Deletes `sub`'s files whose names end with one of `ends`; nothing raised.
fn remove_all(folder: &Path, sub: &str, ends: &[&str]) {
    let Ok(dir) = std::fs::read_dir(folder.join(sub)) else { return };
    for e in dir.flatten() {
        if ends.iter().any(|end| e.file_name().to_string_lossy().ends_with(end)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

const PENDING_FILES: [&str; 3] = [".json", ".tmp", NEXT];
const SENT_FILES: [&str; 2] = [".json", ".tmp"];

/// The tick, saved for the game (it reads it at its start), and whether copies of what was sent are kept (off empties
/// `perf_sent/`). The tick off deletes what is waiting, and the copies.
pub fn set_tick(folder: &Path, on: bool, keep_sent: bool) -> std::io::Result<()> {
    let path = folder.join(CHOICE);
    if on {
        let tmp = folder.join(format!("{CHOICE}.tmp"));
        std::fs::write(&tmp, format!("send=1\n{}", if keep_sent { "" } else { "keep_sent=0\n" }))?;
        build_core::fsx::rename(&tmp, &path)?;
        if !keep_sent {
            remove_all(folder, SENT, &SENT_FILES);
        }
        return Ok(());
    }
    let _ = std::fs::remove_file(&path);
    remove_all(folder, PENDING, &PENDING_FILES);
    remove_all(folder, SENT, &SENT_FILES);
    Ok(())
}

/// A summary's place, oldest first: its name is `<unix time>-<n>.json`.
fn order(name: &str) -> (u64, u64) {
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    match name.strip_suffix(".json").and_then(|stem| stem.split_once('-')) {
        Some((a, b)) if digits(a) && digits(b) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        _ => (0, 0),
    }
}

fn listed(folder: &Path, sub: &str) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(folder.join(sub)) else { return Vec::new() };
    dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".json")).collect()
}

/// The summaries waiting, oldest first.
pub fn waiting(folder: &Path) -> Vec<String> {
    let mut names = listed(folder, PENDING);
    names.sort_by_key(|n| order(n));
    names
}

/// A release's version (docs/dev/reference/releases.md), as the crash report has it.
fn release_version(v: &str) -> bool {
    let v = v.strip_prefix('v').unwrap_or(v);
    let numbers = |s: &str| {
        let p: Vec<&str> = s.split('.').collect();
        p.len() == 3 && p.iter().all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
    };
    if let Some(rest) = v.strip_prefix("nightly-") {
        return match rest.split_once('-') {
            Some((n, h)) => {
                !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && h.len() == 8
                    && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            }
            None => false,
        };
    }
    match v.split_once("-rc.") {
        Some((n, rc)) => numbers(n) && !rc.is_empty() && rc.bytes().all(|b| b.is_ascii_digit()),
        None => numbers(v),
    }
}

/// `(version, release, relay)` of the game in that folder: `longshot_version.txt`'s version and `perf_reports=` (an
/// https address, else none), and the release flag (a release's version, and the log's source fingerprint the same as
/// the release's own `source=` line).
pub fn build_facts(folder: &Path) -> (String, bool, String) {
    let (mut version, mut expected, mut relay) = (String::new(), String::new(), String::new());
    let stamp =
        std::fs::read(folder.join(VERSION_FILE)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    for line in stamp.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("version=") {
            version = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("source=") {
            expected = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("perf_reports=") {
            // (as the game reads it, gpu/vk/perf_report.h stamp_address: https, no space in it)
            if v.starts_with("https://") && !v.contains([' ', '\t']) {
                relay = v.to_string();
            }
        }
    }
    let mut source = String::new();
    'logs: for dir in ["logs", ""] {
        for name in LOG_NAMES {
            let path = if dir.is_empty() { folder.join(name) } else { folder.join(dir).join(name) };
            let Ok(mut f) = std::fs::File::open(&path) else { continue };
            let mut head = vec![0; 4096];
            let n = f.read(&mut head).unwrap_or(0);
            let text = String::from_utf8_lossy(&head[..n]);
            let first = text.lines().next().unwrap_or("");
            // (the first line's "longshot <version> (source <hex>)")
            if let Some(at) =
                first.find("longshot ").filter(|&at| at == 0 || !first.as_bytes()[at - 1].is_ascii_alphanumeric())
            {
                let rest = &first[at + "longshot ".len()..];
                let mut words = rest.split_whitespace();
                if let Some(v) = words.next() {
                    if version.is_empty() {
                        version = v.to_string();
                    }
                    if words.next() == Some("(source") {
                        source = words.next().unwrap_or("").trim_end_matches(')').to_string();
                        if !source.bytes().all(|b| b.is_ascii_hexdigit()) {
                            source.clear();
                        }
                    }
                    break 'logs;
                }
            }
        }
    }
    let release = release_version(&version) && !expected.is_empty() && source == expected;
    (version, release, relay)
}

/// Where summaries go: `LONGSHOT_PERF_URL` (https, or http to this PC only, for a local relay), else the folder's
/// `perf_reports=`.
fn endpoint(env: Env<'_>, stamped: &str) -> String {
    let raw = env("LONGSHOT_PERF_URL").map(|v| v.trim().to_string()).unwrap_or_default();
    if raw.is_empty() {
        return stamped.to_string();
    }
    let host = |rest: &str| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let h = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if let Some(v6) = h.strip_prefix('[') {
            v6.split(']').next().unwrap_or("").to_string()
        } else {
            h.split(':').next().unwrap_or("").to_string()
        }
    };
    if let Some(rest) = raw.strip_prefix("https://") {
        if !host(rest).is_empty() {
            return raw;
        }
    } else if let Some(rest) = raw.strip_prefix("http://")
        && matches!(host(rest).as_str(), "127.0.0.1" | "localhost" | "::1")
    {
        return raw;
    }
    stamped.to_string()
}

/// How the relay answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// 2xx: taken.
    Sent,
    /// Never to be taken (only [`REFUSED`]'s statuses): it goes.
    Refused,
    /// 429, 5xx: asked again after the wait (seconds, when it named one).
    Busy(Option<u64>),
    /// Anything else (a redirect, 404 while the service is redeployed, 408, not reached): kept for next time.
    Later,
}

/// The answer a status and its `Retry-After` make.
pub fn answer(code: u16, retry_after: Option<&str>) -> Answer {
    match code {
        200..=299 => Answer::Sent,
        429 | 500.. => Answer::Busy(retry_after.and_then(wait_of)),
        c if REFUSED.contains(&c) => Answer::Refused,
        _ => Answer::Later,
    }
}

/// `Retry-After`'s seconds: a number, or an HTTP date (IMF-fixdate, `Sun, 06 Nov 1994 08:49:37 GMT`).
fn wait_of(v: &str) -> Option<u64> {
    let v = v.trim();
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        return v.parse().ok();
    }
    let parts: Vec<&str> = v.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts.as_slice() else { return None };
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| m == month)? as i64
        + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let t: Vec<i64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [h, m, s] = t.as_slice() else { return None };
    // (days from the civil date: Howard Hinnant's algorithm)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let at = (era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + m * 60 + s;
    Some((at as f64 - now()).max(0.0) as u64)
}

/// One summary POSTed with curl: its answer, or [`Answer::Later`] when not reached or cut short.
pub fn post(body: &[u8], url: &str, timeout: u64) -> Answer {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let Some(curl) = crate::runtimes::downloader(
        "curl",
        std::env::var_os("PATH"),
        &exe.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
    ) else {
        return Answer::Later;
    };
    // (http only to this PC: endpoint() lets no other through)
    let proto = if url.starts_with("https://") { "=https" } else { "=http" };
    let mut cmd = build_core::child::command(curl);
    cmd.args(["-q", "--globoff", "-sS", "--proto", proto, "--proto-redir", "=https", "-X", "POST"])
        .args(["--data-binary", "@-"])
        .args(["-H", "Content-Type: application/json", "-H", "User-Agent: longshot-perf-report", "-H", "Expect:"])
        .args(["--connect-timeout", &timeout.to_string(), "--max-time", &(timeout * 2).to_string()])
        .args(["--noproxy", "127.0.0.1,localhost,::1", "-o", null, "-D", "-", "-w", "\n%{http_code}\n"])
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = build_core::child::spawn(&mut cmd) else { return Answer::Later };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body);
    }
    let Ok(out) = child.wait_with_output() else { return Answer::Later };
    if !out.status.success() {
        return Answer::Later;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let code: u16 = text.trim_end().rsplit('\n').next().and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    if code == 0 {
        return Answer::Later;
    }
    let retry = text
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.trim().to_string());
    answer(code, retry.as_deref())
}

fn pending(folder: &Path, name: &str) -> PathBuf {
    folder.join(PENDING).join(name)
}

/// `(when the relay may be asked again, the wait that set it)`: `(0, 0)` when it wasn't busy.
fn busy_until(folder: &Path) -> (f64, u64) {
    let text = std::fs::read_to_string(pending(folder, NEXT)).unwrap_or_default();
    let mut w = text.split_whitespace();
    match (w.next().and_then(|a| a.parse().ok()), w.next().and_then(|b| b.parse().ok())) {
        (Some(at), Some(wait)) => (at, wait),
        _ => (0.0, 0),
    }
}

/// The relay answered busy: not asked again before its `Retry-After` (at most a day), else an hour, doubled each time
/// it is busy again, up to a day.
fn note_busy(folder: &Path, retry_after: Option<u64>) {
    let (_, before) = busy_until(folder);
    let wait = match retry_after {
        Some(r) => r.min(BUSY_MOST),
        None => BUSY_FIRST.max(before * 2).min(BUSY_MOST),
    };
    let path = pending(folder, NEXT);
    let tmp = pending(folder, ".next.tmp");
    if std::fs::write(&tmp, format!("{} {wait}\n", (now() + wait as f64) as u64)).is_ok() {
        let _ = build_core::fsx::rename(&tmp, &path);
    }
}

// ---- the send lock ----

fn lock_body(pid: u32) -> Vec<u8> {
    format!("{pid} {}\n", now() as u64 + LEASE).into_bytes()
}

/// `(the holder's pid, the lease's end second)`; None when there is no lock to read. A body without a lease (an older
/// sender's) reads as lease 0.
fn lock_read(path: &Path) -> Option<(u32, u64)> {
    let mut buf = [0u8; 64];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let mut w = text.split_whitespace();
    let pid = w.next()?.parse().ok()?;
    let lease = w.next().and_then(|l| l.parse().ok()).unwrap_or(0);
    Some((pid, lease))
}

/// True only when the pid is certainly not running. Unknown (not Linux) reads as not gone: the lease is then the only
/// evidence.
fn pid_gone(pid: u32) -> bool {
    // (Linux: no /proc/<pid> while /proc itself is there; elsewhere unknown)
    cfg!(target_os = "linux")
        && pid > 1
        && Path::new("/proc/self").exists()
        && !Path::new("/proc").join(pid.to_string()).exists()
}

fn age(path: &Path) -> Option<f64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(m).map_or(0.0, |d| d.as_secs_f64()))
}

/// True while the lock at `path` stands for a sender that may still act: a holder whose pid is certainly gone holds
/// nothing; a holder whose lease runs holds; one whose lease ran out holds while its heartbeat keeps touching the lock
/// (its mtime walks with its beats). A body that can't be read (being made): holds until it is [`LEGACY`] old; a
/// directory (an older or broken form) never holds.
fn lock_holds(path: &Path) -> bool {
    let Some((pid, lease)) = lock_read(path) else {
        return match std::fs::metadata(path) {
            Ok(m) if m.is_dir() => false,
            Ok(_) => age(path).is_some_and(|a| a < LEGACY as f64),
            Err(_) => false,
        };
    };
    if pid_gone(pid) {
        return false;
    }
    if lease != 0 && now() < (lease + STALL) as f64 {
        return true;
    }
    age(path).is_some_and(|a| a < if lease != 0 { 2 * BEAT } else { LEGACY } as f64)
}

fn create_lock(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// True when this process may send; false while another sender's lock holds. A takeover claims an expired lock by
/// rename and installs the new one with a create-new (atomic on every file system, the vfat and exfat of a Deck's SD
/// card too), so two racing takers never both reclaim one stale lock; what a claim moved is looked at first, and a live
/// lock taken in error is put back. A folder that can't hold a lock at all: true (nothing can be asked twice under it,
/// and losing summaries to a lock it can't make would be worse).
fn take_send_lock(folder: &Path) -> bool {
    let path = pending(folder, LOCK);
    let me = std::process::id();
    let mine = pending(folder, &format!("{LOCK}.{me}"));
    for _ in 0..3 {
        match create_lock(&path) {
            Ok(mut f) => {
                if f.write_all(&lock_body(me)).is_err() {
                    drop(f);
                    let _ = std::fs::remove_file(&path); // (a lock whose body didn't get written is nobody's)
                }
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return true, // (no lock can be made at all: send unlocked)
        }
        let _ = std::fs::remove_file(&mine); // (our own half-made claim from a send that broke halfway)
        if lock_holds(&path) {
            return false;
        }
        if std::fs::rename(&path, &mine).is_err() {
            continue; // (another taker won the claim: look again)
        }
        if lock_holds(&mine) {
            // (a fresh lock another taker made in the breath we judged: put it back, never over one at path)
            if let Ok(body) = std::fs::read(&mine)
                && let Ok(mut f) = create_lock(&path)
            {
                let _ = f.write_all(&body);
            }
            let _ = std::fs::remove_file(&mine);
            continue;
        }
        if std::fs::remove_file(&mine).is_err() {
            let _ = std::fs::remove_dir(&mine); // (a .sending left as a directory by an older or broken form)
        }
        match create_lock(&path) {
            Ok(mut f) => {
                if f.write_all(&lock_body(me)).is_err() {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                }
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return !lock_holds(&path),
            Err(_) => return true,
        }
    }
    true // (the claim race kept losing to healthy senders: sending goes ahead)
}

/// Removes only our lock: one a takeover handed to the next sender is not ours to remove.
fn release_send_lock(folder: &Path) {
    let path = pending(folder, LOCK);
    if lock_read(&path).map(|(p, _)| p) == Some(std::process::id()) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Every [`BEAT`] the holder renews its lease (our lock's body rewritten by name; a lock that is no longer ours is
/// left alone), until `stop` says the send is done.
fn heartbeat(folder: PathBuf, stop: mpsc::Receiver<()>, beat: Duration) {
    let path = pending(&folder, LOCK);
    let me = std::process::id();
    while stop.recv_timeout(beat) == Err(mpsc::RecvTimeoutError::Timeout) {
        if lock_read(&path).map(|(p, _)| p) != Some(me) {
            return;
        }
        let mine = pending(&folder, &format!("{LOCK}.{me}"));
        if std::fs::write(&mine, lock_body(me)).is_ok() {
            let _ = std::fs::rename(&mine, &path);
        }
    }
}

// ---- sending ----

/// The relay the builder's own tree names: `<root>/assets/addresses.ini`'s `perf_reports` (https), a release's tree
/// alone (its `VERSION` or `version.txt`; a checkout's names none). `root`: the tree ([`crate::layout::tree_root`]).
pub fn tree_relay(root: Option<&Path>) -> String {
    let Some(root) = root.filter(|r| ["VERSION", "version.txt"].iter().any(|f| r.join(f).is_file())) else {
        return String::new();
    };
    crate::folder::read_addresses(&root.join("assets/addresses.ini"))
        .remove("perf_reports")
        .filter(|v| v.starts_with("https://") && !v.contains([' ', '\t']))
        .unwrap_or_default()
}

/// The folder's state, sending nothing (`perf-report <folder>`): the relay the builder's tree names
/// (`LONGSHOT_ROOT`, the pack it carries, the checkout it is in: [`tree_relay`]).
pub fn status(folder: &Path, env: Env<'_>) -> Status {
    let root = crate::layout::tree_root(None, env).ok();
    status_with(folder, env, &tree_relay(root.as_deref()))
}

/// [`status`] with the tree's relay given.
pub fn status_with(folder: &Path, env: Env<'_>, stamped: &str) -> Status {
    let (version, release, _) = build_facts(folder);
    let (at, _) = busy_until(folder);
    Status {
        tick: ticked(folder),
        keep_sent: keeping_sent(folder, env),
        waiting: waiting(folder).len(),
        kept: listed(folder, SENT).len(),
        version,
        release,
        relay: endpoint(env, stamped),
        busy_until: (at > now()).then_some(at as u64),
        ..Status::default()
    }
}

/// Sends what is waiting, when the tick is on (`perf-report <folder> --send`). Stops at the first one the relay
/// couldn't take now, and after `LONGSHOT_PERF_MAX_SENDS`; nothing while the relay asked to be left alone (unless
/// `LONGSHOT_PERF_BACKOFF=0`); nothing while another sender holds the lock.
pub fn send(folder: &Path, env: Env<'_>) -> Status {
    send_with(folder, env, &post, Duration::from_secs(BEAT))
}

/// [`send`] with its POST and its heartbeat's period given (the tests).
pub fn send_with(folder: &Path, env: Env<'_>, poster: &dyn Fn(&[u8], &str, u64) -> Answer, beat: Duration) -> Status {
    let mut s = status(folder, env);
    if !s.tick {
        return s;
    }
    if !take_send_lock(folder) {
        return s; // (another sender's lease runs: everything stays, nothing is asked twice)
    }
    let (tx, rx) = mpsc::channel();
    let f = folder.to_path_buf();
    let beater = std::thread::spawn(move || heartbeat(f, rx, beat));
    send_unlocked(folder, env, poster, &mut s);
    let _ = tx.send(());
    let _ = beater.join(); // (before the release: no beat can land once the lock is removed)
    release_send_lock(folder);
    s.waiting = waiting(folder).len();
    s.kept = listed(folder, SENT).len();
    s.busy_until = Some(busy_until(folder).0).filter(|&at| at > now()).map(|at| at as u64);
    s
}

fn send_unlocked(folder: &Path, env: Env<'_>, poster: &dyn Fn(&[u8], &str, u64) -> Answer, s: &mut Status) {
    let timeout = env_number(env, "LONGSHOT_PERF_TIMEOUT", 10, 2, 60);
    let most = env_number(env, "LONGSHOT_PERF_MAX_SENDS", MAX_SENDS as u64, 1, MAX_PENDING as u64) as usize;
    let backoff = env_switch(env, "LONGSHOT_PERF_BACKOFF") != Some(false);
    let mut names = waiting(folder);
    if names.len() > MAX_PENDING {
        for old in names.drain(..names.len() - MAX_PENDING) {
            let _ = std::fs::remove_file(pending(folder, &old)); // (too many: the oldest go unsent)
        }
    }
    let keep = s.keep_sent;
    if !keep {
        remove_all(folder, SENT, &SENT_FILES);
    }
    if s.relay.is_empty() || backoff && busy_until(folder).0 > now() {
        return;
    }
    let mut all = true;
    for name in names.iter().take(most) {
        if !ticked(folder) {
            // (unticked mid-send: the player said no, and what this loop wrote goes too)
            remove_all(folder, PENDING, &PENDING_FILES);
            all = false;
            break;
        }
        let path = pending(folder, name);
        // (a bad byte: one field's text, not the summary)
        let summary = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&b)).ok())
            .filter(schema::fits);
        let (result, body) = match summary {
            None => (Answer::Refused, Vec::new()), // (unreadable, or not a summary the game writes: dropped)
            Some(mut v) => {
                // (the game's own version, of the build that wrote it; a release only when that is this folder's)
                let own = v.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let version = if own.is_empty() { s.version.clone() } else { own };
                v["release"] = (s.release && version == s.version).into();
                v["version"] = version.into();
                let body = serde_json::to_vec(&v).unwrap_or_default();
                let r = if body.len() > MAX_BODY { Answer::Refused } else { poster(&body, &s.relay, timeout) };
                (r, body)
            }
        };
        match result {
            Answer::Busy(wait) => {
                if backoff {
                    note_busy(folder, wait);
                }
                if !ticked(folder) {
                    remove_all(folder, PENDING, &PENDING_FILES);
                }
                all = false;
                break;
            }
            Answer::Later => {
                all = false;
                break;
            }
            Answer::Sent | Answer::Refused => {
                let _ = std::fs::remove_file(&path);
                if result == Answer::Sent {
                    s.sent += 1;
                    keep_copy(folder, env, name, &body);
                } else {
                    s.refused += 1;
                }
            }
        }
    }
    if all {
        let _ = std::fs::remove_file(pending(folder, NEXT));
    }
    if s.sent > 0 && keep {
        prune_sent(folder);
    }
}

/// A copy of what the relay took, as sent, in `perf_sent/`; none once the tick or the copies are off.
fn keep_copy(folder: &Path, env: Env<'_>, name: &str, body: &[u8]) {
    if !ticked(folder) || !keeping_sent(folder, env) {
        return;
    }
    let dir = folder.join(SENT);
    let tmp = dir.join(format!("{name}.tmp"));
    let ok = std::fs::create_dir_all(&dir).is_ok()
        && std::fs::write(&tmp, body).is_ok()
        && std::fs::rename(&tmp, dir.join(name)).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// `perf_sent/` down to the [`MAX_PENDING`] copies written last (by when they were written there, not by their names).
fn prune_sent(folder: &Path) {
    let dir = folder.join(SENT);
    let mut names: Vec<(SystemTime, String)> = listed(folder, SENT)
        .into_iter()
        .map(|n| (std::fs::metadata(dir.join(&n)).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), n))
        .collect();
    names.sort();
    if names.len() > MAX_PENDING {
        for (_, old) in &names[..names.len() - MAX_PENDING] {
            let _ = std::fs::remove_file(dir.join(old));
        }
    }
}

pub mod schema;

#[cfg(test)]
mod tests;
