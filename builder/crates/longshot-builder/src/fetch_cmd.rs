//! `longshot-builder fetch <tag | stable | nightly> --out <version folder> [--from <installed version folder> ...]
//! [--output json|text|user]`: a release's builder installed into a version folder (docs/dev/reference/build.md,
//! "fetch"; the release crate does it). The launcher calls it instead of its own release and delta code; a portable or
//! standalone copy's self-update (self_update.rs) does it into a folder beside itself.
//!
//! Its events: `fetch_progress` as it goes, then `fetched`, or an `error` whose code says why
//! (docs/dev/reference/build-events.md). Exit status 0 done, 1 stopped, 2 a command line it does not take.

use build_core::Error;
use build_core::report::{Event, Reporter};
use release::install::{self, Fetched, Request, Want};
use release::{Cancel, FetchError};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `fetch`'s command line.
#[derive(Debug, PartialEq, Eq)]
pub struct FetchArgs {
    /// What: a tag, `stable` or `nightly`.
    pub want: String,
    /// The version folder.
    pub out: PathBuf,
    /// Installed version folders (or programs) a delta may start from.
    pub from: Vec<PathBuf>,
    /// The platform (`--platform`, for a launcher installing another system's; this one's by default).
    pub platform: String,
}

/// `args` (after `fetch`, `--output` taken out) read, or the usage error's text.
pub fn parse(args: &[String]) -> Result<FetchArgs, String> {
    let (mut want, mut out, mut from, mut platform) = (None, None, Vec::new(), release::platform().to_string());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(value()?)),
            "--from" => from.push(PathBuf::from(value()?)),
            "--platform" => {
                platform = value()?;
                if !["linux-x64", "windows-x64"].contains(&platform.as_str()) {
                    return Err(format!("--platform: {platform:?} is not linux-x64 or windows-x64"));
                }
            }
            v if !v.starts_with("--") && want.is_none() => want = Some(v.to_string()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let want = want.ok_or("fetch needs a version: a tag, stable or nightly")?;
    let out = out.ok_or("fetch needs --out <version folder>")?;
    Ok(FetchArgs { want, out, from, platform })
}

/// A fetch's error as the builder's: each code said as itself (build-events.md's table of codes).
pub fn error_of(e: FetchError) -> Error {
    let t = e.text;
    match e.code {
        "release_unreachable" => Error::player("release_unreachable", t),
        "no_such_release" => Error::player("no_such_release", t),
        "no_release_on_channel" => Error::player("no_release_on_channel", t),
        "release_refused" => Error::player("release_refused", t),
        "no_builder_in_release" => Error::player("no_builder_in_release", t),
        "download_failed" => Error::player("download_failed", t),
        "cancelled" => Error::player("cancelled", t),
        _ => Error::player("fetch_failed", t),
    }
}

/// A person's view that shows nothing (a background self-update).
struct Quiet;

impl crate::progress::UserUi for Quiet {
    fn step(&mut self, _: &str, _: f64) {}
    fn error(&mut self, _: &str) {}
    fn done(&mut self) {}
}

/// Fetches `a`, said to `r` (and, with `show`, a person's progress under `--output user`), its intermediate files in
/// `a.out`'s staging.
pub fn fetch(a: &FetchArgs, r: &Reporter, cancel: &Cancel, show: bool) -> Result<Fetched, Error> {
    let curl = release::curl::Curl::new(cancel.clone()).map_err(|t| Error::player("release_unreachable", t))?;
    let get = |k: &str| build_driver::env(k);
    let say = |s: String| r.log(s);
    let edge = release::forge::Edge { inner: &curl, enabled: release::forge::Edge::enabled_by(&get), said: &say };
    let out = std::path::absolute(&a.out).map_err(|e| Error::io(&a.out, e))?;
    let made_out = !out.exists();
    let scratch = out.join(install::STAGING);
    let forge = release::forge::Forge::new(&edge, release::trust::keys(&get), cancel, &scratch);
    let req =
        Request { want: Want::parse(&a.want), out: out.clone(), from: a.from.clone(), platform: a.platform.clone() };
    let mut ui: Box<dyn crate::progress::UserUi> =
        if show && crate::output::chosen().windows() { crate::progress::open() } else { Box::new(Quiet) };
    let mut said = String::new();
    let mut progress = |p: install::Progress| {
        r.send(Event::FetchProgress { step: p.step, hop: p.hop, hops: p.hops, bytes: p.bytes, total: p.total });
        let text = match p.step {
            "delta" | "patch" | "check" if p.hops > 1 => format!("Updating Longshot (step {} of {})", p.hop, p.hops),
            "delta" | "patch" | "check" => "Updating Longshot".to_string(),
            "full" => "Downloading Longshot".to_string(),
            _ => "Checking for Longshot's release".to_string(),
        };
        let f = if p.total > 0 { p.bytes as f64 / p.total as f64 } else { 0.0 };
        if text != said || p.total > 0 {
            ui.step(&text, f);
            said = text;
        }
    };
    let mut log = |s: String| r.log(s);
    let done = install::fetch(
        &forge,
        &req,
        &release::patch::apply,
        cancel,
        &mut install::Tell { progress: &mut progress, say: &mut log },
    );
    let _ = std::fs::remove_dir_all(&scratch);
    match done {
        Ok(f) => {
            ui.done();
            Ok(f)
        }
        Err(e) => {
            // (a version folder this fetch made, left empty: not left behind)
            if made_out {
                let _ = std::fs::remove_dir(&out);
            }
            Err(error_of(e))
        }
    }
}

/// The `fetched` event of a fetch done.
pub fn fetched_event(f: &Fetched) -> Event {
    let m = &f.manifest;
    Event::Fetched {
        version: m.version.clone(),
        channel: m.channel.name(),
        platform: m.platform.clone(),
        program: f.program.display().to_string(),
        how: f.how.name(),
        from: f.from.clone(),
        through: f.through.clone(),
        downloaded: f.downloaded,
        sha256: m.full.sha256.clone(),
        size: m.full.size,
        stamps: m.stamps.clone(),
    }
}

/// `longshot-builder fetch ...`: its exit status.
pub fn run(args: &[String]) -> ExitCode {
    let mut args = args.to_vec();
    let asked = match crate::output::take(&mut args) {
        Ok(a) => a,
        Err(why) => return crate::usage_error(why, false),
    };
    let json = asked == Some(crate::output::Output::Json);
    let a = match parse(&args) {
        Ok(a) => a,
        Err(why) => return crate::usage_error(why, json),
    };
    let o = crate::output::set(asked);
    let r = Reporter::stdout(o.format());
    let cancel = Cancel::default();
    let c = cancel.clone();
    crate::after::ui().on_cancel(move || c.set());
    match fetch(&a, &r, &cancel, true) {
        Ok(f) => {
            r.send(fetched_event(&f));
            ExitCode::SUCCESS
        }
        Err(e) => {
            let text = e.to_string();
            r.send(Event::Error { code: e.code(), text: text.clone() });
            if o.windows() {
                crate::progress::open().error(&text);
            }
            ExitCode::FAILURE
        }
    }
}

/// The folder a version's files are fetched into beside the program at `exe` (self-update's).
#[must_use]
pub fn beside(exe: &Path) -> PathBuf {
    exe.parent().unwrap_or(Path::new(".")).join(".self-update")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_command_lines_it_takes() {
        let a = parse(&words("nightly --out /v/n --from /v/a --from /v/b")).unwrap();
        assert_eq!(a.want, "nightly");
        assert_eq!(a.out, PathBuf::from("/v/n"));
        assert_eq!(a.from, [PathBuf::from("/v/a"), PathBuf::from("/v/b")]);
        assert_eq!(a.platform, release::platform());
        assert_eq!(parse(&words("v3.0.0 --out o --platform windows-x64")).unwrap().platform, "windows-x64");
        assert!(parse(&words("--out o")).unwrap_err().contains("needs a version"));
        assert!(parse(&words("stable")).unwrap_err().contains("--out"));
        assert!(parse(&words("stable --out o --platform mac")).is_err());
        assert!(parse(&words("stable nightly --out o")).is_err());
    }

    #[test]
    fn each_code_is_said_as_itself() {
        for code in ["no_builder_in_release", "release_refused", "release_unreachable", "cancelled"] {
            assert_eq!(error_of(FetchError::new(code, "x")).code(), code);
        }
        assert_eq!(error_of(FetchError::new("io", "x")).code(), "fetch_failed");
    }
}
