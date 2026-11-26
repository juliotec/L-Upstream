//! (Linux) Out of memory. The kernel's OOM killer ends the game with SIGKILL: no handler runs, so the end is unclean,
//! and systemd then stops the game's whole scope (DefaultOOMPolicy=stop), the builder with it unless it left that
//! scope. So [`Watch::start`], once the game has started: the game's cgroup's `memory.events` `oom_kill` count read,
//! then the builder moved into a scope of its own (`longshot-crash-<pid>.scope`, through the user manager's
//! StartTransientUnit, as busctl asks it; not possible in a Flatpak). [`Watch::oom`] at the end: whether the count grew.

use std::path::PathBuf;

/// The memory.events file of the cgroup in /proc/<pid>/cgroup's text (cgroup v2: "0::<path>").
pub fn cgroup_events(cgroup: &str) -> Option<PathBuf> {
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::/"))?.trim_end_matches('\r');
    let mut p = PathBuf::from("/sys/fs/cgroup");
    if !path.is_empty() {
        p.push(path);
    }
    Some(p.join("memory.events"))
}

/// memory.events' oom_kill count.
pub fn oom_kills(events: &str) -> Option<i64> {
    events.lines().find_map(|l| l.strip_prefix("oom_kill ")).and_then(|v| v.trim().parse().ok())
}

/// The game's OOM count at its start.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    events: Option<PathBuf>,
    before: Option<i64>,
}

impl Watch {
    /// Once the game (`pid`) has started: its count read, then the builder out of its scope (at most 2 s; nothing
    /// elsewhere than Linux).
    pub fn start(pid: u32) -> Watch {
        if !cfg!(target_os = "linux") {
            return Watch::default();
        }
        let events = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().and_then(|c| cgroup_events(&c));
        let before = events.as_ref().and_then(|e| std::fs::read_to_string(e).ok()).and_then(|t| oom_kills(&t));
        own_scope();
        Watch { events, before }
    }

    /// Whether the OOM killer took a process of the game's cgroup since: read at once when the game has ended (its
    /// cgroup goes with its scope a moment later).
    pub fn oom(&self) -> bool {
        let after = self.events.as_ref().and_then(|e| std::fs::read_to_string(e).ok()).and_then(|t| oom_kills(&t));
        matches!((self.before, after), (Some(b), Some(a)) if a > b)
    }
}

fn own_scope() {
    let pid = std::process::id().to_string();
    let unit = format!("longshot-crash-{pid}.scope");
    let args = [
        "--user",
        "--quiet",
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        &unit,
        "fail",
        "2",
        "PIDs",
        "au",
        "1",
        &pid,
        "CollectMode",
        "s",
        "inactive-or-failed",
        "0",
    ];
    let Ok(mut child) = std::process::Command::new("busctl")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("LD_PRELOAD")
        .spawn()
    else {
        return;
    };
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(2) {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cgroups_count() {
        assert_eq!(
            cgroup_events("0::/user.slice/app.slice/app-flatpak-x-1.scope\n"),
            Some(PathBuf::from("/sys/fs/cgroup/user.slice/app.slice/app-flatpak-x-1.scope/memory.events"))
        );
        assert_eq!(cgroup_events("0::/\n"), Some(PathBuf::from("/sys/fs/cgroup/memory.events")), "the root cgroup");
        assert_eq!(cgroup_events("12:memory:/x\n"), None, "cgroup v1 only: no count");
        assert_eq!(oom_kills("low 0\nhigh 0\nmax 12\noom 3\noom_kill 2\noom_group_kill 0\n"), Some(2));
        assert_eq!(oom_kills("low 0\n"), None);
    }
}
