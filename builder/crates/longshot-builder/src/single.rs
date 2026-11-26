//! One copy per profile: play holds its profile ([`build_core::lock::ProfileLock`], `<profile>/.longshot.lock`) while
//! it builds and while the game runs, so a second start for the same profile (a second click on the icon, Steam's Play
//! again, the game started by itself while one runs) never starts a second game writing the same saves. Other profiles
//! play at once.
//!
//! The second start, under `--output user`: the running game's window brought forward (Windows; Linux: not done), then
//! exit status 0 with a short message (a message box when there was no window to bring: the game is still being built).
//! Under the other outputs: said (stderr; `--output json` or `recomplauncher`: an `error` event, `profile_in_use`), exit
//! status [`IN_USE`] (recomplauncher.json's `exit_codes` explain it).

use build_core::lock::ProfileLock;
use build_core::report::{Event, Reporter};
use std::path::Path;

/// Exit status: the profile is in use (its game runs, or is being built for it, from another start).
pub const IN_USE: i32 = 123;

/// What a second start does, for this run's output.
#[derive(Debug, PartialEq, Eq)]
pub enum Second {
    /// A person's: the game's window brought forward when there is one; exit status 0.
    Raise,
    /// A launcher's, a terminal's: said, exit status [`IN_USE`].
    Refuse,
}

/// What a second start does under `output`.
pub fn second(output: crate::output::Output) -> Second {
    if output.windows() { Second::Raise } else { Second::Refuse }
}

/// The sentence for a second start of profile `profile`.
pub fn in_use_text(profile: &Path) -> String {
    format!(
        "Longshot is already running for the profile {}: close the game (or wait for its build to end) before starting \
         it again.",
        profile.display()
    )
}

/// Profile `profile` held for this run: the lock, which this run keeps until it ends; `None` when the lock can't be
/// made (a profile folder that can't be written: said, and the game plays without it). The exit status this run stops
/// with when another start holds it.
pub fn hold(profile: &Path) -> Result<Option<ProfileLock>, i32> {
    match ProfileLock::take(profile) {
        Ok(Some(lock)) => Ok(Some(lock)),
        Ok(None) => Err(said(profile, crate::output::chosen())),
        Err(e) => {
            eprintln!("longshot: the profile can't be locked ({e}): a second start of it isn't stopped");
            Ok(None)
        }
    }
}

/// A second start of profile `profile` said under `output`: its exit status.
fn said(profile: &Path, output: crate::output::Output) -> i32 {
    let text = in_use_text(profile);
    match second(output) {
        Second::Raise => {
            // (the builder that holds it started the game: its window brought forward. None (still building, or the
            // game not yet up): a box says why nothing else happens)
            let raised = ProfileLock::holder(profile).is_some_and(build_core::raise_window_of_child);
            if raised {
                eprintln!("longshot: {text} Its window is brought to the front.");
            } else {
                crate::play::say(&text, false, true);
            }
            0
        }
        Second::Refuse => {
            eprintln!("longshot: {text}");
            if output.format() != build_core::report::Format::Text {
                Reporter::stdout(output.format()).send(Event::Error { code: "profile_in_use", text });
            }
            IN_USE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Output;

    #[test]
    fn a_person_s_second_start_brings_the_game_forward_and_a_launcher_s_is_refused() {
        assert_eq!(second(Output::User), Second::Raise);
        for o in [Output::Text, Output::Json, Output::RecompLauncher] {
            assert_eq!(second(o), Second::Refuse, "{o:?}");
        }
        assert!(in_use_text(Path::new("/p/ann")).contains("/p/ann"));
        assert!(![0, 1, 2, crate::play::NOT_BUILT, crate::play::FAILED].contains(&IN_USE));
    }

    #[test]
    fn a_second_start_of_a_held_profile_is_refused_with_its_own_status() {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("single-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let first = hold(&d.join("ann")).unwrap().expect("free");
        // (the second start, from another thread, as the output a launcher or a terminal gives)
        let ann = d.join("ann");
        let status = std::thread::spawn(move || match ProfileLock::take(&ann).unwrap() {
            Some(_) => 0,
            None => said(&ann, Output::Text),
        })
        .join()
        .unwrap();
        assert_eq!(status, IN_USE);
        assert!(hold(&d.join("bo")).unwrap().is_some(), "another profile plays at once");
        drop(first);
        // (a lock is an open file description: a test running beside this one that starts a process gives the child a
        // copy of it for the moments before its exec, so the lock lets go a few microseconds after the drop, not at it:
        // tried again until then)
        let mut again = None;
        for _ in 0..200 {
            again = ProfileLock::take(&d.join("ann")).unwrap();
            if again.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(again.is_some(), "free again once the first ends");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
