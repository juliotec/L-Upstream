//! The memory gate: a big job (a compiler on a lifted unit, a prompts' texture batch, the shader preparation) starts
//! only when the memory free holds it.
//!
//! One compiler per CPU thread runs PCs out of memory (the kernel stops a compiler, or the whole build), and a count low
//! enough for the biggest files leaves most threads idle. So a job starts when the memory free, less what the jobs
//! running may still take on top of what they hold now, keeps its own need and a reserve
//! (`LONGSHOT_MEM_RESERVE_GB`, 1.5 GB) free. Below a floor jobs always run: on Windows up to 4 (half the threads on
//! smaller CPUs), as Windows pages rather than fails; on Linux one, as Linux stops what does not fit and the build
//! compiles it again alone. With `LONGSHOT_MEM_LIMIT_GB` the player chooses: a job starts when the needs of those
//! running and its own fit in that, whatever is free.
//!
//! A build has one gate for all its jobs ([`Gate::from_env`] once), so its limit is one limit. A job beside the step's
//! own ([`Gate::enter_beside`]: the shader preparation beside the compile) counts in the room like the others but not
//! toward the floor, and goes in when it fits or the gate is empty: it never holds the compile below its floor, and
//! never waits for room the PC does not have.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// The memory free now, in MB (Linux: `MemAvailable`; Windows: the available physical memory), if it can be told.
pub fn free_mb() -> Option<u64> {
    crate::sys::available_memory().map(|b| b >> 20)
}

/// The memory a process and the processes it started hold now, in MB (a compiler driver that runs its compiler as a
/// child, ccache), if it can be told.
pub fn tree_mb(pid: u32) -> Option<u64> {
    crate::sys::tree_resident(pid).map(|b| b >> 20)
}

/// `LONGSHOT_MEM_LIMIT_GB` in MB: `None` for unset, not a number, or not above 0 (then the memory free decides).
pub fn limit_from(value: Option<&str>) -> Option<u64> {
    let gb: f64 = value?.trim().parse().ok()?;
    (gb.is_finite() && gb > 0.0).then_some((gb * 1024.0) as u64)
}

struct Lease {
    need: u64,
    pid: Option<u32>,
    /// A job beside the step's own ([`Gate::enter_beside`]): not counted toward the floor.
    beside: bool,
}

struct Running {
    next: u64,
    leases: HashMap<u64, Lease>,
    /// The first choice that did not fit, by its job's id, and since when it waits.
    head: Option<(u64, std::time::Instant)>, // (the job's id, and since when)
}

impl Running {
    /// The step's own jobs inside (the floor counts these).
    fn own(&self) -> usize {
        self.leases.values().filter(|l| !l.beside).count()
    }
}

/// How long the first choice of [`Gate::enter_first`] waits while smaller jobs go by it: past that, none does, so the
/// jobs running end and its room comes (else the longest job of a compile could wait to the end, the small ones
/// always taking the room as it came free).
pub const PATIENCE: Duration = Duration::from_secs(10);

/// The gate, shared by the workers of a step.
pub struct Gate {
    running: Mutex<Running>,
    changed: Condvar,
    reserve: u64,
    limit: Option<u64>,
    floor: usize,
    free: fn() -> Option<u64>,
    held: fn(u32) -> Option<u64>,
    patience: Duration,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("reserve", &self.reserve)
            .field("limit", &self.limit)
            .field("floor", &self.floor)
            .finish()
    }
}

/// A job let through the gate: it leaves when this is dropped.
pub struct Ticket<'g> {
    gate: &'g Gate,
    id: u64,
}

impl Ticket<'_> {
    /// The job's process, once started: what it holds is then counted as it grows.
    pub fn started(&self, pid: u32) {
        let mut r = self.gate.running.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(l) = r.leases.get_mut(&self.id) {
            l.pid = Some(pid);
        }
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        let mut r = self.gate.running.lock().unwrap_or_else(|p| p.into_inner());
        r.leases.remove(&self.id);
        self.gate.changed.notify_all();
    }
}

impl Gate {
    /// The gate as the environment sets it (`LONGSHOT_MEM_RESERVE_GB`, `LONGSHOT_MEM_LIMIT_GB`), with this system's
    /// floor.
    pub fn from_env() -> Gate {
        let reserve = std::env::var("LONGSHOT_MEM_RESERVE_GB")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|g| g.is_finite() && *g >= 0.0)
            .unwrap_or(1.5);
        let floor = if cfg!(windows) { (crate::jobs::cpus() / 2).clamp(1, 4) } else { 1 };
        Gate::new((reserve * 1024.0) as u64, limit_from(std::env::var("LONGSHOT_MEM_LIMIT_GB").ok().as_deref()), floor)
    }

    /// A gate keeping `reserve` MB free, or holding the jobs' needs under `limit` MB, never below `floor` jobs.
    pub fn new(reserve: u64, limit: Option<u64>, floor: usize) -> Gate {
        Gate::with_probes(reserve, limit, floor, free_mb, tree_mb)
    }

    /// As [`Gate::new`], with the memory read by `free` and `held` (for tests).
    pub fn with_probes(
        reserve: u64,
        limit: Option<u64>,
        floor: usize,
        free: fn() -> Option<u64>,
        held: fn(u32) -> Option<u64>,
    ) -> Gate {
        Gate {
            running: Mutex::new(Running { next: 0, leases: HashMap::new(), head: None }),
            changed: Condvar::new(),
            reserve,
            limit,
            floor: floor.max(1),
            free,
            held,
            patience: PATIENCE,
        }
    }

    /// This gate with another patience for its first choice ([`PATIENCE`]).
    pub fn with_patience(mut self, patience: Duration) -> Gate {
        self.patience = patience;
        self
    }

    /// The MB a new job may take now; `None`: no bound (the memory free cannot be told).
    fn room(&self, r: &Running) -> Option<u64> {
        if let Some(limit) = self.limit {
            return Some(limit.saturating_sub(r.leases.values().map(|l| l.need).sum::<u64>()));
        }
        let free = (self.free)()?;
        // (what the running jobs may still take, on top of what they hold now)
        let owed: u64 = r.leases.values().map(|l| l.need.saturating_sub(l.pid.and_then(self.held).unwrap_or(0))).sum();
        Some(free.saturating_sub(owed).saturating_sub(self.reserve))
    }

    #[cfg(test)]
    fn fits(&self, r: &Running, need: u64) -> bool {
        self.room(r).is_none_or(|room| room >= need)
    }

    fn admit(&self, r: &mut Running, need: u64, beside: bool) -> Ticket<'_> {
        let id = r.next;
        r.next += 1;
        r.leases.insert(id, Lease { need, pid: None, beside });
        Ticket { gate: self, id }
    }

    /// Waits until a job that needs `need` MB fits, and lets it in.
    pub fn enter(&self, need: u64) -> Ticket<'_> {
        let mut r = self.running.lock().unwrap_or_else(|p| p.into_inner());
        while r.own() >= self.floor && self.room(&r).is_some_and(|room| room < need) {
            // (the memory free is read again every second: a program opened during the build makes later jobs wait)
            r = self.changed.wait_timeout(r, Duration::from_secs(1)).unwrap_or_else(|p| p.into_inner()).0;
        }
        self.admit(&mut r, need, false)
    }

    /// A job beside the step's own (the shader preparation beside the compile) let in: once `need` MB fits (its need
    /// at most the limit, when there is one), or the gate is empty; `None` when `stop` says so first. It does not
    /// count toward the floor, so the step's own jobs still run while it is in.
    pub fn enter_beside(&self, need: u64, stop: &dyn Fn() -> bool) -> Option<Ticket<'_>> {
        let need = self.limit.map_or(need, |l| need.min(l));
        let mut r = self.running.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if stop() {
                return None;
            }
            if r.leases.is_empty() || self.room(&r).is_none_or(|room| room >= need) {
                return Some(self.admit(&mut r, need, true));
            }
            r = self.changed.wait_timeout(r, Duration::from_millis(200)).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// Without waiting: the first of `needs` (MB each, in the caller's order of preference) that fits now, let in, with
    /// its index; `None` when none does. The memory free is read once for all of them, so a scheduler can look past a
    /// job too big for now to the next one that fits (the compile: the longest first, the CPUs kept busy meanwhile);
    /// but only for [`PATIENCE`]: once the first choice has waited that long, nothing else goes by it.
    pub fn enter_first(&self, jobs: &[(u64, u64)]) -> Option<(usize, Ticket<'_>)> {
        let mut r = self.running.lock().unwrap_or_else(|p| p.into_inner());
        let &(head_id, head) = jobs.first()?;
        let room = if r.own() < self.floor { None } else { self.room(&r) };
        let first = match room {
            Some(room) if head > room => {
                // (the same job still first: its wait goes on; another job of the same need starts its own)
                let since = match r.head {
                    Some((id, t)) if id == head_id => t,
                    _ => {
                        let now = std::time::Instant::now();
                        r.head = Some((head_id, now));
                        now
                    }
                };
                if since.elapsed() >= self.patience {
                    return None;
                }
                jobs.iter().position(|&(_, n)| n <= room)?
            }
            _ => 0,
        };
        if first == 0 {
            r.head = None;
        }
        Some((first, self.admit(&mut r, jobs[first].1, false)))
    }

    /// Waits until a job leaves the gate, or `timeout` passes (the memory free may have changed meanwhile).
    pub fn wait(&self, timeout: Duration) {
        let r = self.running.lock().unwrap_or_else(|p| p.into_inner());
        let _ = self.changed.wait_timeout(r, timeout);
    }

    /// How many jobs are inside now.
    pub fn inside(&self) -> usize {
        self.running.lock().unwrap_or_else(|p| p.into_inner()).leases.len()
    }

    /// How many jobs of `each` MB the memory free now is likely to hold at once, between the floor and `jobs`: the
    /// compile's time estimate.
    pub fn expected(&self, jobs: usize, each: u64) -> usize {
        let room = match self.limit {
            Some(limit) => limit,
            None => match (self.free)() {
                Some(free) => free.saturating_sub(self.reserve),
                None => return jobs,
            },
        };
        ((room / each.max(1)) as usize).clamp(self.floor.min(jobs), jobs.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_limit_holds_the_needs_and_the_floor_always_runs() {
        let gate = Gate::with_probes(0, Some(1000), 1, || None, |_| None);
        let a = gate.enter(800); // (alone: below the floor it runs whatever it needs)
        let inside = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|s| {
            let (gate, flag) = (&gate, inside.clone());
            s.spawn(move || {
                let _b = gate.enter(400); // (800 + 400 is over 1000: it waits for a)
                flag.store(1, Ordering::SeqCst);
            });
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(inside.load(Ordering::SeqCst), 0, "the second job waited");
            drop(a);
        });
        assert_eq!(inside.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_memory_free_less_what_the_running_still_take() {
        // 4000 MB free, 500 kept: a job holding 100 of its 2000 still takes 1900, so 1600 is left
        let gate = Gate::with_probes(500, None, 1, || Some(4000), |_| Some(100));
        let first = gate.enter(2000);
        first.started(7);
        let r = gate.running.lock().unwrap();
        assert!(gate.fits(&r, 1600));
        assert!(!gate.fits(&r, 1601));
    }

    #[test]
    fn a_preparation_whose_game_is_given_its_ticket_is_not_counted_twice() {
        // (6000 MB free once the game holds its 3000 of a 3000 need, 500 kept: the room is 5500. Its ticket not told
        // the game's pid, its whole need would come off the free again, which already lacks it: 2500)
        let gate = Gate::with_probes(500, None, 1, || Some(6_000), |pid| (pid == 42).then_some(3_000));
        let prep = gate.enter_beside(3_000, &|| false).expect("in");
        {
            let r = gate.running.lock().unwrap();
            assert_eq!(gate.room(&r), Some(2_500), "not started yet: its need still owed");
        }
        prep.started(42);
        let r = gate.running.lock().unwrap();
        assert_eq!(gate.room(&r), Some(5_500), "what it holds is in the free already: nothing owed on top");
    }

    #[test]
    fn a_job_beside_the_step_counts_in_the_room_not_the_floor() {
        // (a 16 GB limit: the preparation's 9.5 GB goes in beside 6 GB of compiles, and the compile then has what is left)
        let gate = Gate::with_probes(0, Some(16_000), 1, || None, |_| None);
        let a = gate.enter(6_000);
        let prep = gate.enter_beside(9_500, &|| false).expect("it fits");
        let r = gate.running.lock().unwrap();
        assert!(gate.fits(&r, 500) && !gate.fits(&r, 501), "one limit for both");
        drop(r);
        drop(a);
        // (the floor counts the compile's own: one runs whatever the preparation holds)
        let alone = gate.enter(15_000);
        drop((alone, prep));
        // (a preparation bigger than the limit is held to it, and goes in once the compile's jobs are out)
        let small = Gate::with_probes(0, Some(8_000), 1, || None, |_| None);
        let job = small.enter(1_000);
        let stopped = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            let waiting = s.spawn(|| small.enter_beside(9_500, &|| stopped.load(Ordering::SeqCst)).map(|t| t.id));
            std::thread::sleep(Duration::from_millis(100));
            assert!(!waiting.is_finished(), "1 GB in: 8 GB does not fit");
            drop(job);
            assert!(waiting.join().unwrap().is_some(), "the gate empty: in");
        });
        // (no room the PC has: in only once the gate is empty, and out when the build stops)
        let tiny = Gate::with_probes(0, None, 1, || Some(4_000), |_| None);
        let job = tiny.enter(100);
        std::thread::scope(|s| {
            let waiting = s.spawn(|| tiny.enter_beside(9_500, &|| stopped.load(Ordering::SeqCst)).is_none());
            std::thread::sleep(Duration::from_millis(100));
            stopped.store(true, Ordering::SeqCst);
            assert!(waiting.join().unwrap(), "stopped while it waited");
        });
        drop(job);
    }

    #[test]
    fn the_variable_s_limit() {
        assert_eq!(limit_from(Some("8")), Some(8192));
        assert_eq!(limit_from(Some("0")), None);
        assert_eq!(limit_from(Some("x")), None);
        assert_eq!(limit_from(None), None);
    }

    #[test]
    fn a_first_choice_that_waited_its_patience_is_gone_by_no_more() {
        let gate = Gate::with_probes(0, Some(1000), 1, || None, |_| None).with_patience(Duration::from_millis(50));
        let (_, a) = gate.enter_first(&[(0, 600)]).unwrap();
        // (800 does not fit beside 600: 100s go by it, until it has waited its patience)
        let (i, b) = gate.enter_first(&[(1, 800), (2, 100)]).unwrap();
        assert_eq!(i, 1);
        drop(b);
        std::thread::sleep(Duration::from_millis(60));
        assert!(gate.enter_first(&[(1, 800), (2, 100)]).is_none(), "past its patience, the 100 waits too");
        drop(a);
        let (i, _c) = gate.enter_first(&[(1, 800), (2, 100)]).unwrap();
        assert_eq!(i, 0, "the room came: the first choice goes in");
    }

    #[test]
    fn a_new_first_choice_of_the_same_need_waits_its_own_patience() {
        let gate = Gate::with_probes(0, Some(1000), 1, || None, |_| None).with_patience(Duration::from_millis(50));
        let (_, _a) = gate.enter_first(&[(0, 600)]).unwrap();
        assert_eq!(gate.enter_first(&[(1, 800), (2, 100)]).unwrap().0, 1);
        std::thread::sleep(Duration::from_millis(60));
        // (job 1 went another way, and job 3, of the same 800, is first now: not yet past its patience)
        assert_eq!(gate.enter_first(&[(3, 800), (4, 100)]).map(|t| t.0), Some(1));
    }

    #[test]
    fn the_first_that_fits_is_let_in_past_a_bigger_one() {
        let gate = Gate::with_probes(0, Some(1000), 1, || None, |_| None);
        let (i, a) = gate.enter_first(&[(0, 900), (1, 100)]).unwrap(); // (below the floor: the first, whatever it needs)
        assert_eq!(i, 0);
        let (i, b) = gate.enter_first(&[(0, 900), (1, 100)]).unwrap(); // (900 + 900 is over 1000: 100 fits)
        assert_eq!(i, 1);
        assert!(gate.enter_first(&[(0, 900), (2, 200)]).is_none());
        assert_eq!(gate.inside(), 2);
        drop((a, b));
        assert_eq!(gate.inside(), 0);
    }
}
