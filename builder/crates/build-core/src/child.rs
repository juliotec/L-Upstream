//! The programs a build starts (the compilers, the linkers, the shader preparation, zstd, curl): none outlives the
//! builder.
//!
//! A build stopped from outside (its window closed, a launcher's Cancel, a crash, a kill) must not leave twenty
//! compilers or a 9 GB shader preparation running with nobody to wait for them. So every child is tied to the builder
//! as it is started:
//!
//! - **Linux**: the child asks the kernel for SIGKILL when the thread that started it ends (`PR_SET_PDEATHSIG`, set
//!   between the fork and the exec). It is the thread, not the process: every caller here waits on its child on the
//!   thread that started it, and must keep doing so.
//! - **Windows**: the builder puts itself, at its start ([`crate::join_job`]), in a job object it opens once and never
//!   closes (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`): every process it starts is in the job from its creation, and every
//!   process those start, atomically; when the builder ends, however it ends, Windows closes the job's last handle and
//!   ends what is in it. That matters for llvm-mingw's toolchain, whose `clang` and `ld.lld` are small wrapper
//!   programs that start the real tool as their first act: a child put in the job only once started could leave that
//!   one out. Where Windows refuses the builder a job (one around it that takes none inside it, before Windows 8),
//!   each child is put in the job once started instead, and a wrapper's own child may then escape it. Its children
//!   also open no console window (`CREATE_NO_WINDOW`), a safety net beside [`crate::hidden_console`].
//! - Elsewhere nothing ties them.
//!
//! What the builder starts for the player to keep (the game itself, `longshot-builder play`) is started with
//! [`std::process::Command`], not with these, and on Windows breaks away from the job ([`crate::detached`]; the job
//! allows it, `JOB_OBJECT_LIMIT_BREAKAWAY_OK`).

use std::ffi::OsStr;
use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

/// A command for `program`, tied to the builder once it is started with [`spawn`], [`output`] or [`status`].
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    crate::sys::tie(&mut c);
    c
}

/// `cmd` started (made with [`command`]), and on Windows put in the builder's job.
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    let child = cmd.spawn()?;
    crate::sys::adopt(&child);
    Ok(child)
}

/// `cmd` run to its end, its output kept: as [`Command::output`], stdin closed and stdout and stderr read (a caller's
/// own choice of either is replaced).
pub fn output(cmd: &mut Command) -> io::Result<Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    spawn(cmd)?.wait_with_output()
}

/// `cmd` run to its end: its status, as [`Command::status`] (what the builder's own standard handles are, its are).
pub fn status(cmd: &mut Command) -> io::Result<ExitStatus> {
    spawn(cmd)?.wait()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn a_child_ends_with_the_thread_that_started_it() {
        // (a child that would sleep a minute, started on a thread that then ends: the kernel kills it)
        let pid = std::thread::spawn(|| {
            let mut c = command("sleep");
            c.arg("60").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let child = spawn(&mut c).unwrap();
            let pid = child.id();
            std::mem::forget(child); // (not waited on here: the test's own check below)
            pid
        })
        .join()
        .unwrap();
        let t = std::time::Instant::now();
        let gone = || {
            // (a killed child of this process stays a zombie until reaped: dead either way)
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map_or(true, |s| s.rsplit(')').next().is_some_and(|r| r.trim_start().starts_with('Z')))
        };
        while !gone() {
            assert!(t.elapsed() < std::time::Duration::from_secs(10), "the child outlived its thread");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    #[cfg(unix)]
    fn output_and_status_run_the_program() {
        let mut c = command("sh");
        c.args(["-c", "echo out; echo err >&2; exit 3"]);
        let o = output(&mut c).unwrap();
        assert_eq!(
            (o.stdout.as_slice(), o.stderr.as_slice(), o.status.code()),
            (&b"out\n"[..], &b"err\n"[..], Some(3))
        );
        assert!(status(command("true").stdout(Stdio::null())).unwrap().success());
    }
}
