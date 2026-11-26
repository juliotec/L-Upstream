//! A compile that failed is named by the compiler's first `error:` line, in the player's message and in the log, even
//! when hundreds of lines of notes follow it: only the last 4,000 bytes used to be kept (and 1,500 in the log), so the
//! error the compile stopped on was cut and the build said only which file failed.

use build_compile::compile::{first_error, short};
use build_compile::{FailedUnit, Failure, Stopped};

/// What clang says of one error in a big generated file: the error, then a long run of notes.
fn said() -> String {
    let mut t = String::from("code_009.c:12:7: error: extraneous ')' before ';'\n");
    for i in 0..400 {
        t.push_str(&format!("code_009.c:{}:1: note: in expansion of macro 'GUEST_{i}'\n", 13 + i));
    }
    t.push_str("1 error generated.\n");
    t
}

#[test]
fn the_log_keeps_the_first_error_and_the_end() {
    let text = said();
    assert!(text.len() > 4000, "the case is one the tail alone cut");
    let log = short(&text, 1500);
    assert!(log.starts_with("code_009.c:12:7: error: extraneous ')' before ';'\n[... "), "{log}");
    assert!(log.ends_with("1 error generated.\n"), "{log}");
    assert!(log.len() < 1600, "about 1,500 bytes: {}", log.len());
    assert_eq!(short("a: error: b\n", 1500), "a: error: b\n", "a text that fits is whole");
}

#[test]
fn a_text_with_no_error_keeps_its_first_line() {
    let text = format!("clang: killed\n{}end\n", "x\n".repeat(3000));
    let log = short(&text, 100);
    assert!(log.starts_with("clang: killed\n[... "), "{log}");
    assert!(log.ends_with("end\n"));
    assert_eq!(first_error(&text), None);
}

#[test]
fn a_cut_lands_on_a_character() {
    let text = format!("é: error: {}\n{}", "é".repeat(2000), "ü".repeat(2000));
    let log = short(&text, 101);
    assert!(log.contains("[... "));
}

#[test]
fn the_player_reads_the_first_error() {
    let unit = |name: &str, text: &str| FailedUnit {
        name: name.into(),
        from_lift: true,
        text: text.into(),
        changed_by_pc: false,
    };
    let f = Failure {
        stopped: Stopped::Compile,
        units: vec![unit("code_001.c", "Killed\n"), unit("code_009.c", &said())],
        text: String::new(),
        objects: Vec::new(),
    };
    let e = f.error();
    assert_eq!(e.code(), "compile_failed");
    assert_eq!(
        e.to_string(),
        "2 file(s) did not compile: code_001.c, code_009.c. The compiler said: code_009.c:12:7: error: extraneous ')' \
         before ';'"
    );
    let none = Failure { units: vec![unit("a.c", "Killed\n")], ..f };
    assert_eq!(none.error().to_string(), "1 file(s) did not compile: a.c");
}
