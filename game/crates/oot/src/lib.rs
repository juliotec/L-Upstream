//! Ocarina of Time's code in Rust: the hook sites' callbacks (game/sites/sites.toml, game/sites.def), each in place of
//! the C it replaced, under the same symbol, so the site and the lift do not change. Safe code only: guest memory and the
//! host through the `guest` crate.
//!
//! - [`fov`]: Field of view (the site `camera.lens`, `hos_oot_fov`), the first one: it was host/oot_fov.c.

pub mod fov;
pub mod options;
