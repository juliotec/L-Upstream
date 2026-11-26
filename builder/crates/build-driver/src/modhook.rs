//! The `mod` command's one dispatch point: `longshot-builder mod <anything...>` (and `Longshot.exe mod ...`) calls
//! [`run`] with the arguments after `mod` and a [`BuilderCx`], what the builder already knows. Here [`run`] is a stub
//! that says the tools are not built in ([`MISSING_EXIT`], the `mod_tooling_missing` error); the Mod API's `modtool`
//! crate fills it (the same extension pattern as [`crate::play::mods_stamp`]).
//!
//! The builder takes its own flags before the subcommand (`--profile`, `--output`, `--cache`), and passes the rest
//! untouched: `longshot-builder mod --output json check ./my-mod --types`.

use build_core::Error;
use build_core::report::{Format, Reporter};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Exit status: this builder has no mod tooling (`mod_tooling_missing`). 120 to 123 are play's
/// ([`crate::play`]'s docs); 124 is what a Windows console close ends a process with.
pub const MISSING_EXIT: u8 = 125;

/// The error code of [`MISSING_EXIT`] (docs/dev/reference/build-events.md).
pub const MISSING_CODE: &str = "mod_tooling_missing";

/// What a mod command says when this builder has no mod tooling.
pub const MISSING_TEXT: &str = "mod tooling not built in to this builder";

/// How a command speaks (`--output`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// JSON Lines events, for a launcher.
    Json,
    /// Text, never a window.
    Text,
    /// Text, a progress window and error boxes.
    User,
    /// RecompLauncher's progress and error lines.
    RecompLauncher,
}

impl Output {
    /// `--output`'s value, or the error's text.
    pub fn parse(text: &str) -> Result<Output, String> {
        match text {
            "json" => Ok(Output::Json),
            "text" => Ok(Output::Text),
            "user" => Ok(Output::User),
            "recomplauncher" => Ok(Output::RecompLauncher),
            other => Err(format!("--output: {other:?} is not json, text, user or recomplauncher")),
        }
    }

    /// The events' format on stdout.
    pub fn format(self) -> Format {
        match self {
            Output::Json => Format::Json,
            Output::RecompLauncher => Format::Lines,
            Output::Text | Output::User => Format::Text,
        }
    }

    /// Whether windows are shown (the progress window, message boxes).
    pub fn windows(self) -> bool {
        self == Output::User
    }
}

/// Environment variables as pairs, name and value.
pub type Env = Vec<(OsString, OsString)>;

/// A request to play a profile with extras: what `mod run` asks of the builder's play path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayRequest {
    /// The profile folder to play (absolute, or relative to the working folder).
    pub profile: PathBuf,
    /// Mod folders linked in for this run only (a mod under development): the game gets them in
    /// [`crate::play::DEV_MODS_VAR`], joined by the OS's path separator.
    pub extra_mod_dirs: Vec<PathBuf>,
    /// The game's own arguments (passed after a `--`: the builder reads none of them).
    pub game_args: Vec<OsString>,
    /// More environment variables for the game, over the builder's own.
    pub env: Env,
}

/// What the player is asked to confirm in a window of the builder's own ([`BuilderCx::confirm`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Confirmation {
    /// The window's title.
    pub title: String,
    /// The question.
    pub body: String,
    /// What the answer rests on, name and value: the mod, its source, its code's hash, the permissions asked.
    pub facts: Vec<(String, String)>,
}

/// What the builder knows when it runs a mod command, resolved as `play` resolves them.
pub struct BuilderCx<'a> {
    /// The profile folder: `--profile`'s (absolute), else `profile/` beside the program.
    pub profile_dir: PathBuf,
    /// The game input of the profile's profile.json (the copy it names, absolute); none when the profile is not there,
    /// is unreadable or names no copy.
    pub game_input: Option<PathBuf>,
    /// The version folder: the program's own.
    pub version_dir: PathBuf,
    /// The builder's cache (`--cache`, `LONGSHOT_CACHE`, else the per-user one).
    pub cache_dir: PathBuf,
    /// How the command speaks (`--output`).
    pub output: Output,
    /// Plays a profile with extras: the normal play path (the fast check, the update when it says so, the game, the
    /// crash and restart handling). Its exit status is the game's, or play's own (120 to 123); a status over 255 (a
    /// Windows game's) is [`crate::play::FAILED`].
    pub play: &'a dyn Fn(PlayRequest) -> ExitCode,
    /// Asks the player in a window the builder owns, and says whether they accepted. The only way trust or a permission
    /// is granted: what asks for it (the game, over its channel) only asks. With no window possible (`--output text` or
    /// `json`, no display) it says why and answers false.
    pub confirm: &'a dyn Fn(&Confirmation) -> bool,
}

impl<'a> BuilderCx<'a> {
    /// The context for the program in `version`: `profile` is `--profile`'s (none: `profile/` beside the program), `cache`
    /// the cache folder. A profile that is not there or does not read has no `game_input`.
    pub fn resolve(
        version: &Path,
        profile: Option<&Path>,
        cache: PathBuf,
        output: Output,
        play: &'a dyn Fn(PlayRequest) -> ExitCode,
        confirm: &'a dyn Fn(&Confirmation) -> bool,
    ) -> BuilderCx<'a> {
        let profile_dir = match profile {
            Some(d) => std::path::absolute(d).unwrap_or_else(|_| d.to_path_buf()),
            None => crate::play::default_profile(version),
        };
        let game_input = crate::play::Profile::read(&profile_dir).ok().flatten().and_then(|p| p.game_in(&profile_dir));
        BuilderCx {
            profile_dir,
            game_input,
            version_dir: version.to_path_buf(),
            cache_dir: cache,
            output,
            play,
            confirm,
        }
    }

    /// How events are written (`--output`).
    pub fn format(&self) -> Format {
        self.output.format()
    }
}

/// The folders `dirs` as [`crate::play::DEV_MODS_VAR`]'s value (the OS's path separator), or the reason one cannot be.
pub fn dev_mods_value(dirs: &[PathBuf]) -> Result<OsString, String> {
    std::env::join_paths(dirs).map_err(|e| format!("a mod folder has the path separator in its name: {e}"))
}

/// `mod`'s arguments (after `mod` and the builder's flags): this builder has no mod tooling, so it says so and ends
/// with [`MISSING_EXIT`]. The Mod API replaces the body.
pub fn run(_args: &[OsString], cx: &BuilderCx) -> ExitCode {
    ExitCode::from(missing(cx))
}

/// The ops this builder serves on the game's channel beyond its built-in ones ([`crate::channel`]): what `hello` lists.
/// None here: the Mod API's crate names its own and answers them in [`serve`].
pub fn ops() -> &'static [&'static str] {
    &[]
}

/// A request that is not a built-in op, from the game's channel ([`crate::channel`]) or the command line: its result, or
/// why not. `events` says progress as it goes (every event carries the request's id). The stub refuses every op with
/// `mod_tooling_missing`, as [`run`] does; the Mod API's crate answers its own ops and says `unknown_op` to others.
/// Everything the game sends may be forged, so an op does only what its name says, checks its arguments itself, and
/// never grants trust (`cx.confirm` asks the player in a window).
pub fn serve(
    _req: &crate::channel::Request,
    _cx: &BuilderCx,
    _events: &dyn Fn(crate::channel::Event),
) -> Result<serde_json::Value, crate::channel::OpError> {
    Err(crate::channel::OpError::new(MISSING_CODE, MISSING_TEXT))
}

/// [`run`]'s stub: said as an event, the status it ends with.
fn missing(cx: &BuilderCx) -> u8 {
    Reporter::stdout(cx.format()).error(&Error::player("mod_tooling_missing", MISSING_TEXT));
    MISSING_EXIT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_play(_: PlayRequest) -> ExitCode {
        ExitCode::SUCCESS
    }

    fn no_confirm(_: &Confirmation) -> bool {
        false
    }

    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("modhook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_stub_says_the_tooling_is_missing_with_its_own_status() {
        let cx = BuilderCx::resolve(Path::new("/v"), None, "/c".into(), Output::Json, &no_play, &no_confirm);
        assert_eq!(missing(&cx), MISSING_EXIT);
        assert_eq!(MISSING_EXIT, 125);
        assert!(![120, 121, 122, 123, 124].contains(&MISSING_EXIT));
        assert_eq!(MISSING_TEXT, "mod tooling not built in to this builder");
        let _ = run(&[], &cx);
    }

    #[test]
    fn a_context_resolves_the_profile_and_its_game() {
        let d = scratch("resolve");
        let version = d.join("v");
        let none = BuilderCx::resolve(&version, None, d.join("cache"), Output::Text, &no_play, &no_confirm);
        assert_eq!(none.profile_dir, version.join("profile"), "beside the program");
        assert_eq!((none.game_input, none.version_dir.as_path()), (None, version.as_path()));
        assert_eq!(none.cache_dir, d.join("cache"));
        // (--profile: that folder, its profile.json's game)
        let prof = d.join("ann");
        std::fs::create_dir_all(&prof).unwrap();
        let nup = d.join("Game.nup");
        std::fs::write(&nup, b"nup").unwrap();
        std::fs::write(
            prof.join("profile.json"),
            format!(r#"{{"v":1,"name":"Ann","game":{:?}}}"#, nup.to_str().unwrap()),
        )
        .unwrap();
        let cx = BuilderCx::resolve(&version, Some(&prof), d.join("cache"), Output::User, &no_play, &no_confirm);
        assert_eq!((cx.profile_dir.as_path(), cx.game_input.as_deref()), (prof.as_path(), Some(nup.as_path())));
        assert_eq!(cx.output, Output::User);
        assert_eq!(cx.format(), Format::Text);
        // (a profile that does not read: no game, not a failure)
        std::fs::write(prof.join("profile.json"), b"not json").unwrap();
        assert_eq!(
            BuilderCx::resolve(&version, Some(&prof), d.join("c"), Output::Text, &no_play, &no_confirm).game_input,
            None
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_dev_mod_folders_are_joined_by_the_os_separator() {
        let dirs = [PathBuf::from("/m/a"), PathBuf::from("/m/b")];
        let v = dev_mods_value(&dirs).unwrap();
        assert_eq!(std::env::split_paths(&v).collect::<Vec<_>>(), dirs);
        assert_eq!(dev_mods_value(&[]).unwrap(), OsString::new());
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert!(dev_mods_value(&[PathBuf::from(format!("/a{sep}b"))]).is_err());
    }

    /// The variable's name is the game's: host/longshot_profile.h's `LONGSHOT_DEV_MODS`.
    #[test]
    fn the_variable_is_the_one_the_game_side_names() {
        let header = include_str!("../../../../host/longshot_profile.h");
        let line = format!("#define LONGSHOT_DEV_MODS \"{}\"", crate::play::DEV_MODS_VAR);
        assert!(header.contains(&line), "host/longshot_profile.h lacks {line}");
    }
}
