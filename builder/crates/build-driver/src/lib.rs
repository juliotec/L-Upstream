//! The builder's driver (`longshot-builder`): a player's own dump to a game folder.
//!
//! [`build::build`] runs the stages in order, each with its cache ([`build`]'s docs): the read stage
//! ([`read::ReadStage`], over game-files and image), the lift (lift::whole), the compile (build_compile), the
//! finish (the game folder's files, [`folder`]; the assets, [`assets`], over build-assets) and the shader preparation
//! ([`shaders`], supervised by [`supervisor`]). Each stage is called through one seam ([`stages`]), so a test runs the
//! driver with its own and a stage still being written is one change to bring in.
//!
//! - [`options`]: what a build is asked (the game, the folder, the player's choices);
//! - [`layout`]: the per-user cache, a work folder per game folder, the stores every work folder shares;
//! - [`folder`]: the game folder's checks, the files swapped in, its own files and its record for `update`;
//! - [`play`]: the version folder players start Longshot from, a profile's profile.json, the fast check and play;
//! - [`verify`]: a game folder checked against what the builder put there, and repaired;
//! - [`times`], [`log`]: the builds' times and the run log.
//!
//! Everything it says goes through build-core's [`build_core::report::Reporter`]: JSON Lines with `--output json`, text for
//! a person otherwise (docs/dev/reference/build-events.md).

pub mod assets;
pub mod build;
pub mod channel;
pub mod clashes;
pub mod folder;
pub mod hle;
pub mod layout;
pub mod log;
pub mod modhook;
pub mod options;
pub mod perf;
pub mod play;
pub mod read;
pub mod redact;
pub mod runtimes;
pub mod shaders;
pub mod sources;
pub mod stages;
pub mod storage;
pub mod supervisor;
pub mod times;
pub mod tools;
pub mod verify;

pub use build::{Built, Places};
/// What a compile makes (`longshot-builder compile`'s modes).
pub use build_compile::Mode as CompileMode;
pub use options::{BuildOptions, CompileShaders, GameSource, GuestProfile, Upscaler};
pub use stages::Stages;

static FLAGS: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// A build setting given on the command line (`--cache`, `--ngx-sdk`, `--builder-exe`), by the variable it once was
/// only: [`env`] answers it over the environment's (docs/dev/reference/build.md, "Build switches").
pub fn set_flag(name: &str, value: &str) {
    FLAGS.lock().unwrap_or_else(|p| p.into_inner()).insert(name.to_string(), value.to_string());
}

/// An environment variable, `None` when unset or not Unicode (the one place the driver reads the environment); a
/// command line's setting of the same name ([`set_flag`]) wins.
pub fn env(name: &str) -> Option<String> {
    let flag = FLAGS.lock().unwrap_or_else(|p| p.into_inner()).get(name).cloned();
    flag.or_else(|| std::env::var(name).ok())
}

/// [`env`] as the system has it: a value that is not Unicode (a home folder's name on Linux) is kept, not dropped.
pub fn env_os(name: &str) -> Option<std::ffi::OsString> {
    let flag = FLAGS.lock().unwrap_or_else(|p| p.into_inner()).get(name).cloned();
    flag.map(Into::into).or_else(|| std::env::var_os(name))
}

/// The SHA-256 of a file, lower-case hex, and its size: the one helper the vendors' files, the tools' archives and the
/// game folder's pinned files are checked with.
pub fn sha256_file(path: &std::path::Path) -> std::result::Result<(String, u64), String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = sha2::Sha256::new();
    let n = std::io::copy(&mut f, &mut h).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((build_core::key::to_hex(&h.finalize()), n))
}

/// The SHA-256 of a text, lower-case hex.
pub fn sha256_text(text: &str) -> String {
    use sha2::Digest;
    build_core::key::to_hex(&sha2::Sha256::digest(text.as_bytes()))
}

/// A test's own empty folder (`<target>/test-scratch/build-driver-<name>-<pid>`).
#[cfg(test)]
pub(crate) fn scratch(name: &str) -> std::path::PathBuf {
    // (under the target folder, on the disk: the system's temporary folder may be memory, and these hold game-folder
    // sized trees and stand-in games)
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |t| root.join(t));
    let d = target.join("test-scratch").join(format!("build-driver-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
