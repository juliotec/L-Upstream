//! What a person sees of a build with `--output user` (output.rs): one small seam, [`UserUi`], that the build's steps,
//! its errors and its end go through. Its one implementation for now, [`Stderr`], says the steps on stderr and an error
//! in the system's message box (play.rs's `say`); the builder's own windows (Slint, with the crash window) come behind
//! the same trait.

use build_core::report::{Event, Stage};

/// A person's view of a build: its step and how far it is, an error, its end.
pub trait UserUi: Send {
    /// The step's sentence and how far the whole build is (0 to 1).
    fn step(&mut self, text: &str, fraction: f64);
    /// The build stopped: why, for the player.
    fn error(&mut self, text: &str);
    /// The build is done.
    fn done(&mut self);
}

/// A build event as a person's view shows it: the step's sentence (`step`) and how far (`fraction`: the whole
/// build's; the shader preparation, after the build's own steps, its own). Whether the event changed either.
pub fn shown(event: &Event, step: &mut String, fraction: &mut f64) -> bool {
    match event {
        Event::Stage { text, .. } => *step = text.clone(),
        Event::Progress { stage: Stage::Shaders, fraction: f, .. } => *fraction = f.clamp(0.0, 1.0),
        Event::Progress { overall, .. } => *fraction = overall.clamp(0.0, 1.0),
        _ => return false,
    }
    true
}

/// [`UserUi`] on stderr: a line each time the step or the whole percent changes; an error in a message box.
#[derive(Default)]
pub struct Stderr {
    last: Option<(String, u32)>,
}

impl Stderr {
    /// The line said for a step and a fraction, when it differs from the last one said.
    fn line(&mut self, text: &str, fraction: f64) -> Option<String> {
        let pct = (fraction.clamp(0.0, 1.0) * 100.0).floor() as u32;
        let now = (text.to_string(), pct);
        if self.last.as_ref() == Some(&now) {
            return None;
        }
        self.last = Some(now);
        Some(format!("longshot: {text} ({pct} %)"))
    }
}

impl UserUi for Stderr {
    fn step(&mut self, text: &str, fraction: f64) {
        if let Some(line) = self.line(text, fraction) {
            eprintln!("{line}");
        }
    }

    fn error(&mut self, text: &str) {
        crate::play::say(text, true, true);
    }

    fn done(&mut self) {}
}

/// The person's view for this run.
pub fn open() -> Box<dyn UserUi> {
    if crate::output::chosen().windows() {
        return Box::new(crate::after::Windows);
    }
    Box::new(Stderr::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_sees_the_step_and_the_whole_build_s_fraction() {
        let (mut step, mut f) = (String::new(), 0.0);
        let stage = Event::Stage { stage: Stage::Compile, text: "Compiling the game".into() };
        assert!(shown(&stage, &mut step, &mut f));
        assert_eq!(step, "Compiling the game");
        assert!(shown(&Event::Progress { stage: Stage::Compile, fraction: 0.5, overall: 0.565 }, &mut step, &mut f));
        assert_eq!(f, 0.565);
        // (the shaders: their own fraction, the build's overall held at 1)
        assert!(shown(&Event::Progress { stage: Stage::Shaders, fraction: 0.25, overall: 1.0 }, &mut step, &mut f));
        assert_eq!(f, 0.25);
        assert!(!shown(&Event::Log { text: "x".into() }, &mut step, &mut f));
    }

    #[test]
    fn stderr_says_each_change_once() {
        let mut s = Stderr::default();
        assert_eq!(s.line("Compiling the game", 0.561).as_deref(), Some("longshot: Compiling the game (56 %)"));
        assert_eq!(s.line("Compiling the game", 0.569), None, "the same percent");
        assert_eq!(s.line("Compiling the game", 0.575).as_deref(), Some("longshot: Compiling the game (57 %)"));
        assert_eq!(s.line("Preparing the shaders", 0.575).as_deref(), Some("longshot: Preparing the shaders (57 %)"));
    }
}
