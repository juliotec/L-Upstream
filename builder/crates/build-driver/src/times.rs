//! The builds' times in a work folder (`build_times.json`): each build's total, its compile and its stages, the last
//! 20 kept, so the `done` event can say this build's time beside the last one's. (Each unit's own compile time is the
//! compile's: `build/compile_state.json`.)

use build_core::fsx;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const FORMAT: u32 = 1;
const KEEP: usize = 20;

/// One build's times.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildTime {
    /// When it ended, in seconds since 1970.
    pub when: u64,
    /// `build` or `update`.
    pub kind: String,
    /// Units compiled now, of all of them.
    pub compiled: (usize, usize),
    /// The compile's seconds.
    pub compile_seconds: f64,
    /// The whole build's.
    pub total_seconds: f64,
    /// Each stage's seconds.
    pub stages: BTreeMap<String, f64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    format: u32,
    builds: Vec<BuildTime>,
}

/// `build` added to the record at `path` (a record missing, half written or of another format is started again); the
/// last build's total before it.
pub fn record(path: &Path, build: BuildTime) -> Option<f64> {
    let mut saved = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
        .filter(|s| s.format == FORMAT)
        .unwrap_or_default();
    let previous = saved.builds.last().map(|b| b.total_seconds);
    saved.format = FORMAT;
    saved.builds.push(build);
    let n = saved.builds.len();
    saved.builds.drain(..n.saturating_sub(KEEP));
    // (a record not written costs only the next build's comparison)
    if let Ok(bytes) = serde_json::to_vec_pretty(&saved) {
        let _ = fsx::write_atomic(path, &bytes);
    }
    previous
}

/// Now, in seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A length of time the way the log says it: `1:02:03`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_builds_are_kept_and_the_previous_total_said() {
        let d = crate::scratch("times");
        let p = d.join("build_times.json");
        let b = |t: f64| BuildTime {
            when: 0,
            kind: "build".into(),
            compiled: (1, 2),
            compile_seconds: 1.0,
            total_seconds: t,
            stages: BTreeMap::new(),
        };
        assert_eq!(record(&p, b(10.0)), None);
        assert_eq!(record(&p, b(20.0)), Some(10.0));
        for i in 0..30 {
            record(&p, b(f64::from(i)));
        }
        let saved: Saved = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(saved.builds.len(), KEEP);
        assert_eq!(clock(3723.4), "1:02:03");
        let _ = std::fs::remove_dir_all(&d);
    }
}
