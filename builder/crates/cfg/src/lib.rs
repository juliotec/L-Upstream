//! Discovery: the game's functions and their control flow, found from the image: a function is an entry and every
//! instruction reachable from it without crossing another entry; a BR's jump table is recovered from the code before
//! it; PLT stubs are seen through; a setjmp's return addresses are entries of their own.
//!
//! The result is a [`crate::plan::Plan`]: the functions, the jump tables, the PLT stubs and the setjmp owners the lift reads.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let image = image::Image::load("work/sa/image".as_ref())?;
//! let found = cfg::discover(&image, cfg::entries::inputs(&image)?);
//! println!("{} functions", found.functions.len());
//! # Ok(()) }
//! ```

mod classify;
pub mod entries;
pub mod module;
pub mod plan;
mod table;
mod walk;

pub use classify::Kind;
pub use walk::{Discovery, Inputs, Stats};

/// The plan of a whole discovery of `image` from `inputs` (its image path left empty: the caller knows it).
pub fn discover(image: &image::Image, inputs: Inputs) -> crate::plan::Plan {
    let mut d = Discovery::new(image, inputs);
    d.run();
    d.plan(std::path::Path::new(""))
}
