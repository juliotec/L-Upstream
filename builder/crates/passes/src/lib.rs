//! Transformations of the mid IR, between the semantics ([`sem`](../sem)) and the LLVM IR writer, in the order
//! the lifter runs them:
//!
//! - [`share_marks`]: the page marks of stores through one value of a base register share one mark;
//! - [`simplify`]: the guest state's reads and writes forwarded and the dead ones removed, so that LLVM has much
//!   less to read;
//! - [`split()`]: a function too big for the compiler cut into pieces at block boundaries.

mod marks;
mod simplify;
mod split;

pub use marks::{MarkStats, share_marks};
pub use simplify::{SimplifyStats, dead_values, dead_writes, forward, share_pure, simplify};
pub use split::{BACK_WEIGHT, SYNC_WEIGHT, SplitOptions, cost, split};

#[cfg(test)]
mod tests;
