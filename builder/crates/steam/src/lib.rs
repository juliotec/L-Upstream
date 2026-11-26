//! Longshot as a non-Steam game in Steam's library: the tile Steam's own "Add a Non-Steam Game" writes, in the
//! signed-in account's `userdata/<id>/config/shortcuts.vdf` (the binary VDF, [`vdf`]). The Longshot Launcher's
//! `Core/SteamShortcuts.cs`, ported (docs/dev/reference/build.md, "Steam"):
//!
//! - **Where**: Steam's folder ([`homes`]: Linux `$XDG_DATA_HOME/Steam`, `~/.steam/steam`, Flatpak's and Snap's;
//!   Windows the registry's `SteamPath`), the account Steam last signed in with (`config/loginusers.vdf`'s
//!   `MostRecent`), else the newest `userdata/<id>/config` ([`locate`]).
//! - **The id**: Steam's own rule, crc32 of the Exe and the name with the high bit set ([`app_id`]): its grid art,
//!   controller layout and play time key off it, so a tile made here and one made by hand behave alike.
//! - **Ours only**: a tile is this program's when its id is the one its Exe and name make and it still points at that
//!   Exe, or it is the one the note beside the library ([`NOTE`]) names (a moved version folder keeps its tile and its
//!   id). Another tile of the same name (the player's, the Longshot Launcher's) is never touched: [`Done::NameTaken`].
//! - **Never lost**: a library that doesn't read whole is refused, not rewritten ([`vdf`]); the library as it was
//!   before the first change is kept once beside it (`shortcuts.vdf.bak-longshot`); a library that is a link is
//!   written through it; each write is whole or not at all.
//! - **Art**: Steam's grid folder beside the library, each picture under both names Steam's two UIs read, written only
//!   where the slot is empty ([`write_artwork`]): what the player chose in Steam stays.
//! - **Steam running** ([`steam_running`]) holds its library in memory: a change shows at its next start, and it may
//!   write its own copy back when it quits. The launcher writes anyway and says to restart Steam; [`quit_steam`] (its
//!   `takeDownFirst`) asks Steam to quit first, only when the player asked.

pub mod vdf;

use std::path::{Path, PathBuf};
use vdf::{Map, Value};

/// The tile's name by default.
pub const NAME: &str = "Longshot";
/// The note beside the library naming the tiles this program made (`<library>.longshot-builder`): a line each, the
/// tile's name, the Exe it was written with, its id and the Exe its id was made from, tab-separated. The Longshot
/// Launcher's own note (`.longshot`) is apart: neither claims the other's tile.
pub const NOTE: &str = "longshot-builder";
/// The copy of the library as it was before the first change, kept once beside it.
pub const BACKUP: &str = "bak-longshot";

/// A failure that leaves the library as it was.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The library doesn't read whole: rewriting it would lose tiles.
    #[error(transparent)]
    Unreadable(#[from] vdf::Error),
    /// A file could not be read or written.
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// The library is a link that can't be followed: writing would replace the link with a file.
    #[error("the Steam library {0} is a link that can't be followed")]
    BrokenLink(PathBuf),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |e| Error::Io(path.to_path_buf(), e)
}

// ---- the tile

/// A tile as this program writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name in the library.
    pub name: String,
    /// The program, as Steam writes it (quoted when it has a space).
    pub exe: String,
    /// The program's folder, written the same way.
    pub start_dir: String,
    /// The arguments Steam gives it.
    pub launch_options: String,
    /// The icon's file ("" none).
    pub icon: String,
}

impl Entry {
    /// The tile of the program at `exe` (absolute) named `name`, started with `launch_options`.
    pub fn new(exe: &Path, name: &str, launch_options: &str) -> Entry {
        let dir = exe.parent().unwrap_or(exe);
        Entry {
            name: name.to_string(),
            exe: quoted(&exe.to_string_lossy()),
            start_dir: quoted(&dir.to_string_lossy()),
            launch_options: launch_options.to_string(),
            icon: String::new(),
        }
    }

    /// The id its Exe and name make.
    pub fn app_id(&self) -> i32 {
        app_id(&self.exe, &self.name)
    }
}

/// A path as Steam writes it in Exe and StartDir: in double quotes when it has a space (the quotes are part of the value
/// the id is made from).
pub fn quoted(path: &str) -> String {
    if path.contains(' ') { format!("\"{path}\"") } else { path.to_string() }
}

/// An argument for the launch options, in double quotes (a Linux Steam runs them through a shell; Windows' parses them
/// as a command line), a `"` in it escaped.
pub fn argument(arg: &str) -> String {
    format!("\"{}\"", arg.replace('"', "\\\""))
}

/// Steam's id of a non-Steam game: crc32 of the Exe as written and the name, the high bit set, read as signed.
pub fn app_id(exe: &str, name: &str) -> i32 {
    let mut h = crc32fast::Hasher::new();
    h.update(exe.as_bytes());
    h.update(name.as_bytes());
    (h.finalize() | 0x8000_0000) as i32
}

/// The 64-bit id the old Big Picture keys a shortcut's art by: the id, unsigned, shifted up 32, with 0x02000000.
pub fn grid_id64(app_id: i32) -> u64 {
    (u64::from(app_id as u32) << 32) | 0x0200_0000
}

// ---- where Steam is

/// Steam's folders this user may have, most likely first, with no duplicates by their real place: Linux
/// `$XDG_DATA_HOME/Steam` (`~/.local/share/Steam`, where the Steam Deck's `~/.steam/steam` points), `~/.steam/steam`,
/// Flatpak's (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`, then its older `data/Steam`) and Snap's;
/// Windows the registry's `HKCU\Software\Valve\Steam\SteamPath`, then `%ProgramFiles(x86)%\Steam`. `get`: the
/// environment; `steam_path`: Windows' registry value.
pub fn homes(get: &dyn Fn(&str) -> Option<String>, windows: bool, steam_path: Option<String>) -> Vec<PathBuf> {
    let set = |k: &str| get(k).filter(|v| !v.is_empty());
    let mut all: Vec<PathBuf> = Vec::new();
    if windows {
        all.extend(steam_path.map(PathBuf::from));
        all.extend(set("ProgramFiles(x86)").map(|p| Path::new(&p).join("Steam")));
    } else if let Some(home) = set("HOME").map(PathBuf::from) {
        let data = set("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        all.push(data.join("Steam"));
        all.push(home.join(".steam/steam"));
        let flatpak = home.join(".var/app/com.valvesoftware.Steam");
        all.push(flatpak.join(".local/share/Steam"));
        all.push(flatpak.join("data/Steam"));
        all.push(home.join("snap/steam/common/.local/share/Steam"));
    }
    let mut seen = Vec::new();
    all.into_iter()
        .filter(|p| {
            let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            let new = !seen.contains(&real);
            seen.push(real);
            new
        })
        .collect()
}

/// This PC's Steam folders ([`homes`], with this process's environment and registry).
pub fn system_homes() -> Vec<PathBuf> {
    let steam_path = build_core::user_registry(r"Software\Valve\Steam")
        .0
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("SteamPath"))
        .map(|(_, v)| v.replace('/', "\\"))
        .filter(|v| !v.is_empty());
    homes(&|k| std::env::var(k).ok(), cfg!(windows), steam_path)
}

/// Where a tile goes: the library file and the Steam folder it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    /// Steam's folder.
    pub home: PathBuf,
    /// The account's `shortcuts.vdf` (it may not exist yet).
    pub vdf: PathBuf,
}

impl Library {
    /// The grid folder beside the library: the tiles' art.
    pub fn grid(&self) -> PathBuf {
        self.vdf.parent().unwrap_or(Path::new(".")).join("grid")
    }
}

/// The library of the first of `homes` that has one: the account Steam last signed in with, else the newest
/// `userdata/<id>/config`, else an old Steam's single `config/shortcuts.vdf`. Nothing is made.
pub fn locate(homes: &[PathBuf]) -> Option<Library> {
    homes.iter().find_map(|home| {
        let userdata = home.join("userdata");
        let account = most_recent_account(&home.join("config"), &userdata).or_else(|| newest_account(&userdata));
        let vdf = match account {
            Some(a) => a.join("config").join("shortcuts.vdf"),
            None => Some(home.join("config").join("shortcuts.vdf")).filter(|v| v.is_file())?,
        };
        Some(Library { home: home.clone(), vdf })
    })
}

/// The account `loginusers.vdf` (text VDF) says Steam last used: the SteamID64 whose entry holds `"MostRecent" "1"`,
/// its folder the id's low 32 bits.
fn most_recent_account(config: &Path, userdata: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config.join("loginusers.vdf")).ok()?;
    let mut id: Option<u64> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("\"7656") {
            id = line.trim_matches('"').parse().ok();
            continue;
        }
        let Some(rest) = line.strip_prefix("\"MostRecent\"") else { continue };
        if let Some(id) = id.filter(|_| rest.trim().trim_matches('"') == "1") {
            let dir = userdata.join((id & 0xFFFF_FFFF).to_string());
            if dir.join("config").is_dir() {
                return Some(dir);
            }
        }
    }
    None
}

/// The numeric `userdata` folder whose `config/` was written last.
fn newest_account(userdata: &Path) -> Option<PathBuf> {
    std::fs::read_dir(userdata)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().parse::<u64>().is_ok())
        .filter_map(|e| {
            let time = std::fs::metadata(e.path().join("config")).ok()?.modified().ok()?;
            Some((time, e.path()))
        })
        .max()
        .map(|(_, p)| p)
}

// ---- the library

/// What [`add`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// A new tile, with this id.
    Added(i32),
    /// Our tile, changed to point as asked (its id kept).
    Updated(i32),
    /// Our tile was there as asked already.
    Unchanged(i32),
    /// A tile of that name is there that this program did not make: left as it is.
    NameTaken,
}

/// A tile this program made, as its note says.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Claim {
    name: String,
    exe: String,
    id: i32,
    origin: String,
}

/// The library file Steam really holds: a link followed (it stays a link). One that can't be followed is refused.
fn real(vdf: &Path) -> Result<PathBuf, Error> {
    match std::fs::symlink_metadata(vdf) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(vdf).map_err(|_| Error::BrokenLink(vdf.into())),
        _ => Ok(vdf.to_path_buf()),
    }
}

fn beside(file: &Path, ext: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// The note's claims (none when it can't be read; a line of another shape, or whose id neither of its Exes makes, is
/// no claim).
fn claims(file: &Path) -> Vec<Claim> {
    let text = std::fs::read_to_string(beside(file, NOTE)).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let [name, exe, id, origin] = f[..] else { return None };
            let id: i32 = id.parse().ok()?;
            (app_id(exe, name) == id || app_id(origin, name) == id).then(|| Claim {
                name: name.into(),
                exe: exe.into(),
                id,
                origin: origin.into(),
            })
        })
        .collect()
}

fn write_claims(file: &Path, all: &[Claim]) -> Result<(), Error> {
    let note = beside(file, NOTE);
    if all.is_empty() {
        return match std::fs::remove_file(&note) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::Io(note, e)),
            _ => Ok(()),
        };
    }
    let text: String = all.iter().map(|c| format!("{}\t{}\t{}\t{}\n", c.name, c.exe, c.id, c.origin)).collect();
    build_core::fsx::write_atomic(&note, text.as_bytes())
        .map_err(|e| Error::Io(note, std::io::Error::other(e.to_string())))
}

/// The library at `file` read (none yet: empty), refused when it does not read whole or holds no `shortcuts` map.
fn read(file: &Path) -> Result<Map, Error> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::Io(file.into(), e)),
    };
    let root = vdf::parse(&bytes)?;
    if !bytes.is_empty() && !root.0.iter().any(|(k, v)| k == "shortcuts" && matches!(v, Value::Map(_))) {
        return Err(vdf::not_a_library().into());
    }
    Ok(root)
}

/// The library at `file` written whole, its state before the first change kept once beside it.
fn write(file: &Path, root: &Map) -> Result<(), Error> {
    let backup = beside(file, BACKUP);
    if file.is_file() && !backup.exists() {
        let _ = std::fs::copy(file, &backup);
    }
    build_core::fsx::write_atomic(file, &vdf::dump(root))
        .map_err(|e| Error::Io(file.into(), std::io::Error::other(e.to_string())))
}

/// Our tile among `shortcuts` (its key): the id `e`'s Exe and name make and still that Exe, or the note's claim of
/// that name, its id and still the Exe the note wrote.
fn ours(shortcuts: &Map, e: &Entry, claims: &[Claim]) -> Option<String> {
    let exact = e.app_id();
    let claim = claims.iter().find(|c| c.name == e.name);
    shortcuts.0.iter().find_map(|(key, v)| {
        let Value::Map(t) = v else { return None };
        let by_id = t.int("appid") == exact && t.str("Exe") == e.exe;
        let by_claim = claim.is_some_and(|c| t.int("appid") == c.id && t.str("Exe") == c.exe);
        (by_id || by_claim).then(|| key.clone())
    })
}

fn tile<'a>(shortcuts: &'a Map, key: &str) -> &'a Map {
    match shortcuts.get(key) {
        Some(Value::Map(m)) => m,
        _ => unreachable!("a key ours() found"),
    }
}

/// The id the library's tile for `e` carries (ours, found as [`add`] finds it), or the one a new tile would get.
pub fn tile_id(lib: &Library, e: &Entry) -> Result<i32, Error> {
    let file = real(&lib.vdf)?;
    let root = read(&file)?;
    let id = match root.get("shortcuts") {
        Some(Value::Map(s)) => ours(s, e, &claims(&file)).map(|k| tile(s, &k).int("appid")),
        _ => None,
    };
    Ok(id.unwrap_or_else(|| e.app_id()))
}

/// Whether our tile for `e` is in the library.
pub fn in_library(lib: &Library, e: &Entry) -> bool {
    let Ok(file) = real(&lib.vdf) else { return false };
    let Ok(root) = read(&file) else { return false };
    matches!(root.get("shortcuts"), Some(Value::Map(s)) if ours(s, e, &claims(&file)).is_some())
}

/// A new tile's keys, in the order Steam's own dialog writes them.
fn new_tile(e: &Entry) -> Map {
    let s = |v: &str| Value::Str(v.to_string());
    Map(vec![
        ("appid".into(), Value::Int(e.app_id())),
        ("AppName".into(), s(&e.name)),
        ("Exe".into(), s(&e.exe)),
        ("StartDir".into(), s(&e.start_dir)),
        ("icon".into(), s(&e.icon)),
        ("ShortcutPath".into(), s("")),
        ("LaunchOptions".into(), s(&e.launch_options)),
        ("IsHidden".into(), Value::Int(0)),
        ("AllowDesktopConfig".into(), Value::Int(1)),
        ("AllowOverlay".into(), Value::Int(1)),
        ("OpenVR".into(), Value::Int(0)),
        ("Devkit".into(), Value::Int(0)),
        ("DevkitGameID".into(), s("")),
        ("DevkitOverrideAppID".into(), Value::Int(0)),
        ("LastPlayTime".into(), Value::Int(0)),
        ("FlatpakAppID".into(), s("")),
        ("sortas".into(), s("")),
        ("tags".into(), Value::Map(Map::default())),
    ])
}

/// Whether tile `t` is `e` already (an empty icon claims nothing).
fn same(t: &Map, e: &Entry) -> bool {
    t.str("Exe") == e.exe
        && t.str("StartDir") == e.start_dir
        && t.str("LaunchOptions") == e.launch_options
        && (e.icon.is_empty() || t.str("icon") == e.icon)
}

/// Our tile for `e` added to the library, or changed to point as `e` says (its id kept: Steam's art, controller layout
/// and play time stay with it). A tile of that name that is not ours is left alone ([`Done::NameTaken`]).
pub fn add(lib: &Library, e: &Entry) -> Result<Done, Error> {
    let file = real(&lib.vdf)?;
    let mut root = read(&file)?;
    let mut all = claims(&file);
    let shortcuts = root.map_mut("shortcuts");
    let found = ours(shortcuts, e, &all);
    let id = match &found {
        Some(key) if same(tile(shortcuts, key), e) => return Ok(Done::Unchanged(tile(shortcuts, key).int("appid"))),
        Some(key) => {
            let Some((_, Value::Map(t))) = shortcuts.0.iter_mut().find(|(k, _)| k == key) else { unreachable!() };
            t.set("Exe", Value::Str(e.exe.clone()));
            t.set("StartDir", Value::Str(e.start_dir.clone()));
            t.set("LaunchOptions", Value::Str(e.launch_options.clone()));
            if !e.icon.is_empty() {
                t.set("icon", Value::Str(e.icon.clone()));
            }
            t.int("appid")
        }
        None if shortcuts.0.iter().any(|(_, v)| matches!(v, Value::Map(t) if t.str("AppName") == e.name)) => {
            return Ok(Done::NameTaken);
        }
        None => {
            // (the first free key, not the count: two tiles under one key would lose one when read back)
            let free = (0..).map(|n: u32| n.to_string()).find(|k| shortcuts.get(k).is_none()).unwrap_or_default();
            shortcuts.0.push((free, Value::Map(new_tile(e))));
            e.app_id()
        }
    };
    // (the note first: a crash between leaves a claim that costs nothing, never a tile no note claims)
    let origin = match all.iter().find(|c| c.name == e.name) {
        Some(c) if found.is_some() && app_id(&c.origin, &e.name) == id => c.origin.clone(),
        _ => e.exe.clone(),
    };
    all.retain(|c| c.name != e.name);
    all.push(Claim { name: e.name.clone(), exe: e.exe.clone(), id, origin });
    write_claims(&file, &all)?;
    write(&file, &root)?;
    Ok(if found.is_some() { Done::Updated(id) } else { Done::Added(id) })
}

/// Our tile for `e` taken out of the library (the art stays in the grid folder: the player's library, and a tile
/// added again gets it back): whether there was one.
pub fn remove(lib: &Library, e: &Entry) -> Result<bool, Error> {
    let file = real(&lib.vdf)?;
    if !file.is_file() {
        return Ok(false);
    }
    let mut root = read(&file)?;
    let mut all = claims(&file);
    let shortcuts = root.map_mut("shortcuts");
    let Some(key) = ours(shortcuts, e, &all) else { return Ok(false) };
    shortcuts.0.retain(|(k, _)| *k != key);
    write(&file, &root)?;
    all.retain(|c| c.name != e.name);
    write_claims(&file, &all)?;
    Ok(true)
}

// ---- the art

/// A picture for a tile: its name's suffix (`""` the landscape capsule, `"p"` the portrait one, `"_hero"`, `"_logo"`,
/// `"_icon"`), its extension and its bytes.
pub type Art<'a> = (&'a str, &'a str, &'a [u8]);

/// Each of `art` written to the grid folder `grid` for tile `app_id`, under its 32-bit name and its 64-bit one, where
/// that slot holds no picture under either name in any format (one the player chose stays; a `.tmp` is not a picture).
/// How many files were written.
pub fn write_artwork(grid: &Path, app_id: i32, art: &[Art]) -> Result<usize, Error> {
    std::fs::create_dir_all(grid).map_err(io(grid))?;
    let names = [(app_id as u32).to_string(), grid_id64(app_id).to_string()];
    let listed: Vec<String> = std::fs::read_dir(grid)
        .map_err(io(grid))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let mut wrote = 0;
    for (suffix, ext, bytes) in art {
        let held = names.iter().any(|stem| {
            let start = format!("{stem}{suffix}.");
            listed.iter().any(|f| f.starts_with(&start) && !f.ends_with(".tmp") && !f[start.len()..].contains('.'))
        });
        if held {
            continue;
        }
        for stem in &names {
            if publish_new(&grid.join(format!("{stem}{suffix}.{ext}")), bytes)? {
                wrote += 1;
            }
        }
    }
    Ok(wrote)
}

/// `bytes` published at `path` whole, only when nothing is there (Steam's UI may publish there at any moment: a link
/// to a finished file, which fails rather than replace one): whether it was.
fn publish_new(path: &Path, bytes: &[u8]) -> Result<bool, Error> {
    let tmp = beside(path, &format!("{}.part", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(io(&tmp))?;
    let done = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match done {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(Error::Io(path.into(), e)),
    }
}

// ---- Steam running

/// Whether a program of name `name` is Steam holding a library in memory: the desktop Steam (`steam`, `steam.exe`),
/// its runtime (`steamrt`, `steamrt64`) or the Steam Deck's Gaming mode (`gamescope`).
pub fn is_steam(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "steam" | "steam.exe" | "steamrt" | "steamrt64" | "gamescope")
}

/// Whether Steam runs now, for this user.
pub fn steam_running() -> bool {
    build_core::running_programs().iter().any(|n| is_steam(n))
}

/// Steam asked to quit (`steam.sh -shutdown` in its folder on Linux, `steam.exe -shutdown` on Windows; else `steam
/// -shutdown`), waited for up to 30 s, then watched for 10 s more: whether it is down.
pub fn quit_steam(home: &Path) -> bool {
    if !steam_running() {
        return true;
    }
    let program =
        [home.join("steam.sh"), home.join("steam.exe")].into_iter().find(|p| p.is_file()).unwrap_or("steam".into());
    let Ok(mut child) = std::process::Command::new(&program)
        .arg("-shutdown")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_secs(40) {
        if child.try_wait().ok().flatten().is_some() && !steam_running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let _ = child.kill();
    let _ = child.wait();
    !steam_running()
}

#[cfg(test)]
mod tests;
