//! ReShade's preprocessor as the build adapts it (`build_compile::generated::reshade_preprocessor`), written to a file:
//! for scripts/reshade/check_fx.py, the development probe of the FX compiler, so it builds what the game builds.
//!
//! ```text
//! cargo run -p build-compile --example reshade_preprocessor -- <third_party/reshade_fx/source> <out.cpp>
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [upstream, out] = &args[..] else {
        eprintln!("usage: reshade_preprocessor <reshade_fx/source> <out.cpp>");
        return ExitCode::from(2);
    };
    match build_compile::generated::reshade_preprocessor(upstream) {
        Ok(text) => match std::fs::write(out, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{}: {e}", out.display());
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
