//! `longshot-builder steam-shortcut [--profile <dir>] [--name <text>] [--remove] [--quit-steam]`: Longshot as a
//! non-Steam game in the signed-in Steam account's library (the `steam` crate: the Longshot Launcher's Steam tile,
//! ported), pointing at this program (the version folder's builder, which plays), with `--profile <dir>` in its launch
//! options when given, and Longshot's icon. Its tile only: one of that name it did not make is left as it is
//! (`steam_name_taken`). Steam reads its library when it starts: with Steam running the tile shows after Steam starts
//! again (a `steam_running` warning), unless `--quit-steam` asks Steam to quit first. Said as a `steam_shortcut` event.

use build_core::report::{Event, Reporter};
use build_core::{Error, Result};
use std::path::Path;

/// Longshot's icon, the tile's (Steam draws an `.ico` named in the tile's `icon` on the Steam Deck's Gaming mode; a
/// `.png` there shows the blank default: the Longshot Launcher's finding).
const ICON: &[u8] = include_bytes!("../../../../assets/longshot.ico");

/// What the command was asked.
#[derive(Debug, Default)]
pub struct Asked<'a> {
    /// `--profile`: the profile the tile plays.
    pub profile: Option<&'a Path>,
    /// `--name`: the tile's name.
    pub name: Option<&'a str>,
    /// `--remove`: the tile taken out.
    pub remove: bool,
    /// `--quit-steam`: Steam asked to quit before the library is written.
    pub quit_steam: bool,
}

/// The tile's launch options for the program at `exe`: `--profile "<dir>"` (absolute) when one is given; `play` first
/// when the program does not play by itself with no arguments (a developer's `longshot-builder` out of a version
/// folder).
pub fn launch_options(exe: &Path, profile: Option<&Path>) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut tail: Vec<String> = Vec::new();
    if let Some(p) = profile {
        let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        tail = vec!["--profile".into(), p.to_string_lossy().into_owned()];
    }
    if crate::play::wanted(exe, &tail).is_none() {
        words.push("play".into());
    }
    if !tail.is_empty() {
        words.push(tail[0].clone());
        words.push(steam::argument(&tail[1]));
    }
    words.join(" ")
}

/// The command for the program at `exe`, said with `r`.
pub fn run(asked: &Asked, exe: &Path, r: &Reporter) -> Result<()> {
    let homes = steam::system_homes();
    let Some(lib) = steam::locate(&homes) else {
        return Err(Error::player(
            "steam_not_found",
            "Steam isn't set up for this user here: no Steam library was found (Steam signed in once makes one). \
             Install Steam and sign in, then run this again.",
        ));
    };
    let name = asked.name.unwrap_or(steam::NAME);
    let mut e = steam::Entry::new(exe, name, &launch_options(exe, asked.profile));
    let failed = |e: steam::Error| match e {
        steam::Error::Unreadable(why) => Error::player("steam_library_unreadable", why.to_string()),
        steam::Error::BrokenLink(p) => Error::player(
            "steam_library_unreadable",
            format!("The Steam library {} is a link that can't be followed: nothing was changed.", p.display()),
        ),
        steam::Error::Io(path, source) => Error::io(&path, source),
    };
    let mut running = steam::steam_running();
    if running && asked.quit_steam {
        r.log("steam: asking Steam to quit before the library is written");
        if !steam::quit_steam(&lib.home) {
            return Err(Error::player(
                "steam_still_running",
                "Steam didn't quit, so it could write its library back over the change. Quit Steam yourself (Steam > \
                 Exit), then run this again.",
            ));
        }
        running = false;
    }
    let (action, id, artwork) = if asked.remove {
        let id = steam::tile_id(&lib, &e).map_err(failed)?;
        let removed = steam::remove(&lib, &e).map_err(failed)?;
        (if removed { "removed" } else { "absent" }, id, 0)
    } else {
        let id = steam::tile_id(&lib, &e).map_err(failed)?;
        // (the icon in Steam's grid folder under the tile's id, where none is yet; the tile names it)
        let grid = lib.grid();
        let artwork = steam::write_artwork(&grid, id, &[("_icon", "ico", ICON)]).map_err(failed)?;
        let icon = grid.join(format!("{}_icon.ico", id as u32));
        if icon.is_file() {
            e.icon = icon.to_string_lossy().into_owned();
        }
        let action = match steam::add(&lib, &e).map_err(failed)? {
            steam::Done::Added(_) => "added",
            steam::Done::Updated(_) => "updated",
            steam::Done::Unchanged(_) => "unchanged",
            steam::Done::NameTaken => {
                return Err(Error::player(
                    "steam_name_taken",
                    format!(
                        "Steam already has a game called {name} that this program didn't add (yours, or the Longshot \
                         Launcher's): it stays as it is. Give this one another name with --name."
                    ),
                ));
            }
        };
        (action, id, artwork)
    };
    if running && action != "unchanged" && action != "absent" {
        r.warn(
            "steam_running",
            "Steam is running: it reads its library when it starts, so the change shows once Steam starts again (on a \
             Steam Deck, restart it). If the change is gone after that, quit Steam and run this again, or pass \
             --quit-steam.",
        );
    }
    r.send(Event::SteamShortcut {
        action,
        name: name.to_string(),
        app_id: id as u32,
        library: lib.vdf.display().to_string(),
        command: format!("{} {}", e.exe, e.launch_options).trim_end().to_string(),
        artwork,
        steam_running: running,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tile_plays_the_profile_given_with_play_first_when_the_program_needs_it() {
        let named = Path::new("/v/Longshot.x86_64");
        assert_eq!(launch_options(named, None), "", "by its name, no arguments: it plays the default profile");
        let p = launch_options(named, Some(Path::new("/p/my profile")));
        assert_eq!(p, "--profile \"/p/my profile\"");
        let builder = Path::new("/somewhere/longshot-builder");
        assert_eq!(launch_options(builder, None), "play", "out of a version folder: play asked for");
        assert_eq!(launch_options(builder, Some(Path::new("/p/ann"))), "--profile \"/p/ann\"", "--profile first plays");
        assert!(
            launch_options(named, Some(Path::new("rel")))
                .contains(&*std::path::absolute("rel").unwrap().to_string_lossy())
        );
    }

    #[test]
    fn the_icon_is_longshot_s() {
        // (an .ico: its header, reserved 0 and type 1)
        assert_eq!(&ICON[..4], &[0, 0, 1, 0]);
    }
}
