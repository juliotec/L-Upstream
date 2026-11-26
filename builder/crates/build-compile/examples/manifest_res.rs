//! The Windows game's manifest as the `.res` the builder links into longshot.exe (`generated::res::manifest_res`),
//! written to a file: for the tests that build a Windows program the way the game is built (tests/test_utf8_paths.py,
//! tests/test_mods_list.py), so they link the builder's own bytes and copy no writer of theirs.
//!
//! ```text
//! cargo run -q -p build-compile --example manifest_res -- <out.res>
//! ```

fn main() {
    let Some(out) = std::env::args_os().nth(1) else {
        eprintln!("usage: manifest_res <out.res>");
        std::process::exit(2);
    };
    if let Err(e) = std::fs::write(&out, build_compile::generated::res::manifest_res()) {
        eprintln!("{}: {e}", std::path::Path::new(&out).display());
        std::process::exit(1);
    }
}
