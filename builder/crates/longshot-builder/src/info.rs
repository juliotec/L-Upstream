//! `longshot-builder info [--profile <dir>] [--output json|text]`: everything a launcher wanted to read from game files,
//! so it reads none (docs/dev/reference/build.md, "info"). A few file reads and no hashing: the version this program
//! is, what it and the game it built can do (`capabilities`), the profile's game folder and whether play's fast check
//! finds it up to date, and the profile's keys this builder knows.
//!
//! ```text
//! {"v":1,"type":"info",
//!  "version": {"tag": "nightly-2101-1a2b3c4d" | null, "channel": "nightly" | "stable" | null,
//!              "source": "<the sources' hash>" | null, "stamp": "<play's builder stamp>"},
//!  "program": "<this program>", "platform": "linux-x64",
//!  "capabilities": {"builder": ["fetch", ...], "game": ["save_dir", "profile", ...]},
//!  "profile": {"dir", "exists", "name", "color", "version", "game", "last_played", "copy_of", "start_slot",
//!              "upscalers": [...], "keys": [every key in the file]},
//!  "build": {"folder", "present", "up_to_date", "stale": null | "<why>", "builder", "stamp": {...} | null,
//!            "game", "upscalers"},
//!  "features": [...],                        (the game's, as capabilities.game: longshot_version.txt's features= today)
//!  "game": {"path": "<the profile's copy, resolved>" | null, "exists": bool},
//!  "upscalers": [...]}                       (the profile's)
//! ```

use build_core::report::{Event, Reporter};
use build_driver::folder::Record;
use build_driver::play::{self, Profile};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// What this builder can do that a launcher may ask of it, a name a capability (never reused for something else).
pub const BUILDER_CAPABILITIES: [&str; 12] = [
    "play",
    "profiles",
    "fetch",
    "self_update",
    "from_source",
    "info",
    "verify",
    "steam_shortcut",
    "doctor",
    "mod",
    "channel",
    "storage",
];

/// Who is asked: this program and what it carries.
pub struct Me<'a> {
    /// The program.
    pub exe: &'a Path,
    /// The release it carries (None: a developer's build).
    pub release: Option<&'a str>,
    /// Its sources' hash.
    pub source: Option<&'a str>,
    /// Play's builder stamp (main.rs's `builder_stamp`).
    pub stamp: &'a str,
}

/// The game's capabilities of a game folder: its record's (from the sources it was built from), else its
/// `longshot_version.txt`'s `features=` line (a folder built before the record kept them).
fn game_features(folder: &Path, rec: Option<&Record>) -> Vec<String> {
    if let Some(r) = rec.filter(|r| !r.features.is_empty()) {
        return r.features.clone();
    }
    std::fs::read_to_string(folder.join("longshot_version.txt"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("features="))
        .map(|f| f.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// The answer's fields for profile folder `profile`, `get` the environment.
pub fn answer(me: &Me<'_>, profile: &Path, get: &dyn Fn(&str) -> Option<String>) -> serde_json::Map<String, Value> {
    let version = me.exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let read = Profile::read(profile);
    let p = read.as_ref().ok().cloned().flatten();
    let file = p.as_ref().map(|p| &p.file);
    let s = |k: &str| file.and_then(|f| f.string(k));
    let profile_json = json!({
        "dir": profile.display().to_string(),
        "exists": profile.join(play::PROFILE_FILE).is_file(),
        "readable": read.is_ok(),
        "name": s("name"), "color": s("color"), "version": s("version"), "game": s("game"),
        "last_played": s("last_played"), "copy_of": s("copy_of"),
        "start_slot": file.and_then(|f| f.start_slot()).and_then(Result::ok),
        "upscalers": file.map(|f| f.upscaler_names()).unwrap_or_default(),
        "keys": file.map(|f| f.keys().map(str::to_string).collect::<Vec<_>>()).unwrap_or_default(),
    });
    let mods = play::mods_stamp(profile, &version);
    let folder = play::game_folder(&version, mods.as_deref());
    let rec = Record::read(&folder).ok();
    let stale = p.as_ref().map_or(Some(play::Stale::NotBuilt), |p| play::check(&version, profile, p, me.stamp, get));
    let present = rec.is_some() && play::game_program(&folder).is_file();
    let features = game_features(&folder, rec.as_ref());
    let build = json!({
        "folder": folder.display().to_string(),
        "present": present,
        "up_to_date": present && stale.is_none(),
        "stale": stale.map(|s| s.why()),
        "builder": rec.as_ref().map(|r| r.builder.clone()),
        "stamp": rec.as_ref().and_then(|r| r.stamp.as_ref()).map(|s| json!({"builder": s.builder, "profile": s.profile,
                                                                          "game": s.game, "mods": s.mods})),
        "game": rec.as_ref().map(|r| serde_json::to_value(&r.game).unwrap_or(Value::Null)),
        "upscalers": rec.as_ref().map(|r| r.upscalers.iter().map(|u| u.name()).collect::<Vec<_>>()),
    });
    let game = p.as_ref().and_then(|p| p.game_in(profile));
    let mut m = serde_json::Map::new();
    m.insert(
        "version".into(),
        json!({"tag": me.release, "channel": me.release.and_then(release::Channel::of).map(release::Channel::name),
               "source": me.source, "stamp": me.stamp}),
    );
    m.insert("program".into(), me.exe.display().to_string().into());
    m.insert("platform".into(), release::platform().into());
    m.insert("capabilities".into(), json!({"builder": BUILDER_CAPABILITIES, "game": features}));
    m.insert("profile".into(), profile_json);
    m.insert("build".into(), build);
    m.insert("features".into(), json!(features));
    m.insert(
        "game".into(),
        json!({"path": game.as_ref().map(|g| g.display().to_string()), "exists": game.as_ref().is_some_and(|g| g.exists())}),
    );
    m.insert(
        "upscalers".into(),
        json!(p.as_ref().map(|p| p.upscalers.iter().map(|u| u.name()).collect::<Vec<_>>()).unwrap_or_default()),
    );
    m
}

/// `longshot-builder info ...`: its exit status.
pub fn run(args: &[String], me: &Me<'_>) -> ExitCode {
    let mut args = args.to_vec();
    let asked = match crate::output::take(&mut args) {
        Ok(a) => a,
        Err(why) => return crate::usage_error(why, false),
    };
    let json = asked == Some(crate::output::Output::Json);
    let mut profile: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--profile" => match it.next() {
                Some(p) => profile = Some(p.into()),
                None => return crate::usage_error("--profile needs a value".into(), json),
            },
            other => return crate::usage_error(format!("info: unknown argument {other:?}"), json),
        }
    }
    let version = me.exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let profile = match profile {
        Some(p) => std::path::absolute(&p).unwrap_or(p),
        None => play::default_profile(&version),
    };
    let o = crate::output::set(asked.or(Some(crate::output::Output::Text)));
    let fields = answer(me, &profile, &build_driver::env);
    Reporter::stdout(o.format()).send(Event::Info { fields });
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("info-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn its_answer_s_shape() {
        let d = scratch("shape");
        let exe = d.join("Longshot.x86_64");
        let profile = d.join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(d.join("Game.nup"), b"nup").unwrap();
        std::fs::write(
            profile.join("profile.json"),
            r#"{"v":1,"name":"Ann","game":"../Game.nup","upscalers":["fsr4"],"start_slot":"slot_02","mods":[]}"#,
        )
        .unwrap();
        // (a folder built before the record kept the features: its version file's line)
        let folder = d.join("builds/default");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("longshot_version.txt"), "version=x\nfeatures=save_dir,profile\n").unwrap();
        let me = Me {
            exe: &exe,
            release: Some("nightly-12-aaaaaaaa"),
            source: Some("abc"),
            stamp: "nightly-12-aaaaaaaa abc",
        };
        let m = answer(&me, &profile, &|_| None);
        let keys: Vec<&str> = m.keys().map(String::as_str).collect();
        for k in ["version", "program", "platform", "capabilities", "profile", "build", "features", "game", "upscalers"]
        {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }
        assert_eq!(m["version"]["channel"], "nightly");
        assert_eq!(m["capabilities"]["game"], json!(["save_dir", "profile"]));
        assert!(m["capabilities"]["builder"].as_array().unwrap().contains(&json!("fetch")));
        assert_eq!(m["profile"]["name"], "Ann");
        assert_eq!(m["profile"]["start_slot"], 2);
        assert_eq!(m["profile"]["keys"], json!(["v", "name", "game", "upscalers", "start_slot", "mods"]));
        assert_eq!(m["upscalers"], json!(["fsr4"]));
        assert_eq!(m["game"]["exists"], true);
        assert_eq!((m["build"]["present"].clone(), m["build"]["up_to_date"].clone()), (json!(false), json!(false)));
        assert_eq!(m["build"]["stale"], "not built yet");
        // (as an event: one JSON line of type info)
        let line = serde_json::to_string(&Event::Info { fields: m }).unwrap();
        assert!(line.starts_with(r#"{"type":"info","#) && line.contains(r#""capabilities":{"#), "{line}");
        // (no profile: an answer still)
        let m = answer(&me, &d.join("none"), &|_| None);
        assert_eq!(m["profile"]["exists"], false);
        assert_eq!(m["capabilities"]["game"], json!(["save_dir", "profile"]));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
