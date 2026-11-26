//! A bug's panic is said as an `error` event and the process ends with status 1 (the `panic-test` command of a debug
//! build panics on a worker thread, its message naming the home folder); in the log files too, and both redacted; after
//! the run's `done`, the panic says nothing more on stdout.

// (the panic-test command is a debug build's alone: a release one, `cargo test --release` as tests/test_lifter.py runs the
// workspace's tests, has no command to panic with)
#![cfg(debug_assertions)]

use std::path::Path;

fn run(home: &Path, log: &Path, after_done: bool) -> (Option<i32>, String) {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_longshot-builder"));
    c.args(["panic-test", "--output", "json", "--log"]).arg(log).env("HOME", home).env("USERPROFILE", home);
    if after_done {
        c.args(["--game", "x"]);
    }
    let out = c.output().unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn a_panic_is_an_internal_error_event_and_status_1() {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("panic");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let home = d.join("home-of-someone");
    let log = d.join("run.log");
    let (code, stdout) = run(&home, &log, false);
    assert_eq!(code, Some(1));
    let last: serde_json::Value = serde_json::from_str(stdout.lines().last().expect("an event")).unwrap();
    assert_eq!((last["type"].as_str(), last["code"].as_str()), (Some("error"), Some("internal_error")), "{stdout}");
    let text = last["text"].as_str().unwrap();
    assert!(text.contains("a test's panic"), "{text}");
    assert!(!stdout.contains("home-of-someone"), "the home folder said: {stdout}");
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("internal_error") && !logged.contains("home-of-someone"), "{logged}");
    // (after `done`: nothing more on stdout, the log told, the status `done`'s)
    let _ = std::fs::remove_file(&log);
    let (code, stdout) = run(&home, &log, true);
    assert_eq!(code, Some(0));
    let types: Vec<String> = stdout
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(types, ["done"], "{stdout}");
    assert!(std::fs::read_to_string(&log).unwrap().contains("internal_error"));
}
