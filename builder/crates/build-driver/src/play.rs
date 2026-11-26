//! Play: the version folder players start Longshot from, and a profile (docs/dev/reference/build.md, "Play").
//!
//! ```text
//! <version>/Longshot.exe | Longshot.x86_64   the builder (a release's longshot-builder): the program players start
//! <version>/builds/<build>/                  a game folder it builds (its record, crate::folder::RECORD_FILE, inside):
//!                                            `default`, or the Mod API's stamp of a profile's code mods ([`mods_stamp`])
//! <version>/profile/                         the default profile (`--profile <dir>` names another, anywhere)
//! <profile>/profile.json                     the profile (docs/dev/reference/profiles.md): this builder reads and
//!                                            writes its `game` and `upscalers` alone ([`Profile`]), every other key
//!                                            kept; the game reads its `start_slot`
//! <profile>/settings.ini                     the game's settings ([`settings`])
//! <profile>/saves/                           the saves alone: save/, save-work/ ([`saves`])
//! <profile>/mods/                            its mods, manifest mods alone ([`mods`])
//! ```
//!
//! The game takes the last three from `LONGSHOT_PROFILE` (host/longshot_profile.h), and from nowhere else (a
//! developer's run plays a profile of its own). Its logs and caches stay in the game folder (`builds/<build>/logs/`, `cache/`), made again by
//! any build; the rest of its user folder (recordings, the mouse's sensitivity) stays in the per-user place, as a game
//! started by itself has it.
//!
//! Playing ([`play`]): the profile's profile.json read, then the fast check ([`check`], a stamp the last build for a
//! profile left in the game folder's record against now), an update only when that says so, then the game, given the
//! profile (`LONGSHOT_PROFILE`: the game takes its saves, settings and start slot from it, and the Mod API's its mods), started
//! again after an update whenever it asked for one over its channel and exited normally (a few times in a row at most).
//! The vendors' licences the player accepted are the profile's (`"licenses"`, each for the pin it was accepted for:
//! [`Profile::licenses`]): an update downloads a vendor's files only when the profile accepted them at this builder's pin.

use crate::folder::{Record, Stamp};
use crate::{BuildOptions, GameSource, Upscaler, build};
use build_core::{Error, Result};
use std::path::{Path, PathBuf};

/// The profile's file in a profile folder.
pub const PROFILE_FILE: &str = launcher_profile::FILE;
/// The folder of a version's game folders ([`game_folder`]).
pub const BUILDS: &str = "builds";
/// The game folder's name in [`BUILDS`] when no code mod asks for another build ([`mods_stamp`]: none).
pub const DEFAULT_BUILD: &str = "default";
/// The default profile's folder in a version folder.
pub const PROFILE: &str = "profile";
/// The profile's saves folder, save/ and save-work/ alone (host/longshot_profile.h's `longshot_profile_saves`).
pub const SAVES: &str = launcher_profile::SAVES;
/// The profile's settings file (host/longshot_profile.h's `longshot_profile_settings`).
pub const SETTINGS: &str = launcher_profile::SETTINGS;
/// The profile's mods folder: manifest mods alone (docs/dev/reference/mods.md).
pub const MODS: &str = launcher_profile::MODS;
/// The variable the game is given its profile in (absolute).
pub const PROFILE_VAR: &str = launcher_profile::SWITCH;
/// The variable a dev run gives the game its extra mod folders in (`longshot-builder mod run`, [`crate::modhook`]): the
/// folders joined by the OS's path separator (`:`, `;` on Windows), each a mod's folder to link in beside the profile's
/// mods. The Mod API's game side reads it (host/longshot_profile.h's `LONGSHOT_DEV_MODS`).
pub const DEV_MODS_VAR: &str = "LONGSHOT_DEV_MODS";
/// The variable `play --safe-start` gives the game (`1`), for that play's starts: the Mod API's game side starts no mod
/// then and says why (a player's way back in when a mod stops the game starting).
pub const SAFE_START_VAR: &str = "LONGSHOT_SAFE_START";
/// How many times in a row the game is built and started again after it asked for it over its channel
/// (`restart_to_apply`, [`crate::channel`]) and exited normally.
pub const MAX_RESTARTS: u32 = 3;

/// profile.json, as far as this builder plays it: the copy and the upscalers. Read and written by the one reader and
/// writer, the `launcher-profile` crate (docs/dev/reference/profiles.md): every other key (the launcher's `name`,
/// `start_slot`, the Mod API's `mods`, ...) is kept as it was, in its place.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    /// The player's copy: a .nup file, or a dump folder (`romfs/` and `exefs/`). Relative: to the profile folder.
    pub game: Option<PathBuf>,
    /// The upscalers built in (`dlss`, `fsr4`, `xess`). One whose vendor's licence the profile has not accepted at
    /// this builder's pin ([`Profile::licenses`]) is left out of an update, said.
    pub upscalers: Vec<Upscaler>,
    /// The file as it was read (or made), every key in it: [`Profile::write`] puts the copy and the upscalers above
    /// into it when they changed.
    pub file: launcher_profile::Profile,
}

/// The same copy and upscalers, and the same file once they are put in it (what [`Profile::write`] would write).
impl PartialEq for Profile {
    fn eq(&self, o: &Profile) -> bool {
        self.game == o.game
            && self.upscalers == o.upscalers
            && match (self.synced(), o.synced()) {
                (Ok(a), Ok(b)) => a == b,
                _ => self.file == o.file,
            }
    }
}

/// The builder's upscaler of the profile's.
fn upscaler_of(u: launcher_profile::Upscaler) -> Upscaler {
    match u {
        launcher_profile::Upscaler::Dlss => Upscaler::Dlss,
        launcher_profile::Upscaler::Fsr4 => Upscaler::Fsr4,
        launcher_profile::Upscaler::Xess => Upscaler::Xess,
    }
}

/// The profile's upscaler of the builder's.
fn upscaler_to(u: Upscaler) -> launcher_profile::Upscaler {
    match u {
        Upscaler::Dlss => launcher_profile::Upscaler::Dlss,
        Upscaler::Fsr4 => launcher_profile::Upscaler::Fsr4,
        Upscaler::Xess => launcher_profile::Upscaler::Xess,
    }
}

impl Profile {
    /// A new profile (`{"v":1,"name":"Default"}`) of copy `game`.
    pub fn new(game: PathBuf) -> Profile {
        let mut file = launcher_profile::Profile::new();
        file.set_string("name", Some("Default"));
        let mut p = Profile { game: Some(game), upscalers: Vec::new(), file };
        // (the copy in the file too, when it is text: a new profile reads back as made)
        if let Ok(f) = p.synced() {
            p.file = f;
        }
        p
    }

    /// Profile folder `dir`'s profile.json: `None` when it has none; the player told when it can't be read, is not
    /// one, or is of a newer format.
    pub fn read(dir: &Path) -> Result<Option<Profile>> {
        let path = dir.join(PROFILE_FILE);
        let bad = |why: String| {
            Error::player(
                "profile_unreadable",
                format!(
                    "{} is not a profile Longshot can read ({why}). Set the game up again from your launcher, or \
                     delete that file and start Longshot again to choose your game files.",
                    path.display()
                ),
            )
        };
        let file = match launcher_profile::Profile::read(&path) {
            Ok(f) => f,
            Err(launcher_profile::Error::Missing) => return Ok(None),
            Err(launcher_profile::Error::Io(e)) => return Err(Error::io(&path, std::io::Error::other(e))),
            // (a newer one may have keys of another meaning: never written over)
            Err(launcher_profile::Error::Newer(n)) => {
                return Err(bad(format!("format {n}, from a newer Longshot: update Longshot")));
            }
            Err(e) => return Err(bad(e.to_string())),
        };
        let game = match file.raw("game").map(|v| v.get()) {
            None | Some("null") => None,
            Some(_) => Some(PathBuf::from(file.game().ok_or_else(|| bad("its \"game\" is not text".into()))?)),
        };
        let upscalers = file
            .upscalers()
            .map_err(|name| bad(format!("its upscalers name {name:?}, which this Longshot does not know")))?
            .into_iter()
            .map(upscaler_of)
            .collect();
        Ok(Some(Profile { game, upscalers, file }))
    }

    /// The file with the copy and the upscalers put in, each only when it changed (a value that did not keeps its
    /// text). A copy whose path is not text can't be written.
    fn synced(&self) -> Result<launcher_profile::Profile> {
        let mut f = self.file.clone();
        match &self.game {
            Some(g) => {
                let text = g.to_str().ok_or_else(|| {
                    Error::player(
                        "profile_unreadable",
                        format!("{} can't be written in a profile: its name is not text", g.display()),
                    )
                })?;
                if f.game().as_deref() != Some(text) {
                    f.set_string("game", Some(text));
                }
            }
            None => f.remove("game"),
        }
        let had: Vec<Upscaler> = f.upscalers().unwrap_or_default().into_iter().map(upscaler_of).collect();
        if self.upscalers.is_empty() {
            f.remove("upscalers");
        } else if had != self.upscalers {
            f.set_upscalers(&self.upscalers.iter().copied().map(upscaler_to).collect::<Vec<_>>());
        }
        Ok(f)
    }

    /// Written as `dir`'s profile.json (whole or not at all), every key it was read with kept.
    pub fn write(&self, dir: &Path) -> Result<()> {
        let path = dir.join(PROFILE_FILE);
        self.synced()?.write(&path).map_err(|e| Error::io(&path, e))
    }

    /// The vendors whose licences the profile accepted for the files this builder downloads (`"licenses"`, each
    /// vendor's at its pin: [`crate::runtimes::Vendor::pin`]). One accepted for an earlier pin is asked again.
    pub fn licenses(&self) -> Vec<crate::runtimes::Vendor> {
        let file = self.file.licenses();
        crate::runtimes::Vendor::ALL.into_iter().filter(|v| file.get(v.name()).is_some_and(|p| p == v.pin())).collect()
    }

    /// `vendors`' licences accepted, each for this builder's pin (`--accept-licenses` for a profile), in the file
    /// once it is written ([`Profile::write`]).
    pub fn accept(&mut self, vendors: &[crate::runtimes::Vendor]) {
        let file = self.file.licenses();
        for v in vendors {
            if file.get(v.name()).is_none_or(|p| p != v.pin()) {
                self.file.set_license(v.name(), v.pin());
            }
        }
    }

    /// The player's copy, for profile folder `dir`.
    pub fn game_in(&self, dir: &Path) -> Option<PathBuf> {
        self.game.as_ref().map(|g| dir.join(g))
    }

    /// The key of what of the profile decides the build (the copy as resolved, the upscalers; never a key it does not
    /// know), for profile folder `dir`.
    pub fn key(&self, dir: &Path) -> String {
        let game = self.game_in(dir).map(|g| std::path::absolute(&g).unwrap_or(g)).unwrap_or_default();
        let mut ups: Vec<&str> = self.upscalers.iter().map(|u| u.name()).collect();
        ups.sort();
        let mut k = build_core::key::Key::new();
        k.text("game", &game.to_string_lossy()).text("upscalers", &ups.join(","));
        k.hex()
    }
}

/// The Mod API's stamp of the mods that need a build (code mods) in profile `profile` (its profile.json's mods and its
/// trust.json's accepted code), for the version folder at `version`: it names the game folder ([`game_folder`]), and
/// the check compares it with the one the last update recorded. The extension point of the Mod API stack (its layer
/// over the `launcher-profile` crate computes it); none here: every profile plays `builds/default`.
pub fn mods_stamp(_profile: &Path, _version: &Path) -> Option<String> {
    None
}

/// The game folder of the version folder at `version` for the code mods' stamp `mods`: `builds/<stamp>`, or
/// `builds/default`.
pub fn game_folder(version: &Path, mods: Option<&str>) -> PathBuf {
    version.join(BUILDS).join(mods.unwrap_or(DEFAULT_BUILD))
}

/// The default profile of the version folder at `version`.
pub fn default_profile(version: &Path) -> PathBuf {
    version.join(PROFILE)
}

/// The saves folder of profile `profile`: its `saves/`.
pub fn saves(profile: &Path) -> PathBuf {
    profile.join(SAVES)
}

/// The settings file of profile `profile`: its `settings.ini`.
pub fn settings(profile: &Path) -> PathBuf {
    profile.join(SETTINGS)
}

/// The mods folder of profile `profile`: its `mods/`.
pub fn mods(profile: &Path) -> PathBuf {
    profile.join(MODS)
}

/// Whether folder `dir` has a save: a `save/` folder with something in it.
fn has_save(dir: &Path) -> bool {
    std::fs::read_dir(dir.join("save")).is_ok_and(|mut d| d.next().is_some())
}

/// A user folder's settings file ([`carry_saves`]: the profile's [`settings`] when it has none).
const OPTIONS_FILE: &str = "longshot_options.ini";

/// Copies a player's earlier saves into profile `profile`'s empty `saves/`, once, keeping them where they were: from
/// the first of `from` that has a save (a game folder's `user/` an earlier play made, the per-user place a game moved
/// them to). Its `save/` into `saves/save/`, and its settings (`longshot_options.ini`) as the profile's `settings.ini`
/// when the profile has none; never `save-work/` (a write cut halfway, which the game in the old place finishes), nor
/// anything else. Where they came from, when they did.
pub fn carry_saves(profile: &Path, from: &[PathBuf]) -> Option<PathBuf> {
    let to = saves(profile);
    if std::fs::read_dir(&to).is_ok_and(|mut d| d.next().is_some()) {
        return None;
    }
    let old = from.iter().find(|f| has_save(f))?;
    // (into a folder of its own first, renamed whole: a copy cut halfway is never the profile's saves)
    let part = profile.join(".saves-copying");
    let _ = std::fs::remove_dir_all(&part);
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let kind = e.file_type()?;
            if kind.is_dir() {
                copy_tree(&e.path(), &to.join(e.file_name()))?;
            } else if kind.is_file() {
                std::fs::copy(e.path(), to.join(e.file_name()))?;
            }
        }
        Ok(())
    }
    let done = copy_tree(&old.join("save"), &part.join("save")).and_then(|_| {
        // (the settings first, whole: the saves' folder renamed in is what says the copy was done)
        let (from, to) = (old.join(OPTIONS_FILE), settings(profile));
        if from.is_file() && !to.exists() {
            let bytes = std::fs::read(&from)?;
            build_core::fsx::write_atomic(&to, &bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(())
    });
    let done = done.and_then(|_| {
        let _ = std::fs::remove_dir(&to);
        std::fs::rename(&part, &to)
    });
    if done.is_err() {
        let _ = std::fs::remove_dir_all(&part);
        return None;
    }
    Some(old.clone())
}

/// Where a player's saves were before profiles, for [`carry_saves`]: the user folders of the game folders an earlier
/// builder made in the version folder at `version` (`Longshot/`, `game/`, `builds/default/`), then the per-user place
/// the game moved them to (`%APPDATA%\Longshot` on Windows, `$XDG_DATA_HOME/longshot` or `~/.local/share/longshot` on
/// Linux), `get` the environment.
pub fn earlier_saves(version: &Path, get: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        ["Longshot", "game", "builds/default"].iter().map(|f| version.join(f).join("user")).collect();
    if cfg!(windows) {
        v.extend(get("APPDATA").map(|a| Path::new(&a).join("Longshot")));
    } else {
        let data = get("XDG_DATA_HOME").filter(|x| x.starts_with('/')).map(PathBuf::from);
        let home = || get("HOME").filter(|h| h.starts_with('/')).map(|h| Path::new(&h).join(".local/share"));
        v.extend(data.or_else(home).map(|d| d.join("longshot")));
    }
    v
}

// ---- the fast check

/// The stamp of the player's copy at `game`: its size, its write time and (Linux) its device and inode, of the .nup,
/// or of a dump's `exefs/main` and its `romfs/` folder. Empty when it can't be read (then never the stamp of a build).
pub fn game_stamp(game: &Path) -> String {
    let one = |p: &Path| -> Option<String> {
        let m = std::fs::metadata(p).ok()?;
        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            format!(" {}:{}", m.dev(), m.ino())
        };
        #[cfg(not(unix))]
        let id = "";
        Some(format!("{} {t}{id}", m.len()))
    };
    let parts =
        if game.is_dir() { vec![game.join("exefs").join("main"), game.join("romfs")] } else { vec![game.into()] };
    parts.iter().map(|p| one(p)).collect::<Option<Vec<_>>>().map(|v| v.join("; ")).unwrap_or_default()
}

/// The stamp a build of profile `p` (at `profile`) in the version folder at `version` by builder `builder` (its version
/// and source: [`check`]'s) records.
pub fn stamp(version: &Path, profile: &Path, p: &Profile, builder: &str) -> Stamp {
    Stamp {
        builder: builder.to_string(),
        profile: p.key(profile),
        game: p.game_in(profile).map(|g| game_stamp(&g)).unwrap_or_default(),
        mods: mods_stamp(profile, version),
        patches: patches_stamp(profile),
    }
}

/// The ExeFS patches the build of profile `profile` takes (crate::read::manifest_mods, the enabled ones, in order):
/// each mod's name and each patch file's name and SHA-256, hashed; empty for none. A patch added, changed, removed,
/// switched off or moved in the order changes it, so the next start builds the game again ([`Stale::Mods`]).
pub fn patches_stamp(profile: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut any = false;
    for (name, dir, on) in crate::read::manifest_mods(Some(profile)) {
        if !on {
            continue;
        }
        for file in game_files::patches::patch_files(&dir) {
            let Ok(bytes) = std::fs::read(dir.join("exefs").join(&file)) else { continue };
            h.update(format!("{name}\t{file}\t").as_bytes());
            h.update(Sha256::digest(&bytes));
            h.update(b"\n");
            any = true;
        }
    }
    if any { build_core::key::to_hex(&h.finalize()) } else { String::new() }
}

/// Why a game folder needs an update before the game starts ([`check`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// Never built (no record), or its game's program is missing.
    NotBuilt,
    /// Built without a stamp (by `build` or `update` for no profile, or an earlier builder).
    NoStamp,
    /// By another builder (a new release).
    Builder,
    /// The profile's copy or upscalers changed (another profile's, sharing the folder, among them).
    Profile,
    /// The player's copy changed (size, time, file).
    Game,
    /// The mods that need a build changed ([`mods_stamp`]).
    Mods,
    /// The shaders are to be prepared again (a new driver, another GPU: crate::shaders::would_prepare).
    Shaders,
    /// The game asked for it (`restart_to_apply` over its channel, [`crate::channel`]) and exited normally.
    Asked,
}

impl Stale {
    /// Its words, for the log.
    pub fn why(self) -> &'static str {
        match self {
            Stale::NotBuilt => "not built yet",
            Stale::NoStamp => "built for no profile",
            Stale::Builder => "another builder",
            Stale::Profile => "the profile's game files or upscalers changed",
            Stale::Game => "the game files changed",
            Stale::Mods => "the mods that need a build changed",
            Stale::Shaders => "the shaders are to be prepared again",
            Stale::Asked => "the game asked for a build",
        }
    }
}

/// The game's program in game folder `folder`: `longshot` (`longshot.exe` on Windows), or in its `bin/`.
pub fn game_program(folder: &Path) -> PathBuf {
    let name = if cfg!(windows) { "longshot.exe" } else { "longshot" };
    let (top, bin) = (folder.join(name), folder.join("bin").join(name));
    if !top.is_file() && bin.is_file() { bin } else { top }
}

/// The fast check: whether the game folder profile `p` (at `profile`) plays in the version folder at `version` is up
/// to date for builder `builder` (its release and source, or a developer's build's own stamp), else why not. A few
/// file reads and stats: the record's stamp against now ([`stamp`]), then the shaders' key (`get`: the environment,
/// crate::env).
pub fn check(
    version: &Path,
    profile: &Path,
    p: &Profile,
    builder: &str,
    get: impl Fn(&str) -> Option<String>,
) -> Option<Stale> {
    let now = stamp(version, profile, p, builder);
    let folder = game_folder(version, now.mods.as_deref());
    let program = game_program(&folder);
    let Ok(rec) = Record::read(&folder) else { return Some(Stale::NotBuilt) };
    if !program.is_file() {
        return Some(Stale::NotBuilt);
    }
    let Some(was) = rec.stamp else { return Some(Stale::NoStamp) };
    if was.builder != now.builder {
        return Some(Stale::Builder);
    }
    if was.profile != now.profile {
        return Some(Stale::Profile);
    }
    if now.game.is_empty() || was.game != now.game {
        return Some(Stale::Game);
    }
    if (now.mods.is_some() && was.mods != now.mods) || was.patches != now.patches {
        return Some(Stale::Mods);
    }
    let windows = program.extension().is_some_and(|e| e == "exe");
    let level = crate::folder::folder_compile_shaders(&folder).unwrap_or(crate::CompileShaders::Most);
    let shaders_dir = program.parent().unwrap_or(&folder);
    if crate::shaders::would_prepare(shaders_dir, windows, level, get) {
        return Some(Stale::Shaders);
    }
    None
}

/// The options of the build of profile `p` (at `profile`) in the version folder at `version` by builder `builder`:
/// the profile's copy and upscalers; its game folder ([`game_folder`]); the profile, whose manifest mods the build
/// takes (BuildOptions::profile); the licences the profile accepted ([`Profile::licenses`]); its language (an update,
/// when it was built before: crate::build::update_options); the stamp it records.
pub fn options(version: &Path, profile: &Path, p: &Profile, builder: &str) -> Result<BuildOptions> {
    let stamp = stamp(version, profile, p, builder);
    let folder = game_folder(version, stamp.mods.as_deref());
    let Some(game) = p.game_in(profile) else {
        return Err(Error::player(
            "profile_unreadable",
            format!(
                "{} names no game files: choose your copy of the game (--game <your .nup file or dump folder>).",
                profile.join(PROFILE_FILE).display()
            ),
        ));
    };
    let game = GameSource::from_path(&game)?;
    let mut o = match Record::read(&folder) {
        Ok(_) => {
            let mut o = build::update_options(&folder)?;
            o.game = game;
            o
        }
        Err(_) => BuildOptions::new(game, crate::layout::game_folder(&folder)),
    };
    o.upscalers = p.upscalers.clone();
    o.licenses = p.licenses();
    o.profile = Some(std::path::absolute(profile).unwrap_or_else(|_| profile.to_path_buf()));
    o.stamp = Some(stamp);
    Ok(o)
}

// ---- play

/// What [`play`] asks of the program around it: the update and the game's start (longshot-builder's play.rs, a
/// test's).
pub trait Host {
    /// The update with options `o`, for reason `why`: the error's text when it stopped.
    fn update(&mut self, o: &BuildOptions, why: Stale) -> std::result::Result<(), String>;
    /// An update that stopped, the game folder's game still there: whether to start it anyway (said to the player).
    fn play_anyway(&mut self, text: &str) -> bool;
    /// The game's program started with `args`, its environment with [`PROFILE_VAR`] (`profile`, absolute) and its
    /// channel to this builder ([`crate::channel`]), waited for: how it ended.
    fn start(&mut self, program: &Path, args: &[String], profile: &Path) -> std::io::Result<Started>;
    /// Said to the player: the game asked for a build [`MAX_RESTARTS`] times in a row, and is not started again.
    fn too_many_restarts(&mut self);
}

/// The profile's folders the game writes in, made when missing and tried for writing: the profile itself (its
/// `settings.ini`), `mods/`, `saves/save` and `saves/save-work`. The sentence for the player when one can't be used
/// (`profile_unwritable`: which folder, and why).
pub fn prepare_profile(profile: &Path) -> std::result::Result<(), String> {
    let folders =
        [profile.to_path_buf(), profile.join("mods"), saves(profile).join("save"), saves(profile).join("save-work")];
    for d in folders {
        let tried = std::fs::create_dir_all(&d).and_then(|()| {
            let probe = d.join(".longshot-write-test");
            std::fs::write(&probe, b"")?;
            std::fs::remove_file(&probe)
        });
        if let Err(e) = tried {
            return Err(format!(
                "Longshot can't write in {} ({e}): the profile keeps its saves, settings and mods there. Fix the folder's \
                 permissions or choose another profile folder.",
                d.display()
            ));
        }
    }
    Ok(())
}

/// How a game ended ([`Host::start`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started {
    /// Its exit status.
    pub status: i32,
    /// It asked over its channel to be built again (`restart_to_apply`).
    pub restart: bool,
}

/// How a [`play`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Played {
    /// The game ran and exited with this status.
    Exited(i32),
    /// The update stopped and there was no game to start (or the host chose not to): its text.
    UpdateFailed(String),
    /// The game could not be started: why.
    StartFailed(String),
    /// A folder of the profile the game writes in can't be used (`profile_unwritable`): its text.
    ProfileUnusable(String),
}

/// Plays profile `p` (at `profile`) in the version folder at `version` for builder `builder` with the game's arguments
/// `args`: the fast check, the update only when it says so, the game (given the profile in [`PROFILE_VAR`] alone: the
/// game keeps its saves in `<profile>/saves` and its settings in `<profile>/settings.ini`; the player's earlier saves
/// copied in first, [`carry_saves`]);
/// after the game asked for a build (`restart_to_apply`) and exited normally (0), an update and the game again with the
/// same arguments, at most [`MAX_RESTARTS`] times in a row. `get`: the environment, for the shaders' key and the earlier saves. `checked`: told
/// each fast check's answer and how long it took.
#[allow(clippy::too_many_arguments)]
pub fn play(
    version: &Path,
    profile: &Path,
    p: &Profile,
    builder: &str,
    args: &[String],
    get: &dyn Fn(&str) -> Option<String>,
    host: &mut dyn Host,
    checked: &mut dyn FnMut(Option<Stale>, std::time::Duration),
) -> Played {
    let (mut asked, mut restarts) = (false, 0);
    let profile = std::path::absolute(profile).unwrap_or_else(|_| profile.to_path_buf());
    // (the profile's saves (host/longshot_profile.h, longshot_profile_saves): made, as the game makes only its last
    // folder; a player's earlier saves copied in, once)
    if let Some(from) = carry_saves(&profile, &earlier_saves(version, get)) {
        eprintln!("longshot: your saves in {} copied into the profile (kept there too)", from.display());
    }
    // (the profile's folders made and tried before the game starts, not the game's to find: it treats a missing or
    // unwritable one as a plain fatal error)
    if let Err(text) = prepare_profile(&profile) {
        return Played::ProfileUnusable(text);
    }
    let game_args = args.to_vec();
    loop {
        let t = std::time::Instant::now();
        let stale = if asked { Some(Stale::Asked) } else { check(version, &profile, p, builder, get) };
        checked(stale, t.elapsed());
        // (the profile's game folder, as the Mod API's stamp names it now)
        let folder = game_folder(version, mods_stamp(&profile, version).as_deref());
        if let Some(why) = stale {
            let done =
                options(version, &profile, p, builder).map_err(|e| e.to_string()).and_then(|o| host.update(&o, why));
            if let Err(text) = done {
                // (an update that failed leaves the folder's game as it was, whole (the swap puts in all or nothing):
                // played, when there is one and the host says so)
                let built = Record::read(&folder).is_ok() && game_program(&folder).is_file();
                if !(built && host.play_anyway(&text)) {
                    return Played::UpdateFailed(text);
                }
            }
        }
        let program = game_program(&folder);
        let Started { status, restart } = match host.start(&program, &game_args, &profile) {
            Ok(s) => s,
            Err(e) => return Played::StartFailed(format!("{}: {e}", program.display())),
        };
        // (a game that asked and then crashed is not started again)
        if !(restart && status == 0) {
            return Played::Exited(status);
        }
        if restarts == MAX_RESTARTS {
            host.too_many_restarts();
            return Played::Exited(status);
        }
        restarts += 1;
        asked = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch;

    #[test]
    fn profile_json_round_trips_and_keeps_what_it_does_not_know() {
        let d = scratch("play-profile");
        assert_eq!(Profile::read(&d).unwrap(), None, "no profile.json: none");
        let text = r##"{
  "v": 1,
  "name": "Ann",
  "color": "#3a7",
  "mods": [{"id": "x", "on": true}],
  "game": "/games/Ocarina of Time.nup",
  "upscalers": ["dlss", "fsr4"]
}"##;
        std::fs::write(d.join(PROFILE_FILE), text).unwrap();
        let p = Profile::read(&d).unwrap().unwrap();
        assert_eq!(p.game.as_deref(), Some(Path::new("/games/Ocarina of Time.nup")));
        assert_eq!(p.upscalers, [Upscaler::Dlss, Upscaler::Fsr4]);
        assert_eq!(p.file.keys().collect::<Vec<_>>(), ["v", "name", "color", "mods", "game", "upscalers"]);
        // (written again: what it knows and what it does not, the same, in its place and as it was written)
        let mut q = p.clone();
        q.upscalers = vec![Upscaler::Xess];
        q.write(&d).unwrap();
        let back = Profile::read(&d).unwrap().unwrap();
        assert_eq!(back.upscalers, [Upscaler::Xess]);
        assert_eq!(back.file.keys().collect::<Vec<_>>(), ["v", "name", "color", "mods", "game", "upscalers"]);
        assert_eq!(back.file.raw("mods").unwrap().get(), r#"[{"id": "x", "on": true}]"#, "the Mod API's mods kept");
        assert_eq!((back.file.name().as_deref(), back.file.color().as_deref()), (Some("Ann"), Some("#3a7")));
        // (nothing changed: written as it was read)
        back.write(&d).unwrap();
        assert_eq!(Profile::read(&d).unwrap().unwrap(), back);
        // (a new one: the Mod API's two keys and the copy)
        let n = Profile::new("/g.nup".into());
        assert_eq!(n.file.to_text(), "{\n  \"v\": 1,\n  \"name\": \"Default\",\n  \"game\": \"/g.nup\"\n}\n");
        // (the licences: each vendor's at this builder's pin counts; one of an earlier pin is asked again)
        use crate::runtimes::Vendor;
        let mut with = back.clone();
        assert!(with.licenses().is_empty());
        with.file.set_license("amd", "an earlier pin");
        with.accept(&[Vendor::Nvidia]);
        with.write(&d).unwrap();
        let read = Profile::read(&d).unwrap().unwrap();
        assert_eq!(read.licenses(), [Vendor::Nvidia]);
        assert_eq!(read.file.licenses().get("nvidia").map(String::as_str), Some(Vendor::Nvidia.pin()));
        let mut again = read.clone();
        again.accept(&[Vendor::Nvidia]);
        assert_eq!(again.file, read.file, "accepted already: the file as it was");
        // (no upscalers: the key gone, as before)
        let mut none = with.clone();
        none.upscalers.clear();
        none.write(&d).unwrap();
        assert!(Profile::read(&d).unwrap().unwrap().file.raw("upscalers").is_none());
        // (a profile of the Mod API's with no copy yet: read, its copy none)
        std::fs::write(d.join(PROFILE_FILE), r#"{"v":1,"name":"Bo"}"#).unwrap();
        assert_eq!(Profile::read(&d).unwrap().unwrap().game, None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_profile_json_it_cannot_take_is_said() {
        let d = scratch("play-profile-bad");
        for (text, says) in [
            ("{", "profile_unreadable"),
            ("[1]", "object"),
            (r#"{"v": 2, "game": "/g.nup"}"#, "newer"),
            (r#"{"v": 1, "game": "/g.nup", "upscalers": ["dlsss"]}"#, "dlsss"),
            (r#"{"v": 1, "game": 3}"#, "game"),
            (r#"{"v": 1, "v": 1}"#, "twice"),
        ] {
            std::fs::write(d.join(PROFILE_FILE), text).unwrap();
            let e = Profile::read(&d).unwrap_err();
            assert_eq!(e.code(), "profile_unreadable", "{text}");
            assert!(e.to_string().contains(says) || says == "profile_unreadable", "{text}: {e}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_game_folder_is_the_builds_one_of_the_mods_stamp() {
        let v = Path::new("/v");
        assert_eq!(game_folder(v, None), Path::new("/v/builds/default"));
        assert_eq!(game_folder(v, Some("ab12")), Path::new("/v/builds/ab12"));
        assert_eq!(default_profile(v), Path::new("/v/profile"));
        assert_eq!(saves(&default_profile(v)), Path::new("/v/profile/saves"));
        assert_eq!(settings(&default_profile(v)), Path::new("/v/profile/settings.ini"));
        assert_eq!(mods_stamp(&default_profile(v), v), None, "none in this builder: the Mod API's to make");
    }

    /// A version folder at `d` with its default profile of a stand-in .nup, its default game folder made by hand as a
    /// build leaves it, its record stamped for builder `b`.
    fn built(d: &Path, b: &str) -> (PathBuf, Profile) {
        let prof = default_profile(d);
        std::fs::create_dir_all(&prof).unwrap();
        std::fs::write(d.join("g.nup"), b"nup").unwrap();
        let p = Profile::new(d.join("g.nup"));
        p.write(&prof).unwrap();
        let folder = game_folder(d, None);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(game_program(&folder), b"game").unwrap();
        let mut rec = Record::new("v1", &GameSource::Nup(d.join("g.nup")), &[]);
        rec.stamp = Some(stamp(d, &prof, &p, b));
        rec.write(&folder).unwrap();
        (prof, p)
    }

    /// The environment of the tests' checks: no shader preparation (the stand-in game has none to keep).
    fn no_prepare(k: &str) -> Option<String> {
        (k == "LONGSHOT_NO_PREPARE").then(|| "1".into())
    }

    #[test]
    fn the_fast_check_says_up_to_date_and_each_change_needs_an_update() {
        let d = scratch("play-check");
        let (prof, p) = built(&d, "b1");
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), None, "fresh and up to date");
        // (a warm check, timed: a few file reads and stats)
        let t = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(check(&d, &prof, &p, "b1", no_prepare), None);
        }
        eprintln!("the fast check: {:?} each", t.elapsed() / 100);
        assert!(t.elapsed() / 100 < std::time::Duration::from_millis(100));
        // (each input changed)
        assert_eq!(check(&d, &prof, &p, "b2", no_prepare), Some(Stale::Builder), "another builder");
        let mut ups = p.clone();
        ups.upscalers = vec![Upscaler::Fsr4];
        assert_eq!(check(&d, &prof, &ups, "b1", no_prepare), Some(Stale::Profile), "the profile's upscalers");
        let mut other = p.clone();
        other.game = Some("other.nup".into());
        assert_eq!(check(&d, &prof, &other, "b1", no_prepare), Some(Stale::Profile), "the profile's copy");
        let mut named = p.clone();
        named.file.set_string("name", Some("Ann"));
        named.file.put("mods", launcher_profile::RawValue::from_string("[]".into()).unwrap());
        assert_eq!(check(&d, &prof, &named, "b1", no_prepare), None, "the profile's other keys build nothing");
        std::fs::write(d.join("g.nup"), b"nup, another dump").unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), Some(Stale::Game), "the copy's size");
        std::fs::write(d.join("g.nup"), b"nup").unwrap();
        let f = std::fs::File::options().write(true).open(d.join("g.nup")).unwrap();
        f.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), Some(Stale::Game), "the copy's time");
        std::fs::remove_file(d.join("g.nup")).unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), Some(Stale::Game), "the copy gone");
        // (the shaders: their key not the folder's, with the preparation on)
        let (prof, p) = built(&d, "b1");
        assert_eq!(check(&d, &prof, &p, "b1", |_| None), Some(Stale::Shaders), "never prepared here");
        // (the mods' stamp, when the record has one and the builder none: not compared)
        let folder = game_folder(&d, None);
        let mut rec = Record::read(&folder).unwrap();
        rec.stamp.as_mut().unwrap().mods = Some("m1".into());
        rec.write(&folder).unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), None);
        // (the record without a stamp, or none, or no game)
        rec.stamp = None;
        rec.write(&folder).unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), Some(Stale::NoStamp));
        std::fs::remove_file(game_program(&folder)).unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), Some(Stale::NotBuilt));
        std::fs::remove_file(folder.join(crate::folder::RECORD_FILE)).unwrap();
        assert_eq!(check(&d, &prof, &p, "b1", no_prepare), Some(Stale::NotBuilt));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_update_s_options_are_the_profile_s_and_its_stamp() {
        let d = scratch("play-options");
        let prof = d.join("players/ann");
        std::fs::create_dir_all(&prof).unwrap();
        std::fs::write(prof.join("g.nup"), b"nup").unwrap();
        let mut p = Profile::new("g.nup".into());
        p.upscalers = vec![Upscaler::Fsr4];
        let o = options(&d, &prof, &p, "b1").unwrap();
        assert_eq!(
            o.game,
            GameSource::Nup(std::path::absolute(prof.join("g.nup")).unwrap()),
            "relative: the profile's"
        );
        assert_eq!(o.out, crate::layout::game_folder(&d.join("builds/default")));
        assert_eq!((o.upscalers.as_slice(), o.update), (&[Upscaler::Fsr4][..], false));
        assert_eq!(o.profile, Some(std::path::absolute(&prof).unwrap()), "the profile's mods");
        assert_eq!(o.stamp, Some(stamp(&d, &prof, &p, "b1")));
        // (built before: an update of it, a record's older keys (a user folder, a launcher) ignored)
        let folder = game_folder(&d, None);
        std::fs::create_dir_all(&folder).unwrap();
        let rec = Record::new("v1", &GameSource::Nup(prof.join("g.nup")), &[]);
        rec.write(&folder).unwrap();
        let o = options(&d, &prof, &p, "b1").unwrap();
        assert!(o.update);
        assert_eq!(o.upscalers, [Upscaler::Fsr4], "the profile's, not the record's");
        assert_eq!(o.profile, Some(std::path::absolute(&prof).unwrap()), "the profile's, not a record's");
        // (no copy: said)
        p.game = None;
        assert_eq!(options(&d, &prof, &p, "b1").unwrap_err().code(), "profile_unreadable");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn earlier_saves_are_copied_into_an_empty_profile_once_and_kept() {
        let d = scratch("play-saves");
        let (old, appdata, profile) = (d.join("game/user"), d.join("appdata/Longshot"), d.join("profile"));
        // (none anywhere: nothing)
        assert_eq!(carry_saves(&profile, &[old.clone(), appdata.clone()]), None);
        std::fs::create_dir_all(old.join("save/0000")).unwrap();
        std::fs::create_dir_all(old.join("save-work/0000")).unwrap();
        std::fs::create_dir_all(old.join("recordings")).unwrap();
        std::fs::create_dir_all(appdata.join("save/1111")).unwrap();
        std::fs::write(old.join("save/0000/file"), b"slot").unwrap();
        std::fs::write(old.join("mouse_sens.txt"), b"1.5").unwrap();
        std::fs::write(old.join("longshot_options.ini"), b"dlss=1\n").unwrap();
        // (the first place that has a save: copied, the original kept, the journal of a cut write left)
        assert_eq!(carry_saves(&profile, &[old.clone(), appdata.clone()]), Some(old.clone()));
        let to = saves(&profile);
        assert_eq!(std::fs::read(to.join("save/0000/file")).unwrap(), b"slot");
        assert_eq!(std::fs::read(settings(&profile)).unwrap(), b"dlss=1\n", "the settings: the profile's settings.ini");
        // (the saves alone in saves/: the rest stays in the user folder the game keeps)
        let mut names: Vec<_> = std::fs::read_dir(&to).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, ["save"]);
        assert!(!to.join("save/1111").exists());
        assert!(old.join("save/0000/file").is_file(), "kept where it was");
        assert!(!profile.join(".saves-copying").exists());
        // (once: the profile has saves now)
        std::fs::write(old.join("save/0000/file"), b"newer").unwrap();
        assert_eq!(carry_saves(&profile, std::slice::from_ref(&old)), None);
        assert_eq!(std::fs::read(to.join("save/0000/file")).unwrap(), b"slot");
        // (the profile is the per-user place itself (RecompLauncher's {config}/Longshot on Windows): its own save/
        // copied into its saves/)
        let own = d.join("own");
        std::fs::create_dir_all(own.join("save/2222")).unwrap();
        assert_eq!(carry_saves(&own, std::slice::from_ref(&own)), Some(own.clone()));
        assert!(saves(&own).join("save/2222").is_dir() && own.join("save/2222").is_dir());
        // (a profile with settings of its own keeps them)
        let kept = d.join("kept");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(settings(&kept), b"dlss=4\n").unwrap();
        assert_eq!(carry_saves(&kept, std::slice::from_ref(&old)), Some(old.clone()));
        assert_eq!(std::fs::read(settings(&kept)).unwrap(), b"dlss=4\n");
        // (the places looked in)
        let env = |k: &str| match k {
            "HOME" => Some("/home/p".to_string()),
            "APPDATA" => Some("C:\\A".to_string()),
            _ => None,
        };
        let places = earlier_saves(Path::new("/v"), env);
        assert_eq!(
            places[..3],
            [PathBuf::from("/v/Longshot/user"), "/v/game/user".into(), "/v/builds/default/user".into()]
        );
        let last =
            if cfg!(windows) { Path::new("C:\\A").join("Longshot") } else { "/home/p/.local/share/longshot".into() };
        assert_eq!(places[3], last);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A host that records what it was asked, the game's statuses given in turn.
    #[derive(Default)]
    struct Fake {
        statuses: Vec<(i32, bool)>,
        updates: Vec<Stale>,
        starts: Vec<(Vec<String>, PathBuf)>,
        fail_update: bool,
        too_many: bool,
    }

    impl Host for Fake {
        fn update(&mut self, o: &BuildOptions, why: Stale) -> std::result::Result<(), String> {
            self.updates.push(why);
            if self.fail_update {
                return Err("stopped".into());
            }
            // (as a build: the game and its record, stamped)
            std::fs::create_dir_all(&o.out).unwrap();
            std::fs::write(game_program(&o.out), b"game").unwrap();
            let mut rec = Record::new("v1", &o.game, &o.upscalers);
            rec.stamp = o.stamp.clone();
            rec.write(&o.out).unwrap();
            Ok(())
        }
        fn play_anyway(&mut self, _: &str) -> bool {
            true
        }
        fn start(&mut self, _: &Path, args: &[String], profile: &Path) -> std::io::Result<Started> {
            self.starts.push((args.to_vec(), profile.to_path_buf()));
            let (status, restart) = if self.statuses.is_empty() { (0, false) } else { self.statuses.remove(0) };
            Ok(Started { status, restart })
        }
        fn too_many_restarts(&mut self) {
            self.too_many = true;
        }
    }

    fn fake(statuses: &[(i32, bool)]) -> Fake {
        Fake { statuses: statuses.to_vec(), ..Fake::default() }
    }

    #[test]
    fn the_profile_s_folders_are_made_and_one_that_cannot_be_used_is_said() {
        let d = scratch("play-prepare");
        let prof = d.join("p");
        prepare_profile(&prof).unwrap();
        for f in ["saves/save", "saves/save-work", "mods"] {
            assert!(prof.join(f).is_dir(), "{f}");
        }
        assert!(!prof.join("mods/.longshot-write-test").exists(), "the probe is gone");
        // (a file where a folder goes: said, with the folder)
        let bad = d.join("r");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("mods"), b"a file").unwrap();
        let text = prepare_profile(&bad).unwrap_err();
        assert!(text.contains(&bad.join("mods").display().to_string()) && text.contains("can't write"), "{text}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn play_updates_only_when_needed_and_builds_again_when_the_game_asks() {
        let d = scratch("play-loop");
        let prof = default_profile(&d);
        std::fs::create_dir_all(&prof).unwrap();
        std::fs::write(d.join("g.nup"), b"nup").unwrap();
        let p = Profile::new(d.join("g.nup"));
        p.write(&prof).unwrap();
        let args = vec!["--fullscreen".to_string()];
        let mut said = vec![];
        // (the first: built, then started, with the profile's saves as its user folder, made)
        let mut h = fake(&[]);
        let r = play(&d, &prof, &p, "b1", &args, &no_prepare, &mut h, &mut |s, _| said.push(s));
        assert_eq!((r, h.updates.as_slice(), h.starts.len()), (Played::Exited(0), &[Stale::NotBuilt][..], 1));
        let game_args = vec!["--fullscreen".to_string()];
        assert_eq!(h.starts[0], (game_args.clone(), prof.clone()));
        assert!(saves(&prof).is_dir());
        // (again: up to date, started at once)
        let mut h = fake(&[(3, false)]);
        let r = play(&d, &prof, &p, "b1", &args, &no_prepare, &mut h, &mut |s, _| said.push(s));
        assert_eq!((r, h.updates.len()), (Played::Exited(3), 0), "its status, nothing built");
        // (the game asks for a build twice: built and started again with the same arguments each time)
        let mut h = fake(&[(0, true), (0, true), (0, false)]);
        let r = play(&d, &prof, &p, "b1", &args, &no_prepare, &mut h, &mut |s, _| said.push(s));
        assert_eq!(r, Played::Exited(0));
        assert_eq!(h.updates, [Stale::Asked, Stale::Asked]);
        assert!(h.starts.iter().all(|(a, _)| *a == game_args) && h.starts.len() == 3);
        assert!(!h.too_many);
        // (the guard: three in a row, then the game's status is the end)
        let mut h = fake(&[(0, true); 10]);
        let r = play(&d, &prof, &p, "b1", &args, &no_prepare, &mut h, &mut |s, _| said.push(s));
        assert_eq!(r, Played::Exited(0));
        assert_eq!(h.updates.len() as u32, MAX_RESTARTS);
        assert_eq!(h.starts.len() as u32, MAX_RESTARTS + 1);
        assert!(h.too_many);
        assert_eq!(said[..2], [Some(Stale::NotBuilt), None]);
        // (a game that asked and then crashed is not started again)
        let mut h = fake(&[(139, true)]);
        let r = play(&d, &prof, &p, "b1", &args, &no_prepare, &mut h, &mut |_, _| {});
        assert_eq!((r, h.starts.len(), h.updates.len()), (Played::Exited(139), 1, 0));
        // (the profile's folders are made before the game starts: its saves, mods and settings' folder)
        assert!(
            saves(&prof).join("save").is_dir() && saves(&prof).join("save-work").is_dir() && prof.join("mods").is_dir()
        );
        // (a user folder of the player's own: passed on as it is, the profile's saves made all the same: the game
        // takes them from the profile alone)
        let mut h = fake(&[]);
        let own = vec!["--user".to_string(), "/mine".into()];
        std::fs::remove_dir_all(saves(&prof)).unwrap();
        play(&d, &prof, &p, "b1", &own, &no_prepare, &mut h, &mut |_, _| {});
        assert_eq!(h.starts[0].0, own);
        assert!(saves(&prof).is_dir());
        // (an update that stops: the game there is played; none there, the error)
        let mut h = fake(&[]);
        h.fail_update = true;
        let mut newer = p.clone();
        newer.upscalers = vec![Upscaler::Fsr4];
        let r = play(&d, &prof, &newer, "b1", &args, &no_prepare, &mut h, &mut |_, _| {});
        assert_eq!((r, h.starts.len()), (Played::Exited(0), 1), "the game it had");
        std::fs::remove_dir_all(game_folder(&d, None)).unwrap();
        let mut h = fake(&[]);
        h.fail_update = true;
        let r = play(&d, &prof, &p, "b1", &args, &no_prepare, &mut h, &mut |_, _| {});
        assert_eq!((r, h.starts.len()), (Played::UpdateFailed("stopped".into()), 0));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn patches_stamp_follows_the_enabled_mods_patch_files() {
        let d = scratch("patches-stamp");
        assert_eq!(patches_stamp(&d), "", "no mods: empty");
        let m = |n: &str| {
            let e = mods(&d).join(n).join("exefs");
            std::fs::create_dir_all(&e).unwrap();
            std::fs::write(e.parent().unwrap().join("mod.json"), b"{}").unwrap();
            e
        };
        let (a, b) = (m("a"), m("b"));
        std::fs::write(a.join("notes.txt"), b"x").unwrap();
        assert_eq!(patches_stamp(&d), "", "mods without patches: empty");
        std::fs::write(a.join("p.pchtxt"), b"one").unwrap();
        std::fs::write(b.join("q.ips"), b"two").unwrap();
        let s1 = patches_stamp(&d);
        assert_eq!(s1.len(), 64);
        std::fs::write(a.join("other.bin"), b"y").unwrap();
        std::fs::write(a.join("readme.md"), b"y").unwrap();
        assert_eq!(patches_stamp(&d), s1, "files that aren't .ips or .pchtxt ignored");
        std::fs::write(a.join("p.pchtxt"), b"One").unwrap();
        let s2 = patches_stamp(&d);
        assert_ne!(s2, s1, "a patch's bytes changed");
        std::fs::write(d.join("mods.json"), r#"{"mods":[{"id":"local/b"},{"id":"local/a"}]}"#).unwrap();
        let s3 = patches_stamp(&d);
        assert_ne!(s3, s2, "the order changed");
        std::fs::write(d.join("mods.json"), r#"{"mods":[{"id":"local/b"},{"id":"local/a","enabled":false}]}"#).unwrap();
        let s4 = patches_stamp(&d);
        assert_ne!(s4, s3, "a mod switched off");
        std::fs::remove_file(b.join("q.ips")).unwrap();
        assert_eq!(patches_stamp(&d), "", "only a switched-off mod's patches: empty");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
