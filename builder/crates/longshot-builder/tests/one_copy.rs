//! One copy per profile, between processes: while another process holds a profile (its game running, or its build),
//! a second `play` of it starts nothing and says so, with exit status 123 (`profile_in_use` with `--output json`);
//! another profile, and the same one once let go, play as before.

use build_core::lock::ProfileLock;
use std::path::Path;

fn play(profile: &Path, output: &str) -> (Option<i32>, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_longshot-builder"))
        .args(["play", "--output", output, "--profile"])
        .arg(profile)
        .env("HOS_HEADLESS", "1")
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

#[test]
fn a_second_play_of_a_held_profile_is_refused_and_other_profiles_play() {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("one-copy");
    let _ = std::fs::remove_dir_all(&d);
    let (ann, bo) = (d.join("ann"), d.join("bo"));
    let held = ProfileLock::take(&ann).unwrap().expect("free");
    let (code, stdout, stderr) = play(&ann, "text");
    assert_eq!(code, Some(123), "{stderr}");
    assert!(stderr.contains("already running") && stdout.is_empty(), "{stderr}");
    let (code, stdout, _) = play(&ann, "json");
    assert_eq!(code, Some(123));
    let e: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!((e["type"].as_str(), e["code"].as_str()), (Some("error"), Some("profile_in_use")), "{stdout}");
    // (another profile: not held; it goes on to play, and stops as a profile with no copy does)
    assert_eq!(play(&bo, "text").0, Some(120));
    drop(held);
    assert_eq!(play(&ann, "text").0, Some(120), "let go: the same profile plays");
    assert!(!ProfileLock::held(&ann), "and the lock went with that process");
    std::fs::remove_dir_all(&d).unwrap();
}
