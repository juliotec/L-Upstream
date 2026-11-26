//! `longshot-builder self-update [--channel stable|nightly] [--output json|text|user]`: a portable or standalone copy
//! (RecompLauncher's zip, a version folder of its own) updates itself from its channel (docs/dev/reference/build.md,
//! "self-update"). A launcher's versions never do: the launcher fetches each version itself (`fetch`) and starts play
//! with `--no-self-update`.
//!
//! - The channel: `--channel`, else the one this program's version is on. Only a newer version of the same channel is
//!   taken (a channel asked that is not its own: that channel's newest).
//! - The new version is fetched (fetch_cmd.rs: a delta from this program when the release has one) into
//!   `.self-update/` beside this program, started once with `--version` (it must say its version), then put in this
//!   program's place (release::swap: on Windows the running program renamed aside, deleted at the next start).
//! - Asked once, under `--output user` (the builder's own window): the answer is kept in [`SETTINGS`] beside the
//!   program. Without one, `self-update` run by hand is its own yes; play's daily check updates only on a yes.
//!
//! Play checks at most once a day ([`in_background`]), on a thread of its own: never before the game starts (only the
//! one question, once, at the start after a check found an update), and the new version is used from the next start.

use build_core::Error;
use build_core::report::{Event, Format, Reporter};
use crash::flow::Page;
use release::forge::Listed;
use release::{Cancel, Channel};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The settings file beside the program: the player's answer, the last check, the version a check offered.
pub const SETTINGS: &str = "longshot_self_update.json";
/// What a launcher passes to play: no self-update for a version it manages.
pub const NO_SELF_UPDATE: &str = "--no-self-update";
/// The variable that turns play's check off.
pub const OFF_VARIABLE: &str = "LONGSHOT_NO_SELF_UPDATE";
/// How often play checks.
pub const EVERY: u64 = 24 * 3600;

/// [`SETTINGS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// The player's answer: update by itself (true) or never (false); None until asked.
    pub answer: Option<bool>,
    /// When play last checked (Unix seconds).
    pub last_check: u64,
    /// The version the last check found and nobody was asked about yet.
    pub offered: Option<String>,
}

impl Settings {
    /// The program's folder `dir`'s settings (the defaults when it has none that read).
    #[must_use]
    pub fn read(dir: &Path) -> Settings {
        let v: Value = std::fs::read_to_string(dir.join(SETTINGS))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Settings {
            answer: v.get("self_update").and_then(Value::as_str).and_then(|a| match a {
                "yes" => Some(true),
                "no" => Some(false),
                _ => None,
            }),
            last_check: v.get("last_check").and_then(Value::as_u64).unwrap_or(0),
            offered: v.get("offered").and_then(Value::as_str).map(str::to_string),
        }
    }

    /// Written beside the program, whole.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let answer = self.answer.map(|a| if a { "yes" } else { "no" });
        let text = json!({"v": 1, "self_update": answer, "last_check": self.last_check, "offered": self.offered});
        let part = dir.join(format!("{SETTINGS}.part"));
        std::fs::write(&part, format!("{text:#}\n"))?;
        std::fs::rename(part, dir.join(SETTINGS))
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether play's check is due: never checked, or a day since (a clock set back counts as due).
#[must_use]
pub fn due(s: &Settings, now: u64) -> bool {
    now < s.last_check || now - s.last_check >= EVERY
}

/// The version to update `own` to on `channel` from the release list: a newer one of the same channel, or, on another
/// channel, its newest; None when there is none.
#[must_use]
pub fn newer_on(own: &str, channel: Channel, list: &[Listed]) -> Option<String> {
    let newest = release::forge::on_channel(list, channel).first()?.tag.clone();
    if Channel::of(own) == Some(channel) {
        release::versions::newer(&newest, own).then_some(newest)
    } else {
        (newest != own).then_some(newest)
    }
}

/// `--no-self-update` taken out of play's `args` (before a `--`): whether it was there.
pub fn take_flag(args: &mut Vec<String>) -> bool {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    match args[..end].iter().position(|a| a == NO_SELF_UPDATE) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

/// The player asked, in the builder's window: Some(yes), or None (no window, or closed: asked again later).
fn ask(own: &str, to: &str) -> Option<bool> {
    let text = format!(
        "Longshot {to} is out (this copy is {own}). Should this copy of Longshot update itself when a new version \
         comes out? It downloads in the background and is used from the next start. You are asked once."
    );
    let page = Page { title: "Longshot: updates", text: &text, block: None, buttons: &["Update by itself", "Never"] };
    match crate::after::ui().page(&page) {
        Ok(Some(0)) => Some(true),
        Ok(Some(_)) => Some(false),
        _ => None,
    }
}

/// The releases listed and the version to update to, through curl.
fn check(own: &str, channel: Channel, cancel: &Cancel) -> Result<Option<String>, Error> {
    let curl = release::curl::Curl::new(cancel.clone()).map_err(|t| Error::player("release_unreachable", t))?;
    let get = |k: &str| build_driver::env(k);
    let edge = release::forge::Edge { inner: &curl, enabled: release::forge::Edge::enabled_by(&get), said: &|_| {} };
    let scratch = Path::new(".");
    let forge = release::forge::Forge::new(&edge, Vec::new(), cancel, scratch);
    let list = forge.list().map_err(crate::fetch_cmd::error_of)?;
    Ok(newer_on(own, channel, &list))
}

/// A program's `--version` answer names release `tag` (started once, at most 30 s): whether it runs on this PC.
fn self_check(program: &Path, tag: &str) -> bool {
    let mut c = build_core::child::command(program);
    c.arg("--version").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let Ok(out) = build_core::child::output(&mut c) else { return false };
    out.status.success()
        && String::from_utf8_lossy(&out.stdout).lines().any(|l| l.starts_with(&format!("longshot-builder {tag} ")))
}

/// The version fetched into `staged` (its folder: program and record) put in place of the program `exe`: its
/// subsystem made `exe`'s (Windows), swapped in, the record moved beside it naming `exe`'s file.
pub fn install_staged(exe: &Path, staged: &Path, windows: bool) -> Result<(), String> {
    let rec = release::install::Record::read(staged).ok_or("the fetched version has no record")?;
    let program = staged.join(&rec.program);
    if windows {
        let (mut new, cur) =
            (std::fs::read(&program).map_err(|e| e.to_string())?, std::fs::read(exe).map_err(|e| e.to_string())?);
        let mut probe = cur;
        if let Ok(was) = release::swap::set_subsystem(&mut probe, 3) {
            release::swap::set_subsystem(&mut new, was)?;
            std::fs::write(&program, new).map_err(|e| e.to_string())?;
        }
    }
    // (held while the program is swapped: play's end waits for it, so the process never ends between the renames)
    let _swapping = SWAPPING.lock().unwrap_or_else(|e| e.into_inner());
    if CLOSING.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("Longshot is closing: the update is done at the next start".into());
    }
    release::swap::swap(exe, &program, windows, &std::thread::sleep).map_err(|e| format!("{}: {e}", exe.display()))?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    let text = std::fs::read_to_string(staged.join(release::install::RECORD)).map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    v["program"] = exe.file_name().unwrap_or_default().to_string_lossy().into_owned().into();
    let part = dir.join(format!("{}.part", release::install::RECORD));
    std::fs::write(&part, format!("{v:#}\n"))
        .and_then(|()| std::fs::rename(&part, dir.join(release::install::RECORD)))
        .map_err(|e| e.to_string())
}

/// `to` fetched beside `exe` (a delta from `exe` when the release has one), checked by running it, and put in its
/// place.
fn update_to(exe: &Path, to: &str, r: &Reporter, cancel: &Cancel, show: bool) -> Result<(), Error> {
    let staged = crate::fetch_cmd::beside(exe);
    let a = crate::fetch_cmd::FetchArgs {
        want: to.to_string(),
        out: staged.clone(),
        from: vec![exe.to_path_buf()],
        platform: release::platform().to_string(),
    };
    let done = (|| {
        let f = crate::fetch_cmd::fetch(&a, r, cancel, show)?;
        r.send(crate::fetch_cmd::fetched_event(&f));
        if !self_check(&f.program, to) {
            return Err(Error::player(
                "self_update_failed",
                format!("Longshot {to} was downloaded but does not run on this PC: this copy stays as it is."),
            ));
        }
        install_staged(exe, &staged, cfg!(windows)).map_err(|why| {
            Error::player("self_update_failed", format!("Longshot {to} could not take this copy's place ({why})."))
        })
    })();
    let _ = std::fs::remove_dir_all(&staged);
    done
}

/// This run's program: the file it runs from.
fn me() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// `longshot-builder self-update ...` by the program carrying `own` (None: a developer's build): its exit status.
pub fn run(args: &[String], own: Option<&str>) -> ExitCode {
    let mut args = args.to_vec();
    let asked = match crate::output::take(&mut args) {
        Ok(a) => a,
        Err(why) => return crate::usage_error(why, false),
    };
    let json = asked == Some(crate::output::Output::Json);
    let mut channel = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.clone().next().and_then(|c| Channel::parse(c))) {
            ("--channel", Some(c)) => {
                channel = Some(c);
                it.next();
            }
            (other, _) => {
                return crate::usage_error(format!("self-update: {other:?} is not --channel stable|nightly"), json);
            }
        }
    }
    let o = crate::output::set(asked);
    let r = Reporter::stdout(o.format());
    let Some(own) = own else {
        r.send(Event::SelfUpdate { action: "not_a_release", version: "dev".into(), channel: String::new(), to: None });
        return ExitCode::SUCCESS;
    };
    let channel = channel.or_else(|| Channel::of(own)).unwrap_or(Channel::Stable);
    let event = |action, to: Option<&str>| Event::SelfUpdate {
        action,
        version: own.to_string(),
        channel: channel.name().into(),
        to: to.map(str::to_string),
    };
    let exe = me();
    let dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let cancel = Cancel::default();
    let failed = |e: Error| {
        r.send(Event::Error { code: e.code(), text: e.to_string() });
        if o.windows() {
            crate::progress::open().error(&e.to_string());
        }
        ExitCode::FAILURE
    };
    let to = match check(own, channel, &cancel) {
        Ok(Some(t)) => t,
        Ok(None) => {
            r.send(event("up_to_date", None));
            return ExitCode::SUCCESS;
        }
        Err(e) => return failed(e),
    };
    let mut s = Settings::read(&dir);
    if o.windows() && s.answer.is_none() {
        s.answer = ask(own, &to);
        let _ = s.write(&dir);
    }
    if o.windows() && s.answer != Some(true) {
        r.send(event(if s.answer == Some(false) { "declined" } else { "not_asked" }, Some(&to)));
        return ExitCode::SUCCESS;
    }
    match update_to(&exe, &to, &r, &cancel, true) {
        Ok(()) => {
            s.offered = None;
            let _ = s.write(&dir);
            r.send(event("updated", Some(&to)));
            ExitCode::SUCCESS
        }
        Err(e) => failed(e),
    }
}

/// Held while the program is swapped for the new one ([`release::swap::swap`]): on Windows the program is moved aside
/// first, then the new one put in (waiting out a scan that holds it, up to ten seconds).
static SWAPPING: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Set when the process is about to end: no swap starts after it.
static CLOSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Waits for a swap going on ([`in_background`]'s), and lets no other start, so the process does not end with no
/// program in place. A download or a check is not waited for: the next start does it again.
pub fn wait_swap() {
    let _held = SWAPPING.lock().unwrap_or_else(|e| e.into_inner());
    CLOSING.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Play's daily check, for the program at `exe` carrying `own`: `--no-self-update` taken out of `args` first (a
/// launcher's version: nothing). Asks the player once (under `--output user`) about a version an earlier check found;
/// then, at most once a day, checks on a thread of its own and, on the player's yes, updates this copy for the next
/// start. Never delays the game but for that one question.
pub fn in_background(exe: &Path, own: Option<&str>, args: &mut Vec<String>) {
    if take_flag(args) || build_driver::env(OFF_VARIABLE).is_some_and(|v| v.trim() == "1") {
        return;
    }
    let Some(own) = own.map(str::to_string) else { return };
    let (exe, dir) = (exe.to_path_buf(), exe.parent().unwrap_or(Path::new(".")).to_path_buf());
    let mut s = Settings::read(&dir);
    if let Some(to) = s.offered.clone().filter(|_| s.answer.is_none() && crate::output::chosen().windows()) {
        s.answer = ask(&own, &to);
        let _ = s.write(&dir);
    }
    if s.answer == Some(false) || !due(&s, now()) {
        return;
    }
    std::thread::spawn(move || {
        s.last_check = now();
        if s.write(&dir).is_err() {
            return; // (a folder it can't write: a launcher's or a read-only copy, never updated)
        }
        let channel = Channel::of(&own).unwrap_or(Channel::Stable);
        let r = Reporter::to(Format::Text, Box::new(std::io::stderr()));
        let Ok(Some(to)) = check(&own, channel, &Cancel::default()) else { return };
        if s.answer == Some(true) {
            match update_to(&exe, &to, &r, &Cancel::default(), false) {
                Ok(()) => eprintln!("longshot: updated to {to}, used from the next start"),
                Err(e) => eprintln!("longshot: the update to {to} failed: {e}"),
            }
        } else {
            s.offered = Some(to);
            let _ = s.write(&dir);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("self-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn listed(tags: &[&str]) -> Vec<Listed> {
        tags.iter().map(|t| Listed { tag: (*t).into(), draft: false, prerelease: false, assets: vec![] }).collect()
    }

    #[test]
    fn only_a_newer_version_of_the_channel_or_another_channel_s_newest() {
        let l = listed(&["nightly-12-aaaaaaaa", "nightly-10-bbbbbbbb", "v3.1.0", "v3.0.0"]);
        assert_eq!(newer_on("nightly-10-bbbbbbbb", Channel::Nightly, &l).as_deref(), Some("nightly-12-aaaaaaaa"));
        assert_eq!(newer_on("nightly-12-aaaaaaaa", Channel::Nightly, &l), None);
        assert_eq!(newer_on("nightly-13-cccccccc", Channel::Nightly, &l), None, "never older");
        assert_eq!(newer_on("v3.0.0", Channel::Stable, &l).as_deref(), Some("v3.1.0"));
        assert_eq!(newer_on("v3.1.0", Channel::Nightly, &l).as_deref(), Some("nightly-12-aaaaaaaa"), "switched");
        assert_eq!(newer_on("v3.1.0", Channel::Stable, &listed(&[])), None);
    }

    #[test]
    fn the_settings_and_the_daily_check() {
        let d = scratch("settings");
        assert_eq!(Settings::read(&d), Settings::default());
        let s = Settings { answer: Some(true), last_check: 1000, offered: Some("v3.1.0".into()) };
        s.write(&d).unwrap();
        assert_eq!(Settings::read(&d), s);
        assert!(!due(&s, 1000 + EVERY - 1) && due(&s, 1000 + EVERY) && due(&s, 10), "a clock set back: due");
        let mut a: Vec<String> = ["-w", NO_SELF_UPDATE, "--", NO_SELF_UPDATE].iter().map(|s| s.to_string()).collect();
        assert!(take_flag(&mut a));
        assert_eq!(a, ["-w", "--", NO_SELF_UPDATE], "the game's own arguments kept");
        assert!(!take_flag(&mut a));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_fetched_version_takes_the_program_s_place_with_its_name_and_subsystem() {
        let d = scratch("install");
        let staged = d.join(".self-update");
        std::fs::create_dir_all(&staged).unwrap();
        // (the fetched version: windowed, as fetch makes Longshot.exe; this copy: a console program of another name)
        let pe = |sub: u16, tail: &[u8]| {
            let mut v = vec![0u8; 0x200];
            v[..2].copy_from_slice(b"MZ");
            v[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            v[0x80..0x84].copy_from_slice(b"PE\0\0");
            v[0x80 + 24..0x80 + 26].copy_from_slice(&0x20bu16.to_le_bytes());
            v[0x80 + 92..0x80 + 94].copy_from_slice(&sub.to_le_bytes());
            v.extend_from_slice(tail);
            v
        };
        std::fs::write(staged.join("Longshot.exe"), pe(2, b"new")).unwrap();
        let rec = json!({"v": 1, "version": "v3.1.0", "channel": "stable", "platform": "windows-x64",
            "full": {"name": "f", "size": 3, "sha256": "0".repeat(64)}, "program": "Longshot.exe"});
        std::fs::write(staged.join(release::install::RECORD), rec.to_string()).unwrap();
        let exe = d.join("longshot-builder.exe");
        std::fs::write(&exe, pe(3, b"old")).unwrap();
        install_staged(&exe, &staged, true).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), pe(3, b"new"), "the new program, a console one as this copy was");
        assert_eq!(std::fs::read(release::swap::old(&exe)).unwrap(), pe(3, b"old"), "the one before aside");
        let r = release::install::Record::read(&d).unwrap();
        assert_eq!((r.version.as_str(), r.program.as_str()), ("v3.1.0", "longshot-builder.exe"));
        // (the next start deletes the one before)
        assert!(release::swap::tidy(&exe).is_some());
        assert!(!release::swap::old(&exe).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
