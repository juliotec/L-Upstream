//! A launcher profile: one folder that is what a player's profile plays with, which the launcher (or the builder's
//! first run) makes and the builder and the game are given whole (`LONGSHOT_PROFILE=<folder>`, [`SWITCH`]). The layout
//! and the file's format are docs/dev/reference/profiles.md's; the C# launcher follows the same page, and the game's
//! own paths are host/longshot_profile.h's.
//!
//! ```text
//! <profile>/
//!   profile.json         which profile it is: its name, colour, the version and the game copy it plays, its build's
//!                        upscalers, the save slot it starts on ([`Profile`]); every key it does not know kept
//!   settings.ini         the game's settings (longshot_options.ini's lines)
//!   saves/               the saves alone: save/<uid>/slot_NN/, save-work/
//!   mods/                its mods, manifest mods alone (a folder with mod.json each; one in an emulator's layout is
//!                        imported first: `Longshot.exe mod import`)
//! ```
//!
//! What is here: [`Profile`], `profile.json` read and written keeping every key it does not know as it is, in its
//! place (the Mod API's `"mods"` among them); [`Layout`], the folder's paths. The builder's play
//! (build_driver::play::Profile) reads and writes the file through it alone.

mod edit;
mod file;
mod mods;

pub use edit::{EDIT_LOCK, EditGuard, LockError, WAIT as EDIT_WAIT, edit_lock};
pub use mods::{ModsError, set_mods};

pub use file::{Error, FILE_MAX, Profile, Upscaler, VERSION};
/// A value as the file has it ([`Profile::raw`], [`Profile::put`]).
pub use serde_json::value::RawValue;

use std::path::{Path, PathBuf};

/// The environment switch that names the profile's folder (host/longshot_profile.h's `LONGSHOT_PROFILE`).
pub const SWITCH: &str = "LONGSHOT_PROFILE";

/// The profile's own file.
pub const FILE: &str = "profile.json";
/// The game's settings (host/longshot_profile.h's `longshot_profile_settings`).
pub const SETTINGS: &str = "settings.ini";
/// The saves' folder: `save/` and `save-work/` alone (host/longshot_profile.h's `longshot_profile_saves`).
pub const SAVES: &str = "saves";
/// The mods' folder: manifest mods alone (docs/dev/reference/mods.md).
pub const MODS: &str = "mods";

/// A profile's folder and the paths in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    dir: PathBuf,
}

impl Layout {
    /// The profile in `dir` (made whole from the working folder when it is relative).
    #[must_use]
    pub fn new(dir: impl AsRef<Path>) -> Layout {
        let dir = dir.as_ref();
        Layout { dir: std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()) }
    }

    /// The folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `profile.json`.
    #[must_use]
    pub fn file(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// `settings.ini`.
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.dir.join(SETTINGS)
    }

    /// `saves/`.
    #[must_use]
    pub fn saves(&self) -> PathBuf {
        self.dir.join(SAVES)
    }

    /// `mods/`.
    #[must_use]
    pub fn mods(&self) -> PathBuf {
        self.dir.join(MODS)
    }

    /// The edit lock's file ([`EDIT_LOCK`]).
    #[must_use]
    pub fn edit_lock(&self) -> PathBuf {
        self.dir.join(EDIT_LOCK)
    }

    /// `profile.json` read ([`Profile::read`]).
    ///
    /// # Errors
    /// As [`Profile::read`].
    pub fn read(&self) -> Result<Profile, Error> {
        Profile::read(&self.file())
    }
}

#[cfg(test)]
mod tests;
