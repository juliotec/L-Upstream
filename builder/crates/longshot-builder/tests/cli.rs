//! The command line's own answers: `--help` (and `-h`, `help`) is the usage and status 0; `--version` names what the
//! program carries (a developer's build, here: none).

fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_longshot-builder")).args(args).output().unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn help_is_the_usage_and_version_says_what_is_carried() {
    for h in ["--help", "-h", "help"] {
        let (code, out) = run(&[h]);
        assert_eq!(code, Some(0), "{h}");
        assert!(out.starts_with("usage: longshot-builder build"), "{h}: {out}");
    }
    let (code, out) = run(&["--version"]);
    assert_eq!(code, Some(0));
    assert!(out.starts_with("longshot-builder ") && out.contains("a developer's build"), "{out}");
}
