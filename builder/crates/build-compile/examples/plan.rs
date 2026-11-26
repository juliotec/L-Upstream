//! What a compile would do in a work folder, compiling nothing: every unit, what it would come to (up to date, taken
//! from the store, compiled), its keys and its command.
//!
//! ```text
//! cargo run -p build-compile --example plan -- --work <work folder> [--root <tree>] [--store <dir>]
//!     [--mode game|game-only|renderer|dry-run] [--commands]
//! ```

use build_compile::{Fate, Mode, Options};
use build_core::report::{Format, Reporter};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let (mut work, mut store, mut mode, mut commands) = (None, None, Mode::Game, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = args.next().map(PathBuf::from).unwrap_or(root),
            "--work" => work = args.next().map(PathBuf::from),
            "--store" => store = args.next().map(PathBuf::from),
            "--commands" => commands = true,
            "--mode" => {
                mode = match args.next().as_deref() {
                    Some("game-only") => Mode::GameOnly,
                    Some("renderer") => Mode::RendererOnly,
                    Some("dry-run") => Mode::DryRun,
                    _ => Mode::Game,
                }
            }
            _ => {
                eprintln!("usage: plan --work <dir> [--root <dir>] [--store <dir>] [--mode ...] [--commands]");
                std::process::exit(2);
            }
        }
    }
    let Some(work) = work else {
        eprintln!("--work <dir> is needed");
        std::process::exit(2);
    };
    let mut o = Options::new(root.canonicalize().unwrap_or(root), work);
    o.mode = mode;
    o.store = store;
    let r = Reporter::to(Format::Text, Box::new(std::io::stderr()));
    match build_compile::preview(&o, &r) {
        Ok(units) => {
            let (mut fresh, mut stored, mut compile) = (0, 0, 0);
            for p in &units {
                let fate = match p.fate {
                    Fate::UpToDate => {
                        fresh += 1;
                        "up to date"
                    }
                    Fate::Stored => {
                        stored += 1;
                        "stored"
                    }
                    Fate::Compile => {
                        compile += 1;
                        "compile"
                    }
                };
                println!(
                    "{fate:<10} {:<40} base {} key {}",
                    p.unit.name,
                    &p.base[..12],
                    p.key.as_deref().map_or("-", |k| &k[..12])
                );
                if commands {
                    println!("    {}", build_compile::compile::command(&p.unit).join(" "));
                }
            }
            println!("{} units: {fresh} up to date, {stored} from the store, {compile} to compile", units.len());
        }
        Err(e) => {
            eprintln!("{} ({})", e, e.code());
            std::process::exit(1);
        }
    }
}
