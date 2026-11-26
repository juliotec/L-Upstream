//! What stops a build.

use std::path::{Path, PathBuf};

/// The result of a step of the build.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a build stopped.
///
/// A [`Error::Player`] is said to the player as it is: a sentence that says what happened and what to do, with a
/// stable `code` a launcher may show its own words for (the `error` event, [`crate::report`]). The other kinds are the
/// builder's own failures; the driver says them with their cause, and asks for the log.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A reason the player can act on, in their words: a file missing, the wrong game version, a full disk.
    #[error("{text}")]
    Player {
        /// Stable id, snake_case (`disk_full`, `wrong_game_version`, `work_folder_busy`).
        code: &'static str,
        /// The sentence.
        text: String,
    },
    /// A file or folder could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or folder.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// A tool the build runs (the compiler, the linker) failed.
    #[error("{tool} failed: {detail}")]
    Tool {
        /// The tool, as named in the log.
        tool: String,
        /// What it said, or its exit status.
        detail: String,
    },
    /// Data that is not what it must be (a damaged file, a format this builder does not know).
    #[error("{what}: {detail}")]
    Format {
        /// The data (a file, a table).
        what: String,
        /// What is wrong with it.
        detail: String,
    },
}

impl Error {
    /// A reason for the player, with its stable `code`.
    pub fn player(code: &'static str, text: impl Into<String>) -> Error {
        Error::Player { code, text: text.into() }
    }

    /// Its stable id: a player error's own code, else the kind (`io`, `tool_failed`, `bad_data`).
    pub fn code(&self) -> &'static str {
        match self {
            Error::Player { code, .. } => code,
            Error::Io { .. } => "io",
            Error::Tool { .. } => "tool_failed",
            Error::Format { .. } => "bad_data",
        }
    }

    /// An I/O error on `path`.
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Error {
        Error::Io { path: path.as_ref().to_path_buf(), source }
    }

    /// Data that is not of its format.
    pub fn format(what: impl Into<String>, detail: impl Into<String>) -> Error {
        Error::Format { what: what.into(), detail: detail.into() }
    }

    /// A tool's failure.
    pub fn tool(tool: impl Into<String>, detail: impl Into<String>) -> Error {
        Error::Tool { tool: tool.into(), detail: detail.into() }
    }
}

/// Whether `e` is a full drive: ENOSPC or EDQUOT (a user's quota) on Unix, ERROR_DISK_FULL (112) or
/// ERROR_HANDLE_DISK_FULL (39) on Windows.
pub fn is_disk_full(e: &std::io::Error) -> bool {
    let Some(code) = e.raw_os_error() else { return e.kind() == std::io::ErrorKind::StorageFull };
    if cfg!(windows) {
        matches!(code, 112 | 39)
    } else {
        // (EDQUOT: 122 on Linux, 69 on the BSDs and macOS)
        code == 28 || code == if cfg!(target_os = "linux") { 122 } else { 69 }
    }
}

/// A full drive said to the player (`disk_full`): where, and what to do.
pub fn disk_full(path: &Path) -> Error {
    Error::player(
        "disk_full",
        format!(
            "The drive is full: no room to write {}. Free some space, or choose a game folder or a work folder on \
             another drive, and build again.",
            path.display()
        ),
    )
}

/// `.at(path)` on an [`std::io::Result`]: the error with the path it was about.
pub trait IoContext<T> {
    /// The error, said with `path`.
    fn at(self, path: impl AsRef<Path>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|e| Error::io(path, e))
    }
}
