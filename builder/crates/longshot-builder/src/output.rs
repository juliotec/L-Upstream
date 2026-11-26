//! `--output json|text|user|recomplauncher`: how every command that builds or plays speaks (docs/dev/reference/build.md,
//! "Output").
//!
//! - `json`: the events as JSON Lines on stdout (docs/dev/reference/build-events.md), for a launcher;
//! - `recomplauncher`: RecompLauncher's lines on stdout, `progress <0..1> <the step>` and `error <text>`;
//! - `text`: text for a person on stdout and stderr, never a window;
//! - `user`: text as `text`, and a person's windows: the progress window while a long build goes on (progress.rs), a
//!   message box for an error.
//!
//! Without `--output`: `user` when nobody reads the terminal (stderr is not one), a desktop is there (Linux: `DISPLAY` or
//! `WAYLAND_DISPLAY`) and nothing says no one is looking (`HOS_HEADLESS=1`, a job queue's job: `LONGSHOT_JOB` or a
//! `LONGSHOT_JOB_*` variable, `CI`, RecompLauncher's Big Picture mode); else `text` ([`decide`]).

pub use build_driver::modhook::Output;

/// The output of a run asked `asked` (`--output`'s, if given): it, else `user` or `text` as the module's docs say.
/// `terminal`: whether stderr is a terminal; `get`: the environment's variables; `names`: their names (for the job
/// queue's `LONGSHOT_JOB_*`); `windows`: on Windows (a desktop always there).
pub fn decide(
    asked: Option<Output>,
    terminal: bool,
    get: &dyn Fn(&str) -> Option<String>,
    names: &[String],
    windows: bool,
) -> Output {
    if let Some(o) = asked {
        return o;
    }
    let set = |k: &str| get(k).is_some_and(|v| !v.trim().is_empty());
    let display = windows || set("DISPLAY") || set("WAYLAND_DISPLAY");
    let headless = get("HOS_HEADLESS").is_some_and(|v| v.trim() == "1");
    let job = names.iter().any(|n| n == "LONGSHOT_JOB" || n.starts_with("LONGSHOT_JOB_"));
    let big_picture = get("RECOMPLAUNCHER_MODE").as_deref() == Some("bigpicture");
    if !terminal && display && !headless && !job && !set("CI") && !big_picture { Output::User } else { Output::Text }
}

/// This run's output, once decided ([`set`]; text until then).
static CHOSEN: std::sync::OnceLock<Output> = std::sync::OnceLock::new();

/// This run's output decided: `asked`, else from this process's stderr and environment.
pub fn set(asked: Option<Output>) -> Output {
    use std::io::IsTerminal;
    let names: Vec<String> = std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
    let o = decide(asked, std::io::stderr().is_terminal(), &|k| std::env::var(k).ok(), &names, cfg!(windows));
    *CHOSEN.get_or_init(|| o)
}

/// This run's output ([`set`]'s; text before it).
pub fn chosen() -> Output {
    CHOSEN.get().copied().unwrap_or(Output::Text)
}

/// `--output <value>` (or `--output=<value>`) taken out of `args` (before a `--`): the output asked, or the error's
/// text. The others left as they were.
pub fn take(args: &mut Vec<String>) -> Result<Option<Output>, String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let Some(i) = args[..end].iter().position(|a| a == "--output" || a.starts_with("--output=")) else {
        return Ok(None);
    };
    let value = match args[i].strip_prefix("--output=") {
        Some(v) => {
            let v = v.to_string();
            args.remove(i);
            v
        }
        None if i + 1 < end => {
            let v = args.remove(i + 1);
            args.remove(i);
            v
        }
        None => return Err("--output needs a value".into()),
    };
    Output::parse(&value).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use build_core::report::Format;

    fn env(vars: &'static [(&'static str, &'static str)]) -> (impl Fn(&str) -> Option<String>, Vec<String>) {
        (
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()),
            vars.iter().map(|(n, _)| n.to_string()).collect(),
        )
    }

    #[test]
    fn the_output_asked_or_user_only_when_a_person_is_there_to_see_it() {
        let d = |asked, terminal, vars: &'static [(&'static str, &'static str)], windows| {
            let (get, names) = env(vars);
            decide(asked, terminal, &get, &names, windows)
        };
        let desktop: &[(&str, &str)] = &[("DISPLAY", ":0")];
        // (asked: as asked, whatever else)
        for o in [Output::Json, Output::Text, Output::User, Output::RecompLauncher] {
            assert_eq!(d(Some(o), true, &[("CI", "1")], false), o);
        }
        // (started from a desktop's icon or a launcher that asks nothing: user)
        assert_eq!(d(None, false, desktop, false), Output::User);
        assert_eq!(d(None, false, &[("WAYLAND_DISPLAY", "wayland-0")], false), Output::User);
        assert_eq!(d(None, false, &[], true), Output::User, "Windows: a desktop always");
        // (else text: a terminal, no display, headless, a queue's job, CI, Big Picture)
        assert_eq!(d(None, true, desktop, false), Output::Text, "a terminal reads it");
        assert_eq!(d(None, false, &[], false), Output::Text, "no display");
        assert_eq!(d(None, false, &[("DISPLAY", "")], false), Output::Text);
        assert_eq!(d(None, false, &[("DISPLAY", ":0"), ("HOS_HEADLESS", "1")], false), Output::Text);
        assert_eq!(d(None, false, &[("DISPLAY", ":0"), ("LONGSHOT_JOB", "x")], false), Output::Text);
        assert_eq!(d(None, false, &[("DISPLAY", ":0"), ("LONGSHOT_JOB_MODE", "shared")], false), Output::Text);
        assert_eq!(d(None, false, &[("DISPLAY", ":0"), ("LONGSHOT_JOBS", "8")], false), Output::User, "the compilers'");
        assert_eq!(d(None, false, &[("DISPLAY", ":0"), ("CI", "true")], false), Output::Text);
        assert_eq!(d(None, false, &[("RECOMPLAUNCHER_MODE", "bigpicture")], true), Output::Text);
        assert_eq!(d(None, false, &[("RECOMPLAUNCHER_MODE", "desktop")], true), Output::User);
    }

    #[test]
    fn each_output_s_format_and_windows() {
        assert_eq!((Output::Json.format(), Output::Json.windows()), (Format::Json, false));
        assert_eq!((Output::RecompLauncher.format(), Output::RecompLauncher.windows()), (Format::Lines, false));
        assert_eq!((Output::Text.format(), Output::Text.windows()), (Format::Text, false));
        assert_eq!((Output::User.format(), Output::User.windows()), (Format::Text, true));
        assert!(Output::parse("JSON").is_err() && Output::parse("lines").is_err());
    }

    #[test]
    fn output_is_taken_out_before_a_double_dash() {
        let w = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let mut a = w("build --game g --output json --out o");
        assert_eq!(take(&mut a), Ok(Some(Output::Json)));
        assert_eq!(a, w("build --game g --out o"));
        let mut a = w("--output=user -w");
        assert_eq!(take(&mut a), Ok(Some(Output::User)));
        assert_eq!(a, w("-w"));
        let mut a = w("-- --output json");
        assert_eq!(take(&mut a), Ok(None), "the game's");
        assert!(take(&mut w("--output")).is_err());
        assert!(take(&mut w("--output -- x")).is_err());
        assert!(take(&mut w("--output xml")).is_err());
    }
}
