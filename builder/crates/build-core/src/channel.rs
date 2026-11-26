//! The pipe pair between the builder and the game it starts (docs/dev/reference/build.md, "The game's channel to its
//! builder"): two anonymous pipes, one each way, whose game ends only that one child inherits.
//!
//! - The builder's ends ([`ServerEnd`]) are made close-on-exec (Linux) and non-inheritable (Windows): nothing the builder
//!   starts has them, the game's own start aside.
//! - The game's ends ([`ChildEnd`]) are made inheritable by [`ChildEnd::prepare`] for the one command given: Linux, in
//!   the child after the fork (`pre_exec`), so no other child, started at the same time or after, has them; Windows,
//!   marked inheritable until [`ChildEnd`] is dropped (std's `Command` has no handle list on a stable compiler, so a
//!   program started in that moment would inherit them too: the builder starts nothing else while it starts the game).
//! - [`ChildEnd::env_value`] is what the game is given in [`ENV`]: `<read>,<write>`, the game's read end of the
//!   builder's replies and its write end of its requests, each the descriptor (Linux) or the handle value (Windows).
//!   The game makes both close-on-exec / non-inheritable at its start (host/builder_channel.c's `bc_claim`, called by
//!   hos_main.c after the hand-over), so what it starts has neither.

use std::io::{PipeReader, PipeWriter};

/// The environment variable the game is given its ends in (host/builder_channel.h's `LONGSHOT_BUILDER_CHANNEL`).
pub const ENV: &str = "LONGSHOT_BUILDER_CHANNEL";

#[cfg(unix)]
type Raw = std::os::fd::OwnedFd;
#[cfg(windows)]
type Raw = std::os::windows::io::OwnedHandle;

/// The builder's ends: requests come in on `reader`, replies and events go out on `writer`.
#[derive(Debug)]
pub struct ServerEnd {
    /// What the game wrote.
    pub reader: PipeReader,
    /// What the builder says.
    pub writer: PipeWriter,
}

/// The game's ends, until it is started: drop this once it is (the builder's copies close then, so the game's end of
/// the pipes closing is seen as the end of the stream).
#[derive(Debug)]
pub struct ChildEnd {
    /// The game's read end (the builder's replies).
    read: Raw,
    /// The game's write end (its requests).
    write: Raw,
}

/// A new pair.
pub fn open() -> std::io::Result<(ServerEnd, ChildEnd)> {
    let (their_read, our_writer) = std::io::pipe()?;
    let (our_reader, their_write) = std::io::pipe()?;
    Ok((
        ServerEnd { reader: our_reader, writer: our_writer },
        ChildEnd { read: their_read.into(), write: their_write.into() },
    ))
}

#[cfg(unix)]
impl ChildEnd {
    /// The two descriptors' numbers, as [`ENV`]'s value.
    pub fn env_value(&self) -> String {
        use std::os::fd::AsRawFd;
        format!("{},{}", self.read.as_raw_fd(), self.write.as_raw_fd())
    }

    /// `cmd` given [`ENV`], and these two descriptors open in the process it starts, and no other.
    #[allow(unsafe_code)]
    pub fn prepare(&self, cmd: &mut std::process::Command) {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let (r, w) = (self.read.as_raw_fd(), self.write.as_raw_fd());
        cmd.env(ENV, self.env_value());
        // SAFETY: only fcntl runs in the child between fork and exec, which is async-signal-safe; it touches no memory
        // of the parent's.
        unsafe {
            cmd.pre_exec(move || {
                for fd in [r, w] {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
}

#[cfg(windows)]
impl ChildEnd {
    /// The two handles' values, as [`ENV`]'s value.
    pub fn env_value(&self) -> String {
        use std::os::windows::io::AsRawHandle;
        format!("{},{}", self.read.as_raw_handle() as usize, self.write.as_raw_handle() as usize)
    }

    /// `cmd` given [`ENV`], and these two handles inheritable until `self` is dropped.
    #[allow(unsafe_code)]
    pub fn prepare(&self, cmd: &mut std::process::Command) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        cmd.env(ENV, self.env_value());
        for h in [&self.read, &self.write] {
            // SAFETY: a handle this value owns, open for as long as it lives.
            unsafe { SetHandleInformation(h.as_raw_handle(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::process::Command;

    /// What a shell started with `cmd` lists of its open descriptors (`ls -l /proc/self/fd`: each pipe by its inode,
    /// `pipe:[1234]`, so another descriptor with the same number is not taken for it).
    fn fds_seen_by(cmd: &mut Command) -> String {
        let out = cmd.args(["-c", "ls -l /proc/self/fd"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// What descriptor `fd` of this process is (`pipe:[1234]`).
    fn link_of(fd: &str) -> String {
        std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn only_the_prepared_command_has_the_games_ends() {
        let (_server, child) = open().unwrap();
        let env = child.env_value();
        let (r, w) = env.split_once(',').unwrap();
        let (rl, wl) = (link_of(r), link_of(w));
        // (a command nobody prepared: neither pipe)
        let other = fds_seen_by(&mut Command::new("sh"));
        assert!(!other.contains(&rl) && !other.contains(&wl), "{other}");
        // (the prepared one: both)
        let mut mine = Command::new("sh");
        child.prepare(&mut mine);
        let seen = fds_seen_by(&mut mine);
        assert!(seen.contains(&rl) && seen.contains(&wl), "{seen} lacks {rl} or {wl}");
        // (and after it: another command still has neither)
        let other = fds_seen_by(&mut Command::new("sh"));
        assert!(!other.contains(&rl) && !other.contains(&wl));
    }

    #[test]
    fn a_child_talks_to_the_server_over_its_ends_and_the_builders_close_when_it_ends() {
        let (mut server, child) = open().unwrap();
        // (the game: reads a line from its read end, answers on its write end; the descriptors from the variable)
        // (bash: dash, CI's sh, takes no fd number above 9 in <&$r, and the channel's ends may be)
        let mut game = Command::new("bash");
        game.args([
            "-c",
            "r=${LONGSHOT_BUILDER_CHANNEL%,*}; w=${LONGSHOT_BUILDER_CHANNEL#*,}; \
             read line <&$r; echo \"got $line\" >&$w",
        ]);
        child.prepare(&mut game);
        let mut running = game.spawn().unwrap();
        drop(child);
        server.writer.write_all(b"hello\n").unwrap();
        let mut said = String::new();
        server.reader.read_to_string(&mut said).unwrap();
        assert_eq!(said, "got hello\n");
        assert!(running.wait().unwrap().success());
    }
}
