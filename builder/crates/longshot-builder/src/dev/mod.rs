//! `longshot-builder dev`: the lifter's commands, for developers (docs/dev/reference/builder.md, "The parts").
//!
//! - `discover`: the image's functions, their counts and the time they take (discover.rs);
//! - `sites`: the build's hook sites, read, resolved and placed over the image's own discovery (sites_cmd.rs);
//! - `lift`: the whole lift, from the image to gen/ (lift_cmd.rs, the `lift` crate's `whole`);
//! - `profile`: a run's counts as a guest profile (profile_cmd.rs);
//! - `site-hooks`: hook sites as the lift lowers them, each a function a C test calls (site_hooks.rs);
//! - `words`: single instructions as functions, for the semantics check (words.rs);
//! - `decode <hex word> ...`: each instruction word as the decoder sees it;
//! - `tools`: tools.lock, the build tools the builder carries: check, install, add, missing, publish, make (tools_cmd.rs);
//! - `release`: a release's packaging: the pack it carries (sources, unpack), its dry run, the earlier releases' files,
//!   the deltas and the manifest (fetch-bases, deltas, files), RecompLauncher's zip (release_cmd.rs, release_deltas.rs);
//! - `rust-key [--root <dir>]`: the key of the tree's Rust sources, which the release writes beside the prebuilt
//!   `liblongshot_rs.a` it built (build-compile's `rust`).

use std::process::ExitCode;

mod discover;
mod lift_cmd;
mod profile_cmd;
pub(crate) mod release_cmd;
mod release_deltas;
mod release_objects;
mod site_hooks;
mod sites_cmd;
mod tools_cmd;
mod words;

const USAGE: &str = "usage: longshot-builder dev discover --image <dir>
       longshot-builder dev sites --image <dir> --tables <game>
       longshot-builder dev lift (--work <dir> | --image <dir> --gen <dir>) [--repo <dir> | --host <dir> --tables <dir>]
                                 [--exefs <dir> | --npdm <file>] [--entries <file>] [--devirt <file>]
                                 [--profile-generate | --profile <file> [--hot-share F] [--hot-max N]]
                                 [--no-polls] [--no-mark-bitmap] [--sync-all-at-calls] [--mark-each]
                                 [--no-simplify] [--split-over N] [--split-piece N] [--unit-cost N] [--hot-unit-cost N]
                                 [--cold-unit-cost N] [--jobs N] [--store <dir>] [--force]
                                 [--output json|text]
       longshot-builder dev profile --map <profile_map.txt> --raw <counts> ... [--into <profile>] --out <profile>
       longshot-builder dev site-hooks --tables <game> --build <build id> --out <file.ll>
                                       <name>=<site>[@<pc>[:<callee>]][/<ident>] ...
       longshot-builder dev words --in <file> --pc <hex> --out <file.ll> --list <file> [--seq <file>] [--count-checks]
       longshot-builder dev decode <hex word> ...
       longshot-builder dev tools [--lock <file>] check|install|add|missing|publish|make ... (dev tools --help)
       longshot-builder dev release sources|unpack|dry-run|fetch-bases|deltas|files|recomplauncher ... (dev release --help)
       longshot-builder dev rust-key [--root <dir>]";

fn decode(words: &[String]) -> ExitCode {
    let mut ok = true;
    for a in words {
        let Ok(w) = u32::from_str_radix(a.trim_start_matches("0x"), 16) else {
            eprintln!("longshot-builder dev decode: not a hex word: {a}");
            ok = false;
            continue;
        };
        match a64::decode(w) {
            Some(insn) => println!("{w:08x}  {insn}  {:?}", insn.flow()),
            None => println!("{w:08x}  (unallocated)"),
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `rust-key [--root <dir>]`: the key of the Rust sources of the tree (default: the current folder), on stdout.
fn rust_key(args: &[String]) -> ExitCode {
    let root = match args {
        [] => std::path::PathBuf::from("."),
        [flag, dir] if flag == "--root" => std::path::PathBuf::from(dir),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match build_compile::rust::source_key(&root) {
        Ok(key) => {
            println!("{key}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("longshot-builder dev rust-key: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `longshot-builder dev <args>`: `args` are the words after `dev`.
pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("discover") => discover::run(&args[1..]),
        Some("sites") => sites_cmd::run(&args[1..]),
        Some("site-hooks") => site_hooks::run(&args[1..]),
        Some("lift") => lift_cmd::run(&args[1..]),
        Some("words") => words::run(&args[1..]),
        Some("profile") => profile_cmd::run(&args[1..]),
        Some("decode") if args.len() > 1 => decode(&args[1..]),
        Some("tools") => tools_cmd::run(&args[1..]),
        Some("release") => release_cmd::run(&args[1..]),
        Some("rust-key") => rust_key(&args[1..]),
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
