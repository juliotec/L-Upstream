//! Longshot's releases as a player's builder installs them (docs/dev/reference/releases.md): `longshot-builder fetch`
//! and `self-update`. The Longshot Launcher did this in C# (Core/LongshotBuildRelease.cs, Releases.cs,
//! ReleaseTrust.cs, CdnFetcher.cs, SelfUpdate.cs, App/ZstdPatcher.cs); it now asks the builder.
//!
//! - [`versions`]: the channels and the version order.
//! - [`trust`]: the release key, and a release's `SHA256SUMS` checked against its signature.
//! - [`forge`]: the forge's release list and a release's files, through Cloudflare's edge first ([`forge::Edge`]).
//! - [`curl`]: the network, by the system's curl (as the builder's other downloads), cancellable, with progress.
//! - [`manifest`]: a release's manifest for a platform, and the single delta from a kept file.
//! - [`chain`]: a chain of up to 5 deltas through earlier releases, within a look-back of 20.
//! - [`patch`]: a delta applied (`zstd --patch-from --long=30`), by ruzstd.
//! - [`install`]: all of it: a version's program into a version folder, by a delta, a chain or the full file.
//! - [`swap`]: a program replaced by a new one while it runs (self-update), and a Windows program made windowed.

pub mod chain;
pub mod curl;
pub mod forge;
pub mod install;
pub mod manifest;
pub mod patch;
pub mod swap;
pub mod trust;
pub mod versions;

pub use versions::Channel;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cancel switch, shared by whoever may cancel (a window's Cancel, Ctrl-C) and the work that checks it.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// Cancelled from now on.
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether it was cancelled.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why a fetch stopped: a stable code (docs/dev/reference/build-events.md) and the sentence for the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError {
    /// The code: `release_unreachable`, `no_such_release`, `no_release_on_channel`, `release_refused`,
    /// `no_builder_in_release`, `download_failed`, `io` or `cancelled`.
    pub code: &'static str,
    /// The sentence.
    pub text: String,
}

impl FetchError {
    /// An error of this code.
    #[must_use]
    pub fn new(code: &'static str, text: impl Into<String>) -> FetchError {
        FetchError { code, text: text.into() }
    }

    /// Cancelled.
    #[must_use]
    pub fn cancelled() -> FetchError {
        FetchError::new("cancelled", "Cancelled.")
    }

    /// A file that could not be read or written.
    #[must_use]
    pub fn io(path: &Path, e: &dyn std::fmt::Display) -> FetchError {
        FetchError::new("io", format!("{}: {e}", path.display()))
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for FetchError {}

/// Bytes' SHA-256, lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

/// A file's SHA-256 (lowercase hex) and size, streamed.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex(&h.finalize()), n))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// This system's platform key in a release: `windows-x64` or `linux-x64`.
#[must_use]
pub fn platform() -> &'static str {
    if cfg!(windows) { "windows-x64" } else { "linux-x64" }
}

/// The program's name in a version folder for a platform: `Longshot.exe` (windowed) or `Longshot.x86_64` (play.rs's
/// names).
#[must_use]
pub fn program_name(platform: &str) -> &'static str {
    if platform == "windows-x64" { "Longshot.exe" } else { "Longshot.x86_64" }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    /// A folder of the test's own, under the workspace's target folder (never /tmp).
    pub fn scratch(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("release-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
