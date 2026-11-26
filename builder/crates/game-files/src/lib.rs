//! The player's game files, as the builder reads them (the read stage of `longshot-builder`).
//!
//! - [`nup`]: the .nup package (a decrypted Switch 2 dump): listed, verified, its exefs unpacked, and its romfs indexed
//!   for the game to read in place ([`romfs`]): the 26 GB are not copied;
//! - [`nso`]: the code modules, loaded (LZ4 and ZBIC segments, RELR relocations) or unpacked in place;
//! - [`zstd`]: zstd and ZBIC frames, decoded whole;
//! - [`checks`]: what a dump must be before a build starts (the modules, the supported build, the romfs's files, links,
//!   paths in the game folder), and `check`'s answer;
//! - [`patches`]: the mods' ExeFS patches, collected, listed and refused where they would stop a hook site;
//! - [`mod_layout`]: the mods in a profile left in the emulators' layout, which need importing as manifest mods;
//! - [`title`]: the game's own name and icon;
//! - [`disk`]: free space, and whether a folder is on a hard drive.
//!
//! Every failure a player can act on is a [`build_core::Error::Player`] with a stable code.

pub mod checks;
pub mod disk;
mod le;
pub mod mod_layout;
pub mod nso;
pub mod nup;
pub mod patches;
pub mod romfs;
pub mod title;
pub mod zstd;

#[cfg(test)]
mod testdata;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
