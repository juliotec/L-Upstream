//! How many workers a step of the build may run.
//!
//! A count given (`--jobs`, `LONGSHOT_JOBS`) is a ceiling for every step that works side by side: the lift, the
//! discovery's walks, the prompts' textures, the compile, the shader preparation. A launcher's "Build workers" choice
//! reaches all of them, not the compile alone (LongshotLauncher issues 148 and 163: the PC unusable while it built).
//! Unset, the compile takes one worker per CPU thread but one; the memory gate ([`crate::memory`]) holds back the big
//! ones either way.

use crate::{Error, Result};

/// The words a bad count is refused with.
pub const BAD_COUNT: &str = "LONGSHOT_JOBS must be a whole number: use 1 or more, or 0 for automatic.";

/// A count as given: `Some(n)` for 1 or more, `None` for automatic (empty or 0); an error for anything else.
pub fn requested(value: Option<&str>) -> Result<Option<usize>> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    match v.parse::<i64>() {
        Ok(0) => Ok(None),
        Ok(n) if n > 0 => Ok(Some(n as usize)),
        _ => Err(Error::player("bad_jobs", BAD_COUNT)),
    }
}

/// The CPU threads this process may run on.
pub fn cpus() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}

/// The default: one per CPU thread but one, so the PC stays usable while it builds (the launcher's Automatic does the
/// same).
pub fn default_jobs() -> usize {
    cpus().saturating_sub(1).max(1)
}

/// The workers of this build: `given` (`--jobs`) when it is 1 or more, else `LONGSHOT_JOBS`, else [`default_jobs`].
pub fn chosen(given: Option<usize>, env: Option<&str>) -> Result<usize> {
    match given.filter(|&n| n > 0) {
        Some(n) => Ok(n),
        None => Ok(requested(env)?.unwrap_or_else(default_jobs)),
    }
}

/// A step's own count: its variable (`LONGSHOT_LIFT_JOBS`, `LONGSHOT_PROMPT_JOBS`) when set, under the build's
/// ceiling, else the step's own `natural` count under it.
pub fn for_step(ceiling: Option<usize>, own: Option<&str>, natural: usize) -> Result<usize> {
    let n = requested(own)?.unwrap_or(natural).max(1);
    Ok(ceiling.map_or(n, |c| n.min(c.max(1))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_1_or_more_or_automatic() {
        assert_eq!(requested(None).unwrap(), None);
        assert_eq!(requested(Some(" ")).unwrap(), None);
        assert_eq!(requested(Some("0")).unwrap(), None);
        assert_eq!(requested(Some("6")).unwrap(), Some(6));
        assert!(requested(Some("-1")).is_err());
        assert!(requested(Some("two")).is_err());
    }

    #[test]
    fn given_wins_then_the_variable_then_the_default() {
        assert_eq!(chosen(Some(3), Some("5")).unwrap(), 3);
        assert_eq!(chosen(None, Some("5")).unwrap(), 5);
        assert_eq!(chosen(Some(0), None).unwrap(), default_jobs());
        assert_eq!(for_step(Some(4), None, 12).unwrap(), 4);
        assert_eq!(for_step(Some(4), Some("2"), 12).unwrap(), 2);
        assert_eq!(for_step(None, None, 12).unwrap(), 12);
    }
}
