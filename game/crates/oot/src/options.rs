//! The game's option ids the Rust reads (game/options.h, where every id of the game's is; the values must be the same,
//! tests/test_oot_fov_rust.py checks them).

/// Gameplay's vertical field of view, percent of the game's current angle; 100 is unchanged.
pub const FOV: i32 = 51;
