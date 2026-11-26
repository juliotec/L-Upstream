//! The driver's log rule (build_driver::redact) on stdin's lines, for tests/test_log_redact.py to compare with the
//! game's (host/log_redact.c, gpu/vk/compat/longshot_redact.h):
//!
//! ```text
//! cargo run -p build-driver --example redact -- core <home in hex> <user in hex> <0|1: Windows' rule>
//! cargo run -p build-driver --example redact -- line          (this PC's home folder and name, as the driver's log)
//! ```

use build_driver::redact::{Who, redact_line};
use std::io::{BufRead, Write};
use std::process::ExitCode;

fn unhex(h: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> =
        (0..h.len() / 2).map(|i| h.get(2 * i..2 * i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect();
    String::from_utf8(bytes?).ok()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rule: Box<dyn Fn(&str) -> String> = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["core", home, user, windows] => {
            let (Some(home), Some(user)) = (unhex(home), unhex(user)) else { return ExitCode::from(2) };
            let windows = windows == "1";
            Box::new(move |l| redact_line(l, &home, &user, windows))
        }
        ["line"] => {
            let who = Who::here();
            Box::new(move |l| who.redact(l))
        }
        _ => {
            eprintln!("usage: redact core <home hex> <user hex> <0|1> | redact line");
            return ExitCode::from(2);
        }
    };
    let mut out = std::io::stdout().lock();
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    while input.read_line(&mut line).is_ok_and(|n| n > 0) {
        let _ = out.write_all(rule(&line).as_bytes());
        line.clear();
    }
    ExitCode::SUCCESS
}
