//! The Rust the game links, as one staticlib (`liblongshot_rs.a`): it re-exports the game's crates so their
//! `extern "C"` functions are in the library. Nothing is defined here; each crate it names is linked in whole.
//!
//! The rules for what it links:
//! - one staticlib for the whole program (this one): two would each carry their own `std`;
//! - built with the workspace's `game` profile (`panic = "abort"`): a panic stops the game, it never unwinds into C;
//! - no `extern "C"` function lets a panic out (with abort, none can).

/// Ocarina of Time's hook site callbacks (game/crates/oot): `hos_oot_fov`.
pub use oot;
