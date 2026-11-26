//! The files: the game's records in `bug-reports/pending/` (hos_crash_record.c writes them; a run's marker left in
//! `bug-reports/running/` becomes an unclean record here, as the game's own start would make it), which were shown
//! (`bug-reports/shown.txt`), which were sent (`bug-reports/sent/`), and the run's log.

use crate::facts::{LogFacts, record_value};
use crate::names;
use crate::redact::Who;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Where a run's files are. The play side knows them (the game folder is a build's, `builds/<stamp>/`; the saves and
/// the logs may be in the profile): [`Paths::of_game`] for the game's own defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The game folder: `longshot_version.txt` (the report's address, the release's source) and
    /// `longshot_defaults.ini`.
    pub game: PathBuf,
    /// The records' folder, `bug-reports/` (the game's: in its game folder).
    pub reports: PathBuf,
    /// The run's log (`logs/longshot_log.txt`), None when it logged to no file.
    pub log: Option<PathBuf>,
    /// The run's `longshot_options.ini`, None for the game folder's.
    pub options: Option<PathBuf>,
    /// `longshot_crash.ini` (`send=1`: reports sent with no window), written by the launcher.
    pub crash_ini: PathBuf,
    /// What the diagnostics bundle adds to the logs: the build's record, the profile's profile.json, the update's log
    /// (never a save: [`crate::diag::allowed`]).
    pub extra: Vec<PathBuf>,
}

impl Paths {
    /// The game's own places in `game`: `bug-reports/`, `logs/longshot_log.txt`, `longshot_crash.ini`, the options file
    /// the game folder's.
    pub fn of_game(game: &Path) -> Paths {
        Paths {
            game: game.to_path_buf(),
            reports: game.join("bug-reports"),
            log: Some(game.join("logs").join("longshot_log.txt")),
            options: None,
            crash_ini: game.join("longshot_crash.ini"),
            extra: Vec::new(),
        }
    }

    /// The diagnostics bundle: the run's log (`found`, else the one given) and the four before it
    /// (`longshot_log.<n>.txt`), then [`Paths::extra`].
    pub fn diagnostics(&self, found: Option<&Path>) -> crate::diag::Bundle {
        let mut b = crate::diag::Bundle::default();
        if let Some(log) = found.or(self.log.as_deref()) {
            b.add(log);
            if let (Some(stem), Some(ext)) = (log.file_stem(), log.extension()) {
                let (stem, ext) = (stem.to_string_lossy().into_owned(), ext.to_string_lossy().into_owned());
                let base =
                    stem.rsplit_once('.').filter(|(_, n)| n.parse::<u8>().is_ok()).map_or(stem.as_str(), |(b, _)| b);
                for n in 1..=4 {
                    b.add(log.with_file_name(format!("{base}.{n}.{ext}")));
                }
            }
        }
        for f in &self.extra {
            b.add(f);
        }
        b
    }

    /// `bug-reports/pending/`.
    pub fn pending(&self) -> PathBuf {
        self.reports.join("pending")
    }

    /// `bug-reports/sent/`.
    pub fn sent(&self) -> PathBuf {
        self.reports.join("sent")
    }

    /// `bug-reports/shown.txt`.
    pub fn ledger(&self) -> PathBuf {
        self.reports.join("shown.txt")
    }

    /// The run's options file: the one given, else the game folder's.
    pub fn options_file(&self) -> PathBuf {
        self.options.clone().unwrap_or_else(|| self.game.join("longshot_options.ini"))
    }

    /// The folder Open the log folder opens: the log's, else the game folder's `logs/`.
    pub fn log_folder(&self, found: Option<&Path>) -> PathBuf {
        found
            .or(self.log.as_deref())
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.game.join("logs"))
    }
}

/// A file's first `max` bytes as text (lossy), None when it cannot be read.
pub(crate) fn read_text(path: &Path, max: u64) -> Option<String> {
    let mut b = Vec::new();
    fs::File::open(path).ok()?.take(max).read_to_end(&mut b).ok()?;
    Some(String::from_utf8_lossy(&b).into_owned())
}

/// A file replaced whole: written beside it, then renamed over it.
pub(crate) fn replace_text(path: &Path, text: &str) -> bool {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    if fs::write(&temp, text).is_err() {
        let _ = fs::remove_file(&temp);
        return false;
    }
    let ok = fs::rename(&temp, path).is_ok();
    if !ok {
        let _ = fs::remove_file(&temp);
    }
    ok
}

/// The names of the files in a folder (not starting with `.`).
pub fn names(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect()
}

/// Seconds since 1970, now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// ---- a run's marker (hos_crash_record.c's Session, as the game writes it) ----

/// The marker's layout: magic[24], pid, epoch, monotonic_ms (u64), record_complete (u32), run_id[33], version[129],
/// log_path[4096], padded to 8: what hos_crash_record.c's `Session` is on x86-64, Linux and Windows alike.
const SESSION: usize = 4312;
const AT_PID: usize = 24;
const AT_EPOCH: usize = 32;
const AT_COMPLETE: usize = 48;
const AT_RUN: usize = 52;
const AT_VERSION: usize = 85;
const AT_LOG: usize = 214;

struct Session {
    pid: u64,
    epoch: u64,
    complete: bool,
    run_id: String,
    version: String,
    log_path: String,
}

fn c_string(b: &[u8]) -> Option<String> {
    let n = b.iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&b[..n]).into_owned())
}

fn valid_run_id(id: &str) -> bool {
    id == "unknown" || (id.len() == 32 && id.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
}

fn session_of(b: &[u8]) -> Option<Session> {
    if b.len() < SESSION || &b[..19] != b"Longshot running 1\0" {
        return None;
    }
    let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or_default());
    let run_id = c_string(&b[AT_RUN..AT_VERSION])?;
    if !valid_run_id(&run_id) {
        return None;
    }
    Some(Session {
        pid: u64_at(AT_PID),
        epoch: u64_at(AT_EPOCH),
        complete: u32::from_le_bytes(b[AT_COMPLETE..AT_COMPLETE + 4].try_into().unwrap_or_default()) != 0,
        run_id,
        version: c_string(&b[AT_VERSION..AT_LOG])?,
        log_path: c_string(&b[AT_LOG..AT_LOG + 4096])?,
    })
}

/// What a run's marker says, while or after it runs: its run id and its log (None when there is no marker of that
/// process, or it is not one of ours).
pub fn marker(paths: &Paths, pid: u32) -> Option<(String, Option<PathBuf>)> {
    let mut b = Vec::new();
    fs::File::open(paths.reports.join("running").join(format!("{pid}.running"))).ok()?.read_to_end(&mut b).ok()?;
    let s = session_of(&b)?;
    Some((s.run_id, (!s.log_path.is_empty()).then(|| PathBuf::from(s.log_path))))
}

/// The log's tail facts an unclean record holds (hos_crash_record.c's log_facts): its size, time, and whether its last
/// 4 KB say FATAL, CRASH or device lost.
fn log_facts_lines(log: &Path) -> Vec<String> {
    let Ok(mut f) = fs::File::open(log) else { return Vec::new() };
    let Ok(meta) = f.metadata() else { return Vec::new() };
    if !meta.is_file() {
        return Vec::new();
    }
    let size = meta.len();
    let _ = f.seek(SeekFrom::Start(size.saturating_sub(4096)));
    let mut tail = Vec::new();
    let _ = f.take(4096).read_to_end(&mut tail);
    let t = String::from_utf8_lossy(&tail);
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let yes = |b: bool| if b { "yes" } else { "no" };
    vec![
        format!("log_size_bytes: {size}"),
        format!("log_tail_bytes: {}", tail.len()),
        format!("log_mtime_utc: {}", names::utc(mtime)),
        format!("log_tail_fatal: {}", yes(t.contains("FATAL:"))),
        format!("log_tail_crash: {}", yes(t.contains("CRASH:"))),
        format!("log_tail_device_lost: {}", yes(t.contains("device lost") || t.contains("VK_ERROR_DEVICE_LOST"))),
    ]
}

/// An unclean record of a run whose marker was left (hos_crash_record.c's format_record with no crash).
fn unclean_record(s: &Session, quit: bool, now: i64, who: &Who) -> String {
    let label = |v: &str| {
        let c = crate::facts::cut(&who.redact(v), 159).to_string();
        if c.is_empty() { "unknown".to_string() } else { c }
    };
    let base = s.log_path.rsplit(['/', '\\']).next().unwrap_or("");
    let mut lines = vec![
        "Longshot unclean record 1".to_string(),
        format!("version: {}", label(&s.version)),
        format!("run_id: {}", s.run_id),
        format!("when: {}", names::utc(now)),
        "uptime_s: unknown".into(),
        "reason: ended without a clean exit".into(),
        "thread: unknown".into(),
        "module_offset: unknown".into(),
        "last_present_age_ms: unknown".into(),
        format!("log: {}", if s.log_path.is_empty() { "unknown".into() } else { label(base) }),
        format!("pid: {}", s.pid),
        format!("started: {}", names::utc(s.epoch as i64)),
        "crash_handler: not observed".into(),
        "termination_note: ended from outside or lost power, not necessarily a crash".into(),
    ];
    if quit {
        lines.push("quit_asked: yes".into());
    }
    if !s.log_path.is_empty() {
        lines.extend(log_facts_lines(Path::new(&s.log_path)));
    }
    lines.push("dump: off".into());
    lines.push("record_complete: yes".into());
    lines.join("\n") + "\n"
}

/// `source` published in pending as `<kind>-<UTC>.txt` (`-<pid>-<n>` before .txt when that is taken): never over an
/// older record. Its name.
fn publish(pending: &Path, source: &Path, kind: &str, now: i64, pid: u64) -> Option<String> {
    let when = names::utc(now);
    for n in 0..100 {
        let name = if n == 0 { format!("{kind}-{when}.txt") } else { format!("{kind}-{when}-{pid}-{n}.txt") };
        let dest = pending.join(&name);
        match fs::hard_link(source, &dest) {
            Ok(()) => {
                let _ = fs::remove_file(source);
                return Some(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // (a file system with no hard links: renamed, when the name is free)
            Err(_) if !dest.exists() => return fs::rename(source, &dest).ok().map(|()| name),
            Err(_) => continue,
        }
    }
    None
}

/// The game's process `pid` has ended: its marker, if it left one (no crash handler ran: killed, out of memory, a
/// power loss), made into its record as the game's next start would, and its record's name. A marker another process
/// still holds is left (Windows lets go of it a moment after the process ends: tried for up to 3 s). `oom`: the
/// out-of-memory killer took it (`oom_kill: yes` in the unclean record).
pub fn recover(paths: &Paths, pid: u32, oom: bool, who: &Who) -> Option<String> {
    let running = paths.reports.join("running");
    let path = running.join(format!("{pid}.running"));
    let mut tries = 0;
    let mut f = loop {
        let f = fs::OpenOptions::new().read(true).write(true).open(&path).ok()?;
        match f.try_lock() {
            Ok(()) => break f,
            Err(_) if tries < 30 => {
                tries += 1;
                drop(f);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    };
    let mut b = Vec::new();
    f.read_to_end(&mut b).ok()?;
    let s = session_of(&b)?;
    let quit = b.get(SESSION..SESSION + 5) == Some(b"quit\n");
    let staging = running.join(format!("{pid}.record"));
    if !staging.exists() && s.complete {
        // (published already: no unclean end to invent)
        drop(f);
        let _ = fs::remove_file(&path);
        return None;
    }
    let saved = read_text(&staging, 24_575).unwrap_or_default();
    let complete = saved.len() > 30
        && saved.starts_with("Longshot crash record 1\n")
        && saved.contains("\nrecord_complete: yes\n");
    let now = now();
    if !complete {
        let mut text = unclean_record(&s, quit, now, who);
        if oom {
            text = text.replace("record_complete: yes\n", "oom_kill: yes\nrecord_complete: yes\n");
        }
        let mut out = fs::OpenOptions::new().write(true).create(true).truncate(true).open(&staging).ok()?;
        out.write_all(text.as_bytes()).ok()?;
        out.sync_all().ok()?;
    }
    let name = publish(&paths.pending(), &staging, if complete { "crash" } else { "unclean" }, now, s.pid)?;
    drop(f);
    let _ = fs::remove_file(&path);
    Some(name)
}

/// The newest record in pending of the game's process `pid` started at or after `since` (seconds since 1970), a crash
/// record before an unclean one: its name and text.
pub fn run_record(paths: &Paths, pid: u32, since: i64) -> Option<(String, String)> {
    let pending = paths.pending();
    let mut best: Option<(u8, String, String)> = None;
    for name in names(&pending) {
        let Some((kind, _)) = names::record_name(&name) else { continue };
        let Some(text) = read_text(&pending.join(&name), 64 << 10) else { continue };
        let r = crate::facts::Record::parse(&text);
        let started = record_value(&text, "started").map_or(0, |w| names::epoch(&w));
        if r.pid != u64::from(pid) || started + 2 < since {
            continue;
        }
        let better = match &best {
            None => true,
            Some((k, n, _)) => (kind == 1 && *k == 2) || (kind == *k && name > *n),
        };
        if better {
            best = Some((kind, name, text));
        }
    }
    best.map(|(_, n, t)| (n, t))
}

/// A record in pending, by its name (a bare name, never a path): its text.
pub fn read_record(paths: &Paths, name: &str) -> Option<String> {
    names::record_name(name)?;
    read_text(&paths.pending().join(name), 64 << 10)
}

/// Adds `name` to the ledger of records shown (None: only the names still pending kept), keeping only names still
/// pending. `all`: every record pending now counts as shown (the first start with a ledger: what was pending before is
/// not news).
pub fn ledger_add(paths: &Paths, name: Option<&str>, all: bool) -> bool {
    let old = read_text(&paths.ledger(), 64 << 10);
    let mut out = String::new();
    let mut pending = names(&paths.pending());
    pending.sort();
    for n in pending {
        if names::record_name(&n).is_some() && (all || old.as_deref().is_none_or(|o| names::listed(o, &n))) {
            out.push_str(&n);
            out.push('\n');
        }
    }
    if let Some(n) = name
        && !names::listed(&out, n)
    {
        out.push_str(n);
        out.push('\n');
    }
    let _ = fs::create_dir_all(&paths.reports);
    replace_text(&paths.ledger(), &out)
}

/// The newest record nobody showed (no builder watched its run, or no window could open): its name; None when there
/// is none, or when there is no ledger yet (then every record pending is listed as shown: not news).
pub fn unshown(paths: &Paths, now: i64) -> Option<String> {
    let Some(shown) = read_text(&paths.ledger(), 64 << 10) else {
        ledger_add(paths, None, true);
        return None;
    };
    let pending = paths.pending();
    names(&pending)
        .into_iter()
        .filter(|n| {
            let oom = names::record_name(n).is_some_and(|(k, _)| k == 2)
                && read_text(&pending.join(n), 64 << 10).is_some_and(|t| crate::facts::Record::parse(&t).oom);
            names::show_at_start(n, &shown, now, oom)
        })
        .max()
}

/// `bug-reports/sent/<record>`: the answer to a report sent.
pub fn sent_already(paths: &Paths, record: &str) -> bool {
    paths.sent().join(record).is_file()
}

/// A report sent: its answer kept in `bug-reports/sent/<record>`, so it is not offered or sent again; with `copy`,
/// what went in `<record without .txt>.json` beside it.
pub fn keep_answer(paths: &Paths, record: &str, status: u16, copy: Option<&str>) {
    let sent = paths.sent();
    let _ = fs::create_dir_all(&sent);
    let _ = fs::write(sent.join(record), format!("status: {status}\n"));
    if let Some(json) = copy {
        let _ = fs::write(sent.join(format!("{}.json", record.strip_suffix(".txt").unwrap_or(record))), json);
    }
}

/// The reports Report manually saved (`pending/report-<UTC>.txt`): the newest 10 kept.
pub fn prune_reports(paths: &Paths) {
    let pending = paths.pending();
    let mut r: Vec<String> = names(&pending)
        .into_iter()
        .filter(|n| n.len() == 27 && n.starts_with("report-") && n.ends_with(".txt"))
        .collect();
    r.sort();
    let extra = r.len().saturating_sub(10);
    for n in &r[..extra] {
        let _ = fs::remove_file(pending.join(n));
    }
}

/// Records of the last day (crash and unclean ends), as the manual report counts them.
pub fn recent(paths: &Paths, now: i64) -> usize {
    names(&paths.pending())
        .iter()
        .filter_map(|n| names::record_name(n))
        .filter(|(_, w)| now - names::epoch(w) <= 86_400)
        .count()
}

// ---- the run's log ----

/// The last lines of the log the report keeps.
pub const LOG_LINES: usize = 200;
/// The report's tail before the crash.
pub const TAIL_LINES: usize = 150;
/// The crash's lines at most.
pub const CRASH_LINES: usize = 80;
const LOG_MAX: u64 = 256 << 20;

/// The run's log: its facts (from its own lines on), its last lines redacted.
#[derive(Clone, Debug, Default)]
pub struct RunLog {
    /// Where it is (None: not found).
    pub path: Option<PathBuf>,
    /// What its own lines say.
    pub facts: LogFacts,
    /// Its last [`LOG_LINES`] lines, redacted.
    pub lines: Vec<String>,
}

fn log_lines(path: &Path) -> Option<impl Iterator<Item = (u64, String)>> {
    let f = fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::new(f.take(LOG_MAX));
    let mut at = 0u64;
    Some(std::iter::from_fn(move || {
        let mut b = Vec::new();
        let n = std::io::BufRead::read_until(&mut r, b'\n', &mut b).ok()?;
        if n == 0 {
            return None;
        }
        let line_at = at;
        at += n as u64;
        Some((line_at, String::from_utf8_lossy(&b).into_owned()))
    }))
}

/// Whether the log at `path` is the run's (its `run_id: <id>` line anywhere: a Linux restart appends to it), and where
/// its own lines start: its banner (`] longshot `) just before that line, else that line.
fn run_at(path: &Path, run_id: &str) -> Option<u64> {
    let want = format!("run_id: {run_id}");
    let mut banner = None;
    for (at, line) in log_lines(path)? {
        if line.contains("] longshot ") {
            banner = Some(at);
        }
        if line.contains(&want) {
            return Some(banner.unwrap_or(at));
        }
    }
    None
}

impl RunLog {
    /// The log of run `run_id`: at `log`, else its first rotated name (`longshot_log.1.txt`: a start since moved it).
    /// Without a run id, the log at `log` as it is.
    pub fn find(log: Option<&Path>, run_id: Option<&str>, who: &Who) -> RunLog {
        let Some(log) = log else { return RunLog::default() };
        let mut tries = vec![log.to_path_buf()];
        if let (Some(stem), Some(ext)) = (log.file_stem(), log.extension()) {
            tries.push(log.with_file_name(format!("{}.1.{}", stem.to_string_lossy(), ext.to_string_lossy())));
        }
        for path in tries {
            let from = match run_id {
                Some(id) => match run_at(&path, id) {
                    Some(at) => at,
                    None => continue,
                },
                None if path.is_file() => 0,
                None => continue,
            };
            return RunLog::read(&path, from, who);
        }
        RunLog::default()
    }

    fn read(path: &Path, from: u64, who: &Who) -> RunLog {
        let mut r = RunLog { path: Some(path.to_path_buf()), ..RunLog::default() };
        let mut ring = std::collections::VecDeque::with_capacity(LOG_LINES);
        for (at, line) in log_lines(path).into_iter().flatten() {
            let clean = who.redact(&line);
            if at >= from {
                r.facts.line(clean.trim_end_matches(['\n', '\r']));
            }
            if ring.len() == LOG_LINES {
                ring.pop_front();
            }
            ring.push_back(clean.trim_end_matches(['\n', '\r']).to_string());
        }
        r.lines = ring.into();
        r
    }

    /// Whether it was found.
    pub fn found(&self) -> bool {
        self.path.is_some()
    }

    /// The crash's lines as the report holds them ([`crate::clean::report_line`]: at most [`CRASH_LINES`]) and the
    /// [`TAIL_LINES`] lines before its first; None when the log has no crash.
    pub fn crash_lines(&self) -> Option<(Vec<String>, Vec<String>)> {
        let mut tail = std::collections::VecDeque::with_capacity(TAIL_LINES);
        let mut crash = Vec::new();
        let mut found = false;
        for (_, line) in log_lines(self.path.as_deref()?)? {
            let line = line.trim_end_matches(['\n', '\r']);
            let line = &line[..line.find(['\r', '\n']).unwrap_or(line.len())];
            if !found && crate::clean::starts(line) {
                found = true;
            }
            if !found {
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line.to_string());
            } else if crash.len() < CRASH_LINES
                && let Some(l) = crate::clean::report_line(line)
            {
                crash.push(l);
            }
        }
        found.then(|| (crash, tail.into()))
    }
}
