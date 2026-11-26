//! The compile's scheduler: up to `workers` jobs at once, each let in by the memory gate with its own need, the
//! longest first.
//!
//! The order is the caller's (the longest first: no long job starts late and runs alone at the end). When the first job
//! waiting does not fit in the memory free now, the next that does starts instead, so the CPUs stay busy while the big
//! one waits for room; the first is tried again as each job ends, and the memory free is read again every second (a
//! program opened during the build makes later jobs wait). Below the gate's floor a job always starts.

use build_core::memory::{Gate, Ticket};
use std::sync::mpsc;
use std::time::Duration;

/// Runs `order`'s jobs (indexes) with `run`, at most `workers` at once, each when the gate lets its need
/// (`needs[job]`, MB) in; `done` gets each result in the caller's thread as it ends. `done` returning false stops the
/// starts (the jobs running still end, and their results still come). Returns the jobs never started.
pub fn run<R: Send>(
    workers: usize,
    gate: Option<&Gate>,
    order: &[usize],
    needs: &[u64],
    run: impl Fn(usize, Option<&Ticket<'_>>) -> R + Sync,
    mut done: impl FnMut(usize, R) -> bool,
) -> Vec<usize> {
    let workers = workers.max(1);
    let mut waiting: Vec<usize> = order.to_vec();
    let (tx, rx) = mpsc::channel::<(usize, R)>();
    std::thread::scope(|scope| {
        let run = &run;
        let mut running = 0usize;
        let mut go = true;
        loop {
            // (start what fits, while there are workers free)
            while go && running < workers && !waiting.is_empty() {
                let pick = match gate {
                    None => Some((0, None)),
                    Some(g) => {
                        let wanted: Vec<(u64, u64)> = waiting.iter().map(|&j| (j as u64, needs[j])).collect();
                        g.enter_first(&wanted).map(|(i, t)| (i, Some(t)))
                    }
                };
                let Some((i, ticket)) = pick else { break };
                let job = waiting.remove(i);
                running += 1;
                let tx = tx.clone();
                scope.spawn(move || {
                    let r = run(job, ticket.as_ref());
                    drop(ticket); // (out of the gate before the result is said: the next start may take its room)
                    let _ = tx.send((job, r));
                });
            }
            if running == 0 && (!go || waiting.is_empty()) {
                break;
            }
            // (a job's end, or a second: the memory free may have grown)
            if let Ok((job, r)) = rx.recv_timeout(Duration::from_secs(1)) {
                running -= 1;
                go &= done(job, r);
                while let Ok((job, r)) = rx.try_recv() {
                    running -= 1;
                    go &= done(job, r);
                }
            }
        }
    });
    waiting
}

/// The compile's time left with `workers` compilers: the work spread over them, the longest job still left as the floor
/// (each is one compiler).
pub fn eta(pending: &[f64], workers: usize) -> f64 {
    let longest = pending.iter().copied().fold(0.0, f64::max);
    longest.max(pending.iter().sum::<f64>() / workers.max(1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};

    #[test]
    fn every_job_runs_once_and_no_more_than_the_workers_at_once() {
        let inside = Mutex::new((0usize, 0usize));
        let mut seen = Vec::new();
        let left = run(
            3,
            None,
            &(0..12).collect::<Vec<_>>(),
            &[0; 12],
            |j, _| {
                {
                    let mut g = inside.lock().unwrap();
                    g.0 += 1;
                    g.1 = g.1.max(g.0);
                }
                std::thread::sleep(Duration::from_millis(5));
                inside.lock().unwrap().0 -= 1;
                j * 2
            },
            |j, r| {
                assert_eq!(r, j * 2);
                seen.push(j);
                true
            },
        );
        assert!(left.is_empty());
        seen.sort();
        assert_eq!(seen, (0..12).collect::<Vec<_>>());
        assert!(inside.lock().unwrap().1 <= 3);
    }

    #[test]
    fn a_job_too_big_for_now_lets_smaller_ones_by_and_starts_when_there_is_room() {
        // (1000 MB in all: job 0 holds 700, job 1 needs 600 and waits, jobs 2 and 3 of 100 fit beside job 0. Job 0
        // holds on until both small ones have started, so what happens follows from the gate alone, not from timing.)
        let gate = Gate::with_probes(0, Some(1000), 1, || None, |_| None);
        let started = Mutex::new(Vec::new());
        let small_in = (Mutex::new(0usize), Condvar::new());
        let first_done = AtomicBool::new(false);
        run(
            4,
            Some(&gate),
            &[0, 1, 2, 3],
            &[700, 600, 100, 100],
            |j, _| {
                started.lock().unwrap().push((j, first_done.load(Ordering::SeqCst)));
                match j {
                    0 => {
                        let (n, cv) = &small_in;
                        let guard = n.lock().unwrap();
                        // (bounded: a scheduler that never starts them fails the checks below, not by hanging)
                        drop(cv.wait_timeout_while(guard, Duration::from_secs(30), |n| *n < 2).unwrap());
                        first_done.store(true, Ordering::SeqCst);
                    }
                    2 | 3 => {
                        *small_in.0.lock().unwrap() += 1;
                        small_in.1.notify_all();
                    }
                    _ => {}
                }
            },
            |_, _| true,
        );
        let s = started.lock().unwrap();
        // (each job says it started from its own thread, so the order they say it in is not the order they were let
        // in: what the gate decides is who started while job 0 held its room, and who only after)
        let before: Vec<usize> = s.iter().filter(|x| !x.1).map(|x| x.0).collect();
        let after: Vec<usize> = s.iter().filter(|x| x.1).map(|x| x.0).collect();
        assert_eq!(before.len(), 3, "jobs 0, 2 and 3 ran together: {s:?}");
        assert!(before.contains(&2) && before.contains(&3), "the small ones went by the big one waiting: {s:?}");
        assert_eq!(after, [1], "job 1 started once job 0 left: {s:?}");
    }

    #[test]
    fn a_stop_starts_nothing_more() {
        let left = run(1, None, &[0, 1, 2], &[0; 3], |_, _| (), |_, _| false);
        assert_eq!(left, [1, 2]);
    }

    #[test]
    fn the_time_left_is_the_work_spread_or_the_longest() {
        assert_eq!(eta(&[10.0, 1.0, 1.0], 4), 10.0);
        assert_eq!(eta(&[4.0, 4.0, 4.0, 4.0], 2), 8.0);
        assert_eq!(eta(&[], 2), 0.0);
    }
}
