//! The shader preparation, supervised: one shader that crashes or hangs the GPU driver costs that shader, not the
//! whole preparation, and is not handed to that driver again by the game (docs/dev/reference/shader-preparation.md).
//!
//! [`crate::shaders::prepare`] starts `longshot --prepare-shaders`. A driver that dies or never returns inside one
//! compile ended that run, the work fell to the game's first start, and the game died in the same place. Here the run
//! is started with a journal (gpu/vk/prepare_journal.h: a line before and after each driver compile call). When it
//! dies, is stopped for silence, or ends itself because one entry passed its time limit (exit status 6), the journal
//! says which entries were inside the driver:
//!
//! - one program open: that is the culprit. If the run crashed, it is appended to `cache/longshot_driver_faults.txt`;
//!   a timeout is left out only for this preparation. The run is started again past it and without it;
//! - several open (the preparation compiles on many threads): the run is started again with all calls serialized, so
//!   that the next death has only one open;
//! - none open: the run is started once more as it was, and a second such death gives up, as before;
//! - more than [`MAX_CULPRITS`] culprits, or [`MAX_STARTS`] starts: give up, as before (a driver that fails on
//!   everything must not loop for hours). Crashed programs found stay in the game's list.
//!
//! Giving up is what a failed preparation always was: "the shaders were not prepared now", and the game prepares them
//! at its first start. The pure parts ([`Journal::parse`], [`Policy`], [`results_text`], [`faults_with`]) take no
//! process and no GPU, and [`supervise`] takes the start of a run as a closure: the tests drive it with a stand-in game.

use std::path::Path;

/// The journal the game writes (`--prepare-journal`), in the game folder's `cache/`.
pub const JOURNAL: &str = "longshot_prepare_journal.txt";
/// The programs a run started again leaves out or builds one at a time (`--prepare-skip`).
pub const ASKED: &str = "longshot_prepare_skip.txt";
/// What the last preparation did with each call into the driver's compiler, kept beside the caches.
pub const RESULTS: &str = "longshot_prepare_results.txt";
/// The game's own list of programs a driver's compiler faulted on (gpu/vk/driver_faults.h).
pub const FAULTS: &str = "longshot_driver_faults.txt";

// (the run's exit statuses: host/hos_main.c, gpu/vk/vk_main.cpp prepare_watchdog and prepare_entry_watch)
/// No usable Vulkan on this PC: nothing to retry.
pub const EXIT_NO_VULKAN: i32 = 3;
/// Nothing moved in the whole run for 300 s: the run left `longshot_prepare_skipped` and gave up.
pub const EXIT_STALLED: i32 = 4;
/// One journalled entry passed its limit: its `t` line names it.
pub const EXIT_ENTRY_LIMIT: i32 = 6;
/// A journal write, flush or marker operation failed.
pub const EXIT_JOURNAL_ERROR: i32 = 7;

/// Seconds one driver call gets. Below the run's own 300 s with nothing moving, so that a stuck shader is named before
/// the whole run is given up; `LONGSHOT_PREPARE_ENTRY_S` in the builder's environment sets another, 0 none.
pub const ENTRY_LIMIT_S: u64 = 240;
/// Programs left out in one preparation; one more death then gives up.
pub const MAX_CULPRITS: usize = 8;
/// Starts of the game in one preparation, whatever they ended with.
pub const MAX_STARTS: usize = 20;
/// Starts again after a death with nothing open in the journal.
pub const MAX_BLIND: usize = 1;

/// An ask that compiles nothing found no cache: not a failure.
const VK_PIPELINE_COMPILE_REQUIRED: i64 = 1_000_297_000;
/// A fault the in-process guard caught (vk_draw.cpp kDriverFault).
const DRIVER_FAULT: i64 = -77;

const FAULTS_HEAD: &str = "# Longshot: programs the graphics driver's compiler faulted on, not asked of it again on that \
                           driver\n# (gpu/vk/driver_faults.h). <vendor> <driver version> <program key>. Delete this file \
                           to try them again.\n";

/// One driver call of the journal.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The run: 1 for the first process that opened the journal.
    pub run: u32,
    /// The entry's number, going on across runs (`--prepare-from`).
    pub seq: u64,
    /// `module`, `graphics` or `compute`.
    pub kind: String,
    /// `<vendor> <driver> <program>` as the journal has it, which is a line of [`FAULTS`].
    pub identity: String,
    /// The program's key (0: a shader of the renderer's own).
    pub key: u64,
    /// The call's own detail (a pipeline's variant), as the journal has it.
    pub detail: String,
    /// The VkResult, `None` while the call has not returned.
    pub result: Option<i64>,
    /// How long it took, or was given before its run named it.
    pub ms: u64,
    /// Its run named it as past its limit.
    pub timed: bool,
    /// Set by [`results_text`] (crashed, timed out) for an entry that never returned.
    pub state: Option<&'static str>,
}

/// What a journal file holds.
#[derive(Debug, Default)]
pub struct Journal {
    /// How many processes opened it.
    pub runs: u32,
    /// Every begin, in the file's order.
    pub entries: Vec<Entry>,
    /// The highest entry number.
    pub last_seq: u64,
    /// `<vendor> <driver>` of the last run.
    pub driver: String,
    /// The last line has no line end (a full disk): the journal cannot be trusted.
    pub incomplete: bool,
}

fn hex(s: &str, len: Option<usize>) -> Option<u64> {
    let ok = !s.is_empty()
        && len.is_none_or(|n| s.len() == n)
        && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if ok { u64::from_str_radix(s, 16).ok() } else { None }
}

fn dec(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl Journal {
    /// A journal's text read. A last line with no line end and lines that are not the journal's are skipped; an end or
    /// a limit names an open entry of its own run, or is skipped.
    pub fn parse(text: &str) -> Journal {
        let mut j = Journal::default();
        let mut lines: Vec<&str> = text.split('\n').collect();
        j.incomplete = !lines.pop().unwrap_or("").is_empty(); // (a line cut short cannot be trusted)
        let mut live: Vec<usize> = Vec::new(); // (the current run's open entries, as indices of entries)
        for line in lines {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let w: Vec<&str> = line.split(' ').collect();
            match w.as_slice() {
                ["run", vendor, driver, seq] if hex(vendor, Some(4)).is_some() && hex(driver, Some(8)).is_some() => {
                    let Some(seq) = hex(seq, None) else { continue };
                    j.runs += 1;
                    j.driver = format!("{vendor} {driver}");
                    j.last_seq = j.last_seq.max(seq);
                    live.clear();
                }
                _ if j.runs == 0 => {}
                ["b", seq, kind @ ("module" | "graphics" | "compute"), vendor, driver, key, detail]
                    if hex(vendor, Some(4)).is_some()
                        && hex(driver, Some(8)).is_some()
                        && hex(detail, Some(16)).is_some() =>
                {
                    let (Some(seq), Some(k)) = (hex(seq, None), hex(key, Some(16))) else { continue };
                    live.retain(|&i| j.entries[i].seq != seq);
                    live.push(j.entries.len());
                    j.entries.push(Entry {
                        run: j.runs,
                        seq,
                        kind: kind.to_string(),
                        identity: format!("{vendor} {driver} {key}"),
                        key: k,
                        detail: detail.to_string(),
                        result: None,
                        ms: 0,
                        timed: false,
                        state: None,
                    });
                    j.last_seq = j.last_seq.max(seq);
                }
                ["e", seq, result, ms] => {
                    let (Some(seq), Some(result), Some(ms)) = (hex(seq, None), dec(result), dec(ms)) else { continue };
                    if let Some(at) = live.iter().position(|&i| j.entries[i].seq == seq) {
                        let e = &mut j.entries[live.remove(at)];
                        e.result = Some(result);
                        e.ms = u64::try_from(ms).unwrap_or(0);
                    }
                }
                ["t", seq, secs] if secs.bytes().all(|b| b.is_ascii_digit()) => {
                    let (Some(seq), Ok(secs)) = (hex(seq, None), secs.parse::<u64>()) else { continue };
                    if let Some(&i) = live.iter().find(|&&i| j.entries[i].seq == seq) {
                        j.entries[i].timed = true;
                        j.entries[i].ms = secs.saturating_mul(1000);
                    }
                }
                _ => {}
            }
        }
        j
    }

    /// The entries of the last run that never returned: where that process was when it ended.
    pub fn open_entries(&self) -> Vec<Entry> {
        self.entries.iter().filter(|e| e.run == self.runs && e.result.is_none()).cloned().collect()
    }

    /// The entry the last run named as past its limit.
    pub fn timed(&self) -> Option<Entry> {
        self.entries.iter().find(|e| e.run == self.runs && e.timed).cloned()
    }
}

/// What the supervisor does after a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Prepared.
    Done,
    /// Started again, with what the policy leaves out or builds one at a time.
    Restart,
    /// Given up: the decision's `why` says it, or the run's exit status does.
    Stop,
}

/// What the supervisor does after a run ended, and why.
#[derive(Clone, Debug)]
pub struct Decision {
    /// What happens next.
    pub action: Action,
    /// A word for a stop: vulkan, stalled, silent, died, own, repeated, culprits, starts, journal.
    pub reason: Option<&'static str>,
    /// The supervisor's own reason for a stop, in words; `None`: the run's exit status says it.
    pub why: Option<String>,
    /// The entry just named.
    pub culprit: Option<Entry>,
    /// How it was named: "crashed" or "timed out".
    pub how: Option<&'static str>,
}

impl Decision {
    fn new(action: Action, reason: Option<&'static str>, why: Option<String>) -> Decision {
        Decision { action, reason, why, culprit: None, how: None }
    }

    fn stop(reason: &'static str, why: Option<String>) -> Decision {
        Decision::new(Action::Stop, Some(reason), why)
    }
}

/// The retry policy, with no process: [`Policy::after`] for each run that ended, in order.
#[derive(Debug)]
pub struct Policy {
    /// Programs left out before a death gives up ([`MAX_CULPRITS`]).
    pub max_culprits: usize,
    /// Starts of the game in all ([`MAX_STARTS`]).
    pub max_starts: usize,
    /// Starts again after a death with nothing open ([`MAX_BLIND`]).
    pub max_blind: usize,
    /// The programs left out so far, and how each was named.
    pub culprits: Vec<(Entry, &'static str)>,
    /// The programs the next run builds one at a time.
    pub suspects: Vec<u64>,
    /// Runs ended so far.
    pub starts: usize,
    /// Deaths with nothing open since the last culprit.
    pub blind: usize,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy::new(MAX_CULPRITS, MAX_STARTS, MAX_BLIND)
    }
}

impl Policy {
    /// A policy with these bounds.
    pub fn new(max_culprits: usize, max_starts: usize, max_blind: usize) -> Policy {
        Policy { max_culprits, max_starts, max_blind, culprits: Vec::new(), suspects: Vec::new(), starts: 0, blind: 0 }
    }

    /// The keys of the programs left out so far.
    pub fn culprit_keys(&self) -> Vec<u64> {
        self.culprits.iter().map(|(e, _)| e.key).collect()
    }

    /// `code`: the run's exit status (`None`: it printed nothing for too long and was stopped). `opened`: the entries
    /// its journal left open ([`Journal::open_entries`]). `timed`: the entry it named as past its limit.
    pub fn after(&mut self, code: Option<i32>, opened: &[Entry], timed: Option<&Entry>) -> Decision {
        self.starts += 1;
        match code {
            Some(0) => return Decision::new(Action::Done, None, None),
            Some(EXIT_NO_VULKAN) => return Decision::stop("vulkan", None),
            Some(EXIT_STALLED) => return Decision::stop("stalled", None),
            _ => {}
        }
        let hung = code.is_none() || code == Some(EXIT_ENTRY_LIMIT);
        let how = if hung { "timed out" } else { "crashed" };
        let verb = if hung { "did not return" } else { "failed" };
        let named;
        let opened = match (code, timed) {
            (Some(EXIT_ENTRY_LIMIT), Some(t)) => {
                named = [t.clone()]; // (the run said which: the others open beside it were only interrupted)
                &named[..]
            }
            _ => opened,
        };
        let mut keys: Vec<u64> = Vec::new();
        for e in opened {
            if !keys.contains(&e.key) {
                keys.push(e.key);
            }
        }
        if keys.is_empty() || keys == [0] {
            if !opened.is_empty() {
                // (only shaders of the renderer's own: no list can leave those out)
                return Decision::stop(
                    "own",
                    Some(format!("the GPU driver {verb} on one of the renderer's own shaders")),
                );
            }
            if code.is_none() {
                // (silent with nothing inside the driver: not the driver's compiler, and 15 minutes a try)
                return Decision::stop("silent", None);
            }
            if self.blind >= self.max_blind {
                return Decision::stop("died", None);
            }
            self.blind += 1;
            return self.restart();
        }
        let left_out = self.culprit_keys();
        if keys.iter().any(|k| left_out.contains(k)) {
            // (left out and inside the driver all the same: the run does not follow the list, so no restart helps)
            return Decision::stop(
                "repeated",
                Some(format!("the GPU driver {verb} again on a shader that was already left out")),
            );
        }
        if keys.len() > 1 {
            // (all calls, including the renderer's own, must be isolated on the retry)
            self.suspects = keys.into_iter().filter(|&k| k != 0).collect();
            return self.restart();
        }
        let entry = opened.iter().find(|e| e.key == keys[0]).cloned().expect("an open entry has the key");
        self.culprits.push((entry.clone(), how));
        self.suspects.clear();
        self.blind = 0;
        let mut d = if self.culprits.len() > self.max_culprits {
            // (named and left out as the others, but no further start)
            Decision::stop(
                "culprits",
                Some(format!("the GPU driver failed on more than {} shaders", self.max_culprits)),
            )
        } else {
            self.restart()
        };
        d.culprit = Some(entry);
        d.how = Some(how);
        d
    }

    fn restart(&self) -> Decision {
        if self.starts >= self.max_starts {
            return Decision::stop(
                "starts",
                Some(format!("the game was started {} times and the GPU driver still fails", self.starts)),
            );
        }
        Decision::new(Action::Restart, None, None)
    }
}

/// The file a restarted run reads (`--prepare-skip`): the programs left out and the suspects built one at a time.
pub fn asked_text(culprits: &[u64], suspects: &[u64]) -> String {
    let skip = culprits.iter().map(|k| format!("skip {k:016x}\n"));
    skip.chain(suspects.iter().map(|k| format!("alone {k:016x}\n"))).collect()
}

/// [`FAULTS`]'s text with one more program, or `None` when it names it already. `identity`: the journal's
/// `<vendor> <driver> <program>`, written as it is: the game matches a line by those three numbers.
pub fn faults_with(text: &str, identity: &str) -> Option<String> {
    if text.lines().any(|l| l.trim() == identity) {
        return None;
    }
    let mut text = if text.trim().is_empty() { FAULTS_HEAD.to_string() } else { text.to_string() };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Some(text + identity + "\n")
}

/// built, cold (asked without compiling, in no cache), faulted (the guard caught it), failed, or what
/// [`results_text`] gave an entry that never returned.
fn entry_state(e: &Entry) -> &'static str {
    match e.result {
        None => e.state.unwrap_or("interrupted"),
        Some(0) => "built",
        Some(VK_PIPELINE_COMPILE_REQUIRED) => "cold",
        Some(DRIVER_FAULT) => "faulted",
        Some(_) => "failed",
    }
}

const STATES: [&str; 7] = ["built", "cold", "failed", "faulted", "crashed", "timed out", "interrupted"];

fn row(e: &Entry) -> String {
    let state = entry_state(e);
    let mut row = format!("{} {} {} {} {}", state.replace(' ', "-"), e.kind, e.identity, e.detail, e.ms);
    if let (Some(r), "failed" | "faulted") = (e.result, state) {
        row += &format!(" {r}");
    }
    row
}

/// The result list: the totals, every entry that was not built, and the slowest that were. `culprits`:
/// [`Policy::culprits`]. `ended`: how the preparation ended, in words.
pub fn results_text(journal: &mut Journal, culprits: &[(Entry, &'static str)], ended: &str, slowest: usize) -> String {
    // (a culprit is an entry of the journal as it was read when its run ended, and supervise reads the file again after
    // every run: the state goes to this journal's own entry of that run and number, which is the one counted and listed)
    let mut extra = Vec::new(); // (culprits this journal has no entry of)
    let mut named = Vec::new(); // (indices of journal entries that are culprits)
    for (e, how) in culprits {
        match journal.entries.iter().position(|x| x.run == e.run && x.seq == e.seq) {
            Some(i) => {
                journal.entries[i].state = Some(how);
                named.push(i);
            }
            None => extra.push(Entry { state: Some(how), ..e.clone() }),
        }
    }
    let mut counts = [0usize; STATES.len()];
    for e in journal.entries.iter().chain(&extra) {
        if let Some(i) = STATES.iter().position(|s| *s == entry_state(e)) {
            counts[i] += 1;
        }
    }
    let mut out = vec![
        "# Longshot: what the last shader preparation at a build did with each call into the GPU driver's compiler"
            .to_string(),
        "# (builder/crates/build-driver/src/supervisor.rs, docs/dev/reference/shader-preparation.md). Made on this PC \
         for this driver; safe to delete."
            .to_string(),
        format!("driver {}", if journal.driver.is_empty() { "unknown" } else { &journal.driver }),
        format!("runs {}", journal.runs),
        format!("ended {ended}"),
        format!("entries {}", journal.entries.len()),
    ];
    out.extend(STATES.iter().zip(counts).map(|(s, n)| format!("{} {n}", s.replace(' ', "-"))));
    out.push(format!(
        "# programs left out during preparation (crashes also in {FAULTS}): <state> <kind> <vendor> <driver> <program> \
         <detail> <ms>"
    ));
    // (in the order they were named)
    let mut extra_left = extra.iter();
    for (e, _) in culprits {
        match journal.entries.iter().find(|x| x.run == e.run && x.seq == e.seq) {
            Some(x) => out.push(row(x)),
            None => out.extend(extra_left.next().map(row)),
        }
    }
    out.push(
        "# every other entry that was not built (cold ones aside): <state> <kind> <vendor> <driver> <program> <detail> \
         <ms> [<VkResult>]"
            .to_string(),
    );
    out.extend(
        journal
            .entries
            .iter()
            .enumerate()
            .filter(|(i, e)| !named.contains(i) && !matches!(entry_state(e), "built" | "cold"))
            .map(|(_, e)| row(e)),
    );
    out.push("# the slowest that were built".to_string());
    let mut built: Vec<&Entry> = journal.entries.iter().filter(|e| entry_state(e) == "built").collect();
    built.sort_by_key(|e| std::cmp::Reverse(e.ms));
    out.extend(built.into_iter().take(slowest).map(row));
    out.join("\n") + "\n"
}

fn read(path: &Path) -> String {
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// Whether written. A folder that cannot be written (or is not there) is not an error here.
fn write(path: &Path, text: &str) -> bool {
    let mut new = path.as_os_str().to_owned();
    new.push(".new");
    std::fs::write(&new, text).is_ok() && std::fs::rename(&new, path).is_ok()
}

fn pending(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound, // (cannot establish that the journal is safe to attribute)
    }
}

/// The program added to the game's own list, unless it is there. Whether the list names it afterwards.
pub fn faults_append(cache: &Path, identity: &str) -> bool {
    let path = cache.join(FAULTS);
    faults_with(&read(&path), identity).is_none_or(|text| write(&path, &text))
}

/// Seconds one entry gets: `LONGSHOT_PREPARE_ENTRY_S`, else [`ENTRY_LIMIT_S`]; 0 none.
pub fn entry_limit(get: impl Fn(&str) -> Option<String>) -> u64 {
    match get("LONGSHOT_PREPARE_ENTRY_S").map(|v| v.trim().parse::<i64>()) {
        None | Some(Err(_)) => ENTRY_LIMIT_S,
        Some(Ok(n)) => u64::try_from(n).unwrap_or(0),
    }
}

fn describe(e: &Entry) -> String {
    let what = match e.kind.as_str() {
        "module" => "a shader module",
        "graphics" => "a graphics pipeline",
        _ => "a compute pipeline",
    };
    format!("{what} of program {:016x}", e.key)
}

/// The preparation run until it is done or given up.
///
/// `start(extra)`: starts the game with these arguments after its own and waits; its exit status, or `None` when it
/// was stopped for printing nothing. An error goes to the caller: the game could not start. `cache`: the game
/// folder's `cache/` (the journal, the result list and [`FAULTS`] are there). Returns the last run's exit status and
/// the policy's last decision ([`Action::Done`] or [`Action::Stop`]).
pub fn supervise(
    mut start: impl FnMut(&[String]) -> std::io::Result<Option<i32>>,
    cache: &Path,
    mut say: impl FnMut(String),
    limit_s: u64,
    mut policy: Policy,
) -> std::io::Result<(Option<i32>, Decision)> {
    let (journal_path, asked_path) = (cache.join(JOURNAL), cache.join(ASKED));
    let pending_path = cache.join(format!("{JOURNAL}.pending"));
    let clear = || {
        for p in [&journal_path, &pending_path, &asked_path] {
            let _ = std::fs::remove_file(p);
        }
    };
    clear();
    let mut extra = vec!["--prepare-journal".to_string(), journal_path.to_string_lossy().into_owned()];
    if limit_s > 0 {
        extra.extend(["--prepare-entry-limit".to_string(), limit_s.to_string()]);
    }
    let (mut more, mut runs) = (Vec::<String>::new(), 0);
    let (code, decision, mut journal) = loop {
        let args: Vec<String> = extra.iter().chain(&more).cloned().collect();
        let mut code = match start(&args) {
            Ok(c) => c,
            Err(e) => {
                clear();
                return Err(e);
            }
        };
        let journal = Journal::parse(&read(&journal_path));
        if code == Some(EXIT_JOURNAL_ERROR) || pending(&pending_path) || journal.incomplete {
            if code == Some(0) {
                code = Some(EXIT_JOURNAL_ERROR);
            }
            say("shader preparation: the journal is incomplete; no program can be attributed to this run".into());
            let d = Decision::stop("journal", Some("the shader preparation journal is incomplete".into()));
            break (code, d, journal);
        }
        // (a run that never opened the journal (it ended before the renderer started, or the file could not be
        // written) left nothing open: what the file's last run left open is the run before's, and already dealt with)
        let wrote = journal.runs > runs;
        runs = journal.runs;
        let (opened, timed) = if wrote { (journal.open_entries(), journal.timed()) } else { (Vec::new(), None) };
        let decision = policy.after(code, &opened, timed.as_ref());
        if let Some(c) = &decision.culprit {
            let timed_out = decision.how == Some("timed out");
            let listed = !timed_out && faults_append(cache, &c.identity);
            say(format!(
                "shader preparation: the GPU driver {} {}: that program is left out{}",
                if timed_out { "did not return from compiling" } else { "ended the game while compiling" },
                describe(c),
                if timed_out {
                    " for this preparation only".to_string()
                } else if listed {
                    " on this driver".to_string()
                } else {
                    format!(" for this preparation ({FAULTS} could not be written, so the game will meet it again)")
                }
            ));
        }
        if decision.action != Action::Restart {
            break (code, decision, journal);
        }
        if decision.culprit.is_none() {
            say(if policy.suspects.is_empty() {
                "shader preparation: the game ended with nothing inside the GPU driver; started again once".to_string()
            } else {
                format!(
                    "shader preparation: the game ended with {} programs inside the GPU driver; started again with \
                     those built one at a time, to find which it was",
                    policy.suspects.len()
                )
            });
        }
        more.clear();
        if journal.last_seq > 0 {
            // (numbers go on, and the run before is not counted against the steps: vk_draw.cpp prepare_restart_forgive)
            more.extend(["--prepare-from".to_string(), journal.last_seq.to_string()]);
        }
        if (!policy.culprits.is_empty() || !policy.suspects.is_empty())
            && write(&asked_path, &asked_text(&policy.culprit_keys(), &policy.suspects))
        {
            more.extend(["--prepare-skip".to_string(), asked_path.to_string_lossy().into_owned()]);
        }
    };
    if journal.runs > 0 {
        // (a run that kept a journal: its result list beside the caches, and the working files gone)
        let ended = match decision.action {
            Action::Done => "prepared".to_string(),
            _ => format!("given up ({})", decision.why.as_deref().or(decision.reason).unwrap_or("unknown")),
        };
        write(&cache.join(RESULTS), &results_text(&mut journal, &policy.culprits, &ended, 20));
        if !policy.culprits.is_empty() {
            let n = policy.culprits.len();
            say(format!(
                "shader preparation: {n} program{} left out during this preparation ({})",
                if n == 1 { "" } else { "s" },
                cache.join(RESULTS).display()
            ));
        }
    }
    clear();
    Ok((code, decision))
}

#[cfg(test)]
mod tests {
    //! The supervisor against a stand-in game: no game, no GPU. The stand-in prepares a list of programs a few at a
    //! time, as the game's threads do, and writes the journal the game writes (gpu/vk/prepare_journal.h). Some of its
    //! programs kill it, or never return, when they are handed to the "driver"; it reads the arguments the supervisor
    //! gives a run started again (--prepare-from, --prepare-skip) and follows them, as the game does.
    use super::*;
    use std::collections::{BTreeMap, HashSet};
    use std::io::Write;
    use std::path::PathBuf;

    const DRIVER: &str = "1002 00800161";

    fn line(key: u64) -> String {
        format!("{DRIVER} {key:016x}")
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Bad {
        Crash,
        Hang,
    }

    /// A stand-in for `longshot --prepare-shaders`. work: its programs' keys, in order. bad: what each bad one does.
    /// threads: how many are inside the driver at once. blind: how many starts die with nothing open. obey: whether it
    /// leaves out what --prepare-skip names; serial: whether it builds that file's suspects one at a time. own: a
    /// shader of the renderer's own (key 0) is open when it dies. mute: starts after this many never open the journal.
    #[derive(Default)]
    struct Game {
        work: Vec<u64>,
        bad: BTreeMap<u64, Bad>,
        threads: usize,
        blind: usize,
        disobey: bool,
        parallel: bool,
        own: bool,
        mute: Option<usize>,
        starts: Vec<BTreeMap<String, String>>,
        asked: Vec<Option<String>>,
    }

    impl Game {
        fn new(work: impl IntoIterator<Item = u64>, bad: &[(u64, Bad)]) -> Game {
            Game { work: work.into_iter().collect(), bad: bad.iter().copied().collect(), threads: 1, ..Game::default() }
        }

        fn start(&mut self, extra: &[String]) -> Option<i32> {
            let args: BTreeMap<String, String> =
                extra.chunks(2).map(|p| (p[0].clone(), p.get(1).cloned().unwrap_or_default())).collect();
            self.starts.push(args.clone());
            if self.mute.is_some_and(|m| self.starts.len() > m) {
                self.asked.push(None);
                return Some(-11);
            }
            let (mut skip, mut alone) = (HashSet::new(), HashSet::new());
            match args.get("--prepare-skip") {
                Some(p) => {
                    let text = std::fs::read_to_string(p).unwrap();
                    for l in text.lines() {
                        let (word, key) = l.split_once(' ').unwrap();
                        let key = u64::from_str_radix(key, 16).unwrap();
                        if word == "skip" {
                            skip.insert(key)
                        } else {
                            alone.insert(key)
                        };
                    }
                    self.asked.push(Some(text));
                }
                None => self.asked.push(None),
            }
            let mut seq: u64 = args.get("--prepare-from").map_or(0, |s| s.parse().unwrap());
            let mut lines = vec![format!("run {DRIVER} {seq:x}")];
            let journal = PathBuf::from(&args["--prepare-journal"]);
            let out = |lines: &[String], code: i32| {
                let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&journal).unwrap();
                for l in lines {
                    writeln!(f, "{l}").unwrap();
                }
                Some(code)
            };
            if self.blind > 0 {
                self.blind -= 1;
                return out(&lines, 1);
            }
            if self.own {
                seq += 1;
                lines.push(format!("b {seq:x} module {DRIVER} {:016x} {:016x}", 0, 64));
                return out(&lines, -11);
            }
            let mut todo: Vec<u64> = self.work.iter().copied().filter(|k| self.disobey || !skip.contains(k)).collect();
            while !todo.is_empty() {
                let mut batch = Vec::new();
                while !todo.is_empty() && batch.len() < self.threads {
                    if !self.parallel && !alone.is_empty() && !batch.is_empty() {
                        break; // (all calls are isolated during diagnosis)
                    }
                    batch.push(todo.remove(0));
                }
                let mut numbered = Vec::new();
                for k in batch {
                    seq += 1;
                    numbered.push((seq, k));
                    lines.push(format!("b {seq:x} graphics {DRIVER} {k:016x} {:016x}", 8));
                }
                for &(n, k) in &numbered {
                    match self.bad.get(&k) {
                        Some(Bad::Crash) => return out(&lines, -11),
                        Some(Bad::Hang) => {
                            let limit = args.get("--prepare-entry-limit").cloned().unwrap_or_else(|| "0".into());
                            lines.push(format!("t {n:x} {limit}"));
                            return out(&lines, EXIT_ENTRY_LIMIT);
                        }
                        None => {}
                    }
                }
                for (n, _) in numbered {
                    lines.push(format!("e {n:x} 0 5"));
                }
            }
            out(&lines, 0)
        }
    }

    struct Prepared {
        code: Option<i32>,
        d: Decision,
        said: Vec<String>,
        faults: Vec<String>,
        results: String,
        left: Vec<String>,
    }

    /// One supervised preparation of the stand-in in a folder of its own.
    fn prepare(game: &mut Game, policy: Policy) -> Prepared {
        let tmp = crate::scratch(&format!("supervisor-{}", rand_name()));
        let cache = tmp.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let mut said = Vec::new();
        let (code, d) = supervise(|x| Ok(game.start(x)), &cache, |s| said.push(s), 240, policy).unwrap();
        let faults = std::fs::read_to_string(cache.join(FAULTS))
            .map(|t| t.lines().filter(|l| !l.starts_with('#')).map(String::from).collect())
            .unwrap_or_default();
        let results = std::fs::read_to_string(cache.join(RESULTS)).unwrap_or_default();
        let mut left: Vec<String> =
            std::fs::read_dir(&cache).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        let _ = std::fs::remove_dir_all(&tmp);
        Prepared { code, d, said, faults, results, left }
    }

    fn rand_name() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        N.fetch_add(1, Ordering::Relaxed).to_string()
    }

    #[test]
    fn a_shader_that_kills_the_run_or_never_returns_costs_that_shader() {
        for (bad, state, verb) in
            [(Bad::Crash, "crashed", "ended the game"), (Bad::Hang, "timed-out", "did not return")]
        {
            let mut game = Game::new(1..=40, &[(17, bad)]);
            let p = prepare(&mut game, Policy::default());
            assert!(p.code == Some(0) && p.d.action == Action::Done, "{state}: {:?} {:?}", p.code, p.d);
            assert_eq!(game.starts.len(), 2, "{state}: the one that died and the one that finished");
            assert_eq!(p.faults, if bad == Bad::Crash { vec![line(17)] } else { vec![] }, "{state}");
            let first = &game.starts[0];
            assert_eq!(first.get("--prepare-entry-limit").map(String::as_str), Some("240"));
            assert!(first.contains_key("--prepare-journal"));
            assert!(!first.contains_key("--prepare-from") && !first.contains_key("--prepare-skip"));
            assert_eq!(game.starts[1].get("--prepare-from").map(String::as_str), Some("17"), "{state}");
            assert_eq!(game.asked[1].as_deref(), Some(format!("skip {:016x}\n", 17).as_str()), "{state}");
            let r = &p.results;
            assert!(r.contains(&format!("{state} graphics {} 0000000000000008 ", line(17))), "{state}:\n{r}");
            assert!(
                r.contains(&format!("{state} 1\n"))
                    && r.contains("built 55\n")
                    && r.contains("ended prepared\n")
                    && r.contains("runs 2\n"),
                "{state}: the totals:\n{r}"
            );
            // (the culprit is counted and listed once, as what it was: not as an entry merely interrupted as well)
            assert!(
                r.contains("interrupted 0\n") && !r.contains(&format!("interrupted graphics {} ", line(17))),
                "{r}"
            );
            assert!(p.said.iter().any(|s| s.contains(verb) && s.contains(&format!("{:016x}", 17))), "{:?}", p.said);
            assert!(p.said.iter().any(|s| s.contains("1 program left out")), "{:?}", p.said);
            let want: Vec<&str> = if bad == Bad::Crash { vec![FAULTS, RESULTS] } else { vec![RESULTS] };
            assert_eq!(p.left, want, "{state}: the working files are left behind");
        }
    }

    #[test]
    fn several_threads_inside_the_driver_the_culprit_alone_is_left_out() {
        let mut game = Game { threads: 4, ..Game::new(1..=40, &[(18, Bad::Crash)]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.code == Some(0) && p.d.action == Action::Done && game.starts.len() == 3);
        assert_eq!(p.faults, vec![line(18)], "the neighbours are not blamed");
        let suspects: String = [17u64, 18, 19, 20].iter().map(|k| format!("alone {k:016x}\n")).collect();
        assert_eq!(game.asked[1].as_deref(), Some(suspects.as_str()));
        assert_eq!(game.asked[2].as_deref(), Some(format!("skip {:016x}\n", 18).as_str()));
        // (the first death's four open entries are only known to have been interrupted; the culprit's is the second's)
        assert!(p.results.contains("interrupted 4\n") && p.results.contains("crashed 1\n"), "{}", p.results);
        // (two culprits in one batch, and a hang beside a crash)
        let mut game = Game { threads: 8, ..Game::new(1..=40, &[(5, Bad::Crash), (6, Bad::Hang), (30, Bad::Crash)]) };
        let p = prepare(&mut game, Policy::default());
        let mut faults = p.faults.clone();
        faults.sort();
        assert!(p.code == Some(0) && p.d.action == Action::Done && faults == vec![line(5), line(30)], "{faults:?}");
        assert!(p.results.contains("crashed 2\n") && p.results.contains("timed-out 1\n"), "{}", p.results);
    }

    #[test]
    fn a_death_with_nothing_open_is_tried_once_more_and_then_given_up() {
        let mut game = Game { blind: 1, ..Game::new(1..=10, &[]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.code == Some(0) && p.d.action == Action::Done && game.starts.len() == 2 && p.faults.is_empty());
        assert!(!game.starts[1].contains_key("--prepare-skip"), "a death with nothing open leaves something out");
        let mut game = Game { blind: 5, ..Game::new(1..=10, &[]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.code == Some(1) && p.d.action == Action::Stop && p.d.reason == Some("died") && p.d.why.is_none());
        assert!(game.starts.len() == 2 && p.faults.is_empty());
        assert!(p.results.contains("given up (died)"), "{}", p.results);
        // (one such death, then a culprit: started again each time, the culprit kept)
        let mut game = Game { blind: 1, ..Game::new(1..=10, &[(4, Bad::Crash)]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.code == Some(0) && game.starts.len() == 3 && p.faults == vec![line(4)]);
    }

    #[test]
    fn the_bounds() {
        let all_crash: Vec<(u64, Bad)> = (1..=40).map(|k| (k, Bad::Crash)).collect();
        let mut game = Game::new(1..=40, &all_crash);
        let p = prepare(&mut game, Policy::default());
        assert!(p.d.action == Action::Stop && p.d.reason == Some("culprits"));
        assert!(p.d.why.as_deref().unwrap_or("").contains(&format!("more than {MAX_CULPRITS} shaders")));
        assert_eq!(game.starts.len(), MAX_CULPRITS + 1);
        assert_eq!(p.faults, (1..=MAX_CULPRITS as u64 + 1).map(line).collect::<Vec<_>>(), "what was found stays");
        let all_hang: Vec<(u64, Bad)> = (1..=40).map(|k| (k, Bad::Hang)).collect();
        let mut game = Game { threads: 4, ..Game::new(1..=40, &all_hang) };
        let p = prepare(&mut game, Policy::new(3, MAX_STARTS, MAX_BLIND));
        assert!(p.d.reason == Some("culprits") && p.faults.is_empty() && p.code == Some(EXIT_ENTRY_LIMIT));
        // (every start ends with several open and the game never builds them one at a time: the starts' bound)
        let mut game = Game { threads: 4, parallel: true, ..Game::new(1..=40, &all_crash) };
        let p = prepare(&mut game, Policy::new(MAX_CULPRITS, 6, MAX_BLIND));
        assert!(p.d.reason == Some("starts") && game.starts.len() == 6 && p.faults.is_empty());
    }

    #[test]
    fn what_is_not_retried() {
        for (code, reason) in [(Some(EXIT_NO_VULKAN), "vulkan"), (Some(EXIT_STALLED), "stalled"), (None, "silent")] {
            let tmp = crate::scratch(&format!("supervisor-final-{reason}"));
            let mut calls = 0;
            let (got, d) = supervise(
                |_| {
                    calls += 1;
                    Ok(code)
                },
                &tmp,
                |_| {},
                240,
                Policy::default(),
            )
            .unwrap();
            assert!(
                got == code && d.action == Action::Stop && d.reason == Some(reason) && calls == 1 && d.why.is_none()
            );
            assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0, "files written for a run that kept no journal");
            let _ = std::fs::remove_dir_all(&tmp);
        }
        // (a folder that is not there: a game folder that cannot be written)
        let (got, d) = supervise(
            |_| Ok(Some(0)),
            Path::new("no-such-folder/cache"),
            |s| panic!("said {s}"),
            240,
            Policy::default(),
        )
        .unwrap();
        assert!(got == Some(0) && d.action == Action::Done);
        let mut game = Game { own: true, ..Game::new([1, 2, 3], &[]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.d.reason == Some("own") && game.starts.len() == 1 && p.faults.is_empty());
        assert!(p.d.why.as_deref().unwrap_or("").contains("renderer's own"));
        let mut game = Game { disobey: true, ..Game::new([1, 2, 3], &[(2, Bad::Crash)]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.d.reason == Some("repeated") && game.starts.len() == 2 && p.faults == vec![line(2)]);
        let mut game = Game { mute: Some(1), ..Game::new([1, 2, 3], &[(2, Bad::Crash)]) };
        let p = prepare(&mut game, Policy::default());
        assert!(p.d.reason == Some("died") && game.starts.len() == 3 && p.faults == vec![line(2)], "{:?}", p.d);
        // (the game could not start: said to the caller, nothing left behind)
        let tmp = crate::scratch("supervisor-nostart");
        let e = supervise(|_| Err(std::io::Error::other("no such file")), &tmp, |_| {}, 240, Policy::default());
        assert!(e.is_err() && std::fs::read_dir(&tmp).unwrap().count() == 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn an_incomplete_journal_names_no_program() {
        for mode in ["pending", "torn", "exit"] {
            let tmp = crate::scratch(&format!("supervisor-incomplete-{mode}"));
            let (_, d) = supervise(
                |extra| {
                    let path = PathBuf::from(&extra[extra.iter().position(|a| a == "--prepare-journal").unwrap() + 1]);
                    let torn = if mode == "torn" { "e 1 0" } else { "" };
                    std::fs::write(
                        &path,
                        format!("run {DRIVER} 0\nb 1 graphics {DRIVER} {:016x} {:016x}\n{torn}", 17, 8),
                    )
                    .unwrap();
                    if mode == "pending" {
                        std::fs::write(format!("{}.pending", path.display()), "").unwrap();
                    }
                    Ok(Some(if mode == "exit" { EXIT_JOURNAL_ERROR } else { -11 }))
                },
                &tmp,
                |_| {},
                240,
                Policy::default(),
            )
            .unwrap();
            assert!(d.reason == Some("journal") && !tmp.join(FAULTS).exists(), "{mode}");
            assert!(!tmp.join(format!("{JOURNAL}.pending")).exists(), "{mode}: marker left behind");
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }

    #[test]
    fn the_policy() {
        let entry = |key: u64, seq: u64| Entry {
            run: 1,
            seq,
            kind: "graphics".into(),
            identity: line(key),
            key,
            detail: "0".repeat(16),
            result: None,
            ms: 0,
            timed: false,
            state: None,
        };
        assert_eq!(Policy::default().after(Some(0), &[], None).action, Action::Done);
        let mut p = Policy::default();
        let d = p.after(None, &[entry(7, 1)], None); // (stopped for silence with one entry inside the driver)
        assert!(d.action == Action::Restart && d.culprit.map(|e| e.key) == Some(7) && d.how == Some("timed out"));
        let mut p = Policy::default();
        let d = p.after(Some(-11), &[entry(1, 1), entry(2, 1), entry(1, 3)], None);
        assert!(d.action == Action::Restart && d.culprit.is_none() && p.suspects == [1, 2], "two programs open");
        let d = p.after(Some(-11), &[entry(9, 1), entry(2, 1)], None);
        assert!(d.culprit.is_none() && p.suspects == [9, 2], "an unrelated call beside a suspect is not attributed");
        let d = p.after(Some(-11), &[entry(0, 1), entry(2, 1)], None);
        assert!(d.culprit.is_none() && p.suspects == [2], "a renderer call beside a suspect is not attributed");
        let mut p = Policy::default();
        p.after(Some(-11), &[entry(1, 1), entry(2, 1)], None);
        let d = p.after(Some(-11), &[entry(8, 1), entry(9, 1)], None);
        assert!(d.culprit.is_none() && p.suspects == [8, 9], "a death among other programs");
        let mut p = Policy::default();
        let d = p.after(Some(EXIT_ENTRY_LIMIT), &[entry(1, 1), entry(2, 2), entry(3, 3)], Some(&entry(2, 2)));
        assert!(d.culprit.map(|e| e.key) == Some(2) && d.how == Some("timed out") && p.culprits.len() == 1);
        let d = p.after(Some(EXIT_ENTRY_LIMIT), &[entry(5, 1)], None); // (exit 6 with no "t" line: what is open)
        assert_eq!(d.culprit.map(|e| e.key), Some(5));
        assert_eq!(asked_text(&[1, 2], &[3]), format!("skip {:016x}\nskip {:016x}\nalone {:016x}\n", 1, 2, 3));
        let env = |v: Option<&'static str>| move |_: &str| v.map(String::from);
        assert_eq!(entry_limit(env(None)), ENTRY_LIMIT_S);
        assert_eq!(entry_limit(env(Some("30"))), 30);
        assert_eq!(entry_limit(env(Some("x"))), ENTRY_LIMIT_S);
        assert_eq!(entry_limit(env(Some("-4"))), 0);
        const { assert!(ENTRY_LIMIT_S < 300, "an entry's limit is below the whole run's 300 s with nothing moving") };
    }

    #[test]
    fn the_journal_and_the_faults_list_are_read_as_the_game_writes_them() {
        let j = Journal::parse(&format!(
            "noise before a run\nrun {DRIVER} 0\nb 1 compute {DRIVER} 00000000000000aa 0000000000000001\n\
             b 2 graphics {DRIVER} 00000000000000bb 0000000000000002\ne 1 -77 12\nt 2 30\nB 3 nonsense\n\
             e 9 0 1\nrun {DRIVER} 2\nb 3 module {DRIVER} 00000000000000CC 0000000000000003\n"
        ));
        assert!(!j.incomplete && j.runs == 2 && j.last_seq == 2 && j.entries.len() == 2, "{j:?}");
        assert_eq!((j.entries[0].result, j.entries[0].ms), (Some(DRIVER_FAULT), 12));
        assert!(j.entries[1].timed && j.entries[1].ms == 30_000 && j.entries[1].result.is_none());
        assert!(j.open_entries().is_empty() && j.timed().is_none(), "the last run is the second, which opened none");
        assert!(Journal::parse("run 1002 00800161 0\nb 1").incomplete);
        let added = faults_with("", "10de 0a0b0c0d 00000000000000aa").unwrap();
        assert!(added.starts_with(FAULTS_HEAD) && added.ends_with("\n10de 0a0b0c0d 00000000000000aa\n"));
        assert_eq!(faults_with(&added, "10de 0a0b0c0d 00000000000000aa"), None);
        assert_eq!(faults_with("x", "y").as_deref(), Some("x\ny\n"));
    }

    /// The journal gpu/vk/prepare_journal.h writes, as tests/test_prepare_journal.py's fixture of the real header writes
    /// it in three runs (one that dies inside an entry, one that names an entry past its limit, one that gets through):
    /// that check compares the header's output with this file, and this reads the same file.
    const WRITTEN: &str = include_str!("../tests/data/prepare_journal.txt");

    #[test]
    fn the_journal_the_game_writes_is_read() {
        let runs: Vec<usize> = WRITTEN.match_indices("run ").map(|(i, _)| i).collect();
        let first = Journal::parse(&WRITTEN[..runs[1]]);
        assert!(first.runs == 1 && first.entries.len() == 4 && first.last_seq == 4 && first.driver == "1002 00800161");
        let opened = first.open_entries();
        assert!(opened.len() == 1 && opened[0].seq == 2 && opened[0].kind == "graphics" && opened[0].key == 0xaaaa);
        assert!(opened[0].identity == "1002 00800161 000000000000aaaa" && opened[0].detail == "0000000000000008");
        assert!(first.timed().is_none(), "a run that died named no entry as past its limit");
        let by = |j: &Journal, seq: u64| j.entries.iter().find(|e| e.seq == seq).cloned().unwrap();
        let e1 = by(&first, 1);
        assert!(e1.result == Some(0) && e1.ms == 7 && e1.kind == "module" && e1.key == 0x11, "an entry that returned");
        let e3 = by(&first, 3);
        assert!(e3.result == Some(DRIVER_FAULT) && e3.ms == 489 && entry_state(&e3) == "faulted", "a caught fault");
        assert!(by(&first, 4).key == 0 && entry_state(&by(&first, 4)) == "built", "a shader of the renderer's own");
        let second = Journal::parse(&WRITTEN[..runs[2]]);
        assert!(second.runs == 2 && second.entries.len() == 8 && second.last_seq == 8, "the second run numbers on");
        assert_eq!(second.open_entries().iter().map(|e| e.seq).collect::<Vec<_>>(), [6], "its own open entry only");
        let timed = second.timed().unwrap();
        assert!(timed.seq == 6 && timed.ms == 6000 && timed.key == 0xaaaa, "the entry past its limit");
        let whole = Journal::parse(WRITTEN);
        assert!(whole.runs == 3 && whole.entries.len() == 12 && whole.last_seq == 12);
        assert!(
            whole.open_entries().is_empty() && whole.timed().is_none(),
            "a run that got through leaves nothing open"
        );
        // (lines cut short, and lines that are not the journal's)
        for torn in ["b d graphics 1002 0080", "e", "b d graphics 1002 00800161 000000000000eeee 0000000000000000"] {
            let t = Journal::parse(&format!("{WRITTEN}{torn}"));
            assert!(t.entries.len() == 12 && t.open_entries().is_empty() && t.incomplete, "{torn:?}");
        }
        let crlf = WRITTEN[..runs[2]].replacen('\n', "\r\n", 3);
        let noisy = Journal::parse(&format!(
            "junk\nb 1 graphics 1002 00800161 0000000000000001 0000000000000000\n{crlf}e 99 0 1\nt 99 3\nb zz\n\n\
             b 9 graphics 1002 00800161 0000000000000009 0000000000000000 more\n"
        ));
        assert!(noisy.runs == 2 && noisy.entries.len() == 8, "foreign lines change what is read: {noisy:?}");
        assert_eq!(noisy.open_entries().iter().map(|e| e.seq).collect::<Vec<_>>(), [6]);
        assert!(Journal::parse("").runs == 0 && Journal::parse("run 1002 00800161 0").runs == 0, "an empty journal");
        let cut = &WRITTEN[..WRITTEN.find("\nt ").unwrap() + 1];
        let half = Journal::parse(&cut[..cut.len() - 4]); // (the last whole line cut inside)
        assert!(half.runs == 2 && half.entries.iter().all(|e| e.seq != 8 || e.result.is_none()), "an end cut short");
        // (the result list)
        let mut j = Journal::parse(WRITTEN);
        let culprit = j.entries[1].clone();
        let listed = results_text(&mut j, &[(culprit, "crashed")], "prepared", 20);
        for want in ["driver 1002 00800161\n", "runs 3\n", "ended prepared\n", "entries 12\n"] {
            assert!(listed.contains(want), "{want:?} in:\n{listed}");
        }
        assert!(listed.contains("crashed graphics 1002 00800161 000000000000aaaa 0000000000000008 0\n"), "{listed}");
        // (built: each run's own shader and its module, and the last run's graphics pipeline; the caught fault three
        // times; the limit run's open entry was only interrupted as far as this list knows)
        for want in ["built 7\n", "faulted 3\n", "crashed 1\n", "interrupted 1\n"] {
            assert!(listed.contains(want), "{want:?} in:\n{listed}");
        }
    }
}
