//! After the game: whether it crashed, the crash window's decisions and its reports. The builder starts the game and
//! waits for it (`longshot-builder play`), so this runs in the builder, not in a helper of the game's
//! (docs/dev/reference/crash-window.md). The game still writes its crash records (host/hos_crash_record.c) and its
//! in-game Bug report; started without the builder it writes records only, and the builder shows one at its next
//! start ([`at_start`]).
//!
//! - [`after_run`]: once the game has ended, its record (a marker it left made into an unclean record), its log, the
//!   verdict, when and why: a [`Shown`] to show, or None to say nothing (a normal exit or a quit).
//! - [`at_start`]: a record of an earlier run nobody showed (a day old at most).
//! - [`flow::show`]: the window's pages, through a [`flow::Pages`] (the `ui` crate's window); [`Shown::text`] when
//!   there is no window.
//! - [`mark_shown`]: once a window showed it, or the player was told (bug-reports/shown.txt).
//!
//! What decides is ported from host/hos_crash_dialog.h, rule for rule; the redaction from
//! gpu/vk/compat/longshot_redact.h ([`redact`], whose tests are the C's answers on the same inputs).

pub mod build_report;
pub mod clean;
pub mod desk;
pub mod diag;
pub mod facts;
pub mod flow;
pub mod names;
pub mod oom;
pub mod post;
pub mod redact;
pub mod report;
pub mod store;

pub use facts::{Cause, Exit, Stage, Verdict};
pub use flow::{Choice, Desk, Page, Pages};
pub use report::Shown;
pub use store::Paths;

use facts::{Kind, LogFacts, Record};
use redact::Who;

/// The game's run as the builder saw it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Run {
    /// Its process.
    pub pid: u32,
    /// When it was started, seconds since 1970.
    pub started: i64,
    /// How it ended.
    pub exit: Exit,
    /// The out-of-memory killer took it ([`oom::Watch::oom`]).
    pub oom: bool,
}

fn address(paths: &Paths) -> Option<String> {
    store::read_text(&paths.game.join("longshot_version.txt"), 16 << 10)
        .and_then(|s| names::stamp_address(&s, "crash_reports"))
}

fn shown(
    paths: &Paths,
    verdict: Verdict,
    record: Option<(String, String)>,
    log: store::RunLog,
    exit: Exit,
    who: Who,
) -> Shown {
    let r = Record::parse(record.as_ref().map_or("", |(_, t)| t));
    let stage = if log.found() { log.facts.stage() } else { Stage::Unknown };
    let cause = facts::cause(verdict, &r, &log.facts, exit);
    let (record_name, record) = record.map_or((None, None), |(n, t)| (Some(n), Some(t)));
    Shown { verdict, stage, cause, record, record_name, log, paths: paths.clone(), who, address: address(paths) }
}

/// The game's run has ended: what to show, None to say nothing. A record nobody needs to see (the run said nothing)
/// is listed as shown.
pub fn after_run(paths: &Paths, run: &Run) -> Option<Shown> {
    let who = Who::here();
    // (its marker, if it left one, made into its record first; a run id read from it while it is there)
    let marked = store::marker(paths, run.pid);
    store::recover(paths, run.pid, run.oom, &who);
    let record = store::run_record(paths, run.pid, run.started);
    let run_id = record
        .as_ref()
        .and_then(|(_, t)| facts::record_value(t, "run_id"))
        .or_else(|| marked.as_ref().map(|(id, _)| id.clone()));
    let log_path = marked.and_then(|(_, l)| l).or_else(|| paths.log.clone());
    let mut log = store::RunLog::find(log_path.as_deref(), run_id.as_deref(), &who);
    // (no run id: the log as it is, only when this run wrote it)
    if run_id.is_none()
        && let Some(p) = &log.path
    {
        let since = std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok());
        if since.is_none_or(|s| (s.as_secs() as i64) + 2 < run.started) {
            log = store::RunLog::default();
        }
    }
    let r = Record::parse(record.as_ref().map_or("", |(_, t)| t));
    let verdict = facts::verdict(&r, &log.facts, run.exit, false);
    if verdict == Verdict::Quiet {
        if let Some((name, _)) = &record {
            store::ledger_add(paths, Some(name), false);
        }
        return None;
    }
    Some(shown(paths, verdict, record, log, run.exit, who))
}

/// At the builder's start: the newest record of the last day nobody showed (its run had no builder, or no window
/// could open), a crash record or an out-of-memory end; None for none.
pub fn at_start(paths: &Paths) -> Option<Shown> {
    let name = store::unshown(paths, store::now())?;
    let text = store::read_record(paths, &name)?;
    let who = Who::here();
    let r = Record::parse(&text);
    let log = store::RunLog::find(paths.log.as_deref(), (!r.run_id.is_empty()).then_some(r.run_id.as_str()), &who);
    let verdict = facts::verdict(&r, &log.facts, Exit::default(), false);
    if verdict == Verdict::Quiet || (r.kind != Kind::Crash && !r.oom) {
        store::ledger_add(paths, Some(&name), false);
        return None;
    }
    Some(shown(paths, verdict, Some((name, text)), log, Exit::default(), who))
}

/// Listed as shown (bug-reports/shown.txt): not shown again at a start. Once a window showed it, or the player was told
/// in text by their own choice of output; never when a window was wanted and none could open.
pub fn mark_shown(s: &Shown) {
    if let Some(n) = &s.record_name {
        store::ledger_add(&s.paths, Some(n), false);
    }
}

impl Shown {
    /// The record's path, when there is one.
    pub fn record_path(&self) -> Option<std::path::PathBuf> {
        self.record_name.as_ref().map(|n| self.paths.pending().join(n))
    }

    /// What happened as text, for a terminal or a log: the window's sentence and why, and where the record and the log
    /// are.
    pub fn text(&self) -> String {
        let mut t = format!("{}. {}", facts::headline(facts::title(self.verdict), self.stage), self.cause.text());
        if let Some(p) = self.record_path() {
            t.push_str(&format!("\nThe crash record: {}", p.display()));
        }
        if let Some(p) = &self.log.path {
            t.push_str(&format!("\nThe log: {}", p.display()));
        }
        t
    }

    /// The facts its log gave.
    pub fn facts(&self) -> &LogFacts {
        &self.log.facts
    }
}
