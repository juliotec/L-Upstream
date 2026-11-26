//! What every stage of the builder (`longshot-builder`) shares.
//!
//! The builder turns a player's own dump into a game folder in stages (`docs/dev/reference/build.md`): read the game
//! files, lift the game's code, compile, finish. Each stage is a crate of its own; this one holds what they have in
//! common, so that they say the same things the same way:
//!
//! - [`report`]: the build's events, as JSON Lines for a launcher (`--output json`) or text for a person, from any thread;
//! - [`Error`]: what stops a build: the sentence a player reads with a stable code, or a plain error with its cause;
//! - [`jobs`] and [`memory`]: how many workers a step may run, and the gate that starts a big one only when the memory
//!   free holds it;
//! - [`fsx`]: the file operations a build needs to be safe against a stopped run, a full disk or an antivirus (atomic
//!   writes, copies, links);
//! - [`key`]: the hashes the caches are keyed by;
//! - [`lock`]: one build at a time in a work folder;
//! - [`child`]: the programs a build starts, tied to it so none outlives it.

pub mod channel;
pub mod child;
pub mod fsx;
pub mod jobs;
pub mod key;
pub mod lock;
pub mod memory;
pub mod report;
pub mod res;
pub mod vulkan;

mod error;
mod sys;

pub use error::{Error, IoContext, Result, disk_full, is_disk_full};

/// An on/off switch's value, as the game reads one (`gpu/vk/compat/longshot_env.h`): `1`, `on`, `yes`, `true` on;
/// empty, `0`, `off`, `no`, `false` off (any case, spaces around); `None` for any other word. The one parser of the
/// builder's switches: what an unknown word means is each switch's own default.
pub fn switch(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "yes" | "true" => Some(true),
        "" | "0" | "off" | "no" | "false" => Some(false),
        _ => None,
    }
}
pub use sys::{
    detached, hidden_console, join_job, lower_priority, machine_registry, message_box, raise_window_of_child, run_game,
    run_game_with, running_programs, stay_awake, user_registry,
};
