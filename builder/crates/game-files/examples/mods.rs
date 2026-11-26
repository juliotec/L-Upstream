//! `exefs_patches.txt` as a build writes it, for tests/test_mods_sizes.py, whose game reads it:
//!
//! ```text
//! cargo run -p game-files --example mods -- exefs-list <mod> <patch file> <kind> <note> ...
//! ```
//!
//! `exefs-list` prints the `exefs_patches.txt` a build writes for these files, four words each: its mod, its path, its
//! kind (the list's word: code, data, other, ...) and its note ("" for none), its mod on.

use game_files::patches::{Kind, PatchFile, fnv32, list_text};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("exefs-list") if args.len() % 4 == 1 => {
            let mut files = Vec::new();
            for f in args[1..].chunks(4) {
                let path = Path::new(&f[1]);
                let Ok(kind) = serde_json::from_value::<Kind>(serde_json::json!(f[2])) else { return usage() };
                let Ok(raw) = std::fs::read(path) else { return usage() };
                files.push(PatchFile {
                    mod_name: f[0].clone(),
                    file: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    hash: format!("{:08x}", fnv32(&raw)),
                    kind,
                    note: f[3].clone(),
                    on: true,
                    writes: Vec::new(),
                });
            }
            print!("{}", list_text(&files));
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: mods exefs-list <mod> <file> <kind> <note> ...");
    ExitCode::from(2)
}
