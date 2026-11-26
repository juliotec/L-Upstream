//! The library's write and read round trip, ours only, no duplicates, removal, a moved program keeping its tile, the
//! art, and where Steam is: all on libraries made here, under the target folder (never a real Steam's).

use super::*;

/// A folder of the test's own, under the target folder.
fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/test-scratch")
        .join(format!("steam-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn library(d: &Path) -> Library {
    let vdf = d.join("Steam/userdata/33811576/config/shortcuts.vdf");
    std::fs::create_dir_all(vdf.parent().unwrap()).unwrap();
    Library { home: d.join("Steam"), vdf }
}

/// The tiles of the library: (key, id, name, Exe, launch options).
fn tiles(lib: &Library) -> Vec<(String, i32, String, String, String)> {
    let root = vdf::parse(&std::fs::read(&lib.vdf).unwrap()).unwrap();
    let Some(Value::Map(s)) = root.get("shortcuts") else { panic!("no shortcuts: {root:?}") };
    s.0.iter()
        .map(|(k, v)| {
            let Value::Map(t) = v else { panic!() };
            (k.clone(), t.int("appid"), t.str("AppName").into(), t.str("Exe").into(), t.str("LaunchOptions").into())
        })
        .collect()
}

/// A library with one tile of somebody else's.
fn theirs(lib: &Library, name: &str) {
    let mut t = new_tile(&Entry::new(Path::new("/opt/emu/emu.AppImage"), name, ""));
    t.set("LastPlayTime", Value::Int(1_700_000_000));
    let root = Map(vec![("shortcuts".into(), Value::Map(Map(vec![("0".into(), Value::Map(t))])))]);
    std::fs::write(&lib.vdf, vdf::dump(&root)).unwrap();
}

#[test]
fn the_id_is_steam_s_own() {
    // (the id Steam itself wrote on a Steam Deck for this tile: the Longshot Launcher's fixture)
    let exe = "\"/home/deck/Emulation/tools/EmulationStation-DE-x64_SteamDeck.AppImage\"";
    assert_eq!(app_id(exe, "EmulationStation-DE-x64_SteamDeck"), -1467123759);
    assert_eq!(grid_id64(-1467123759), (u64::from((-1467123759i32) as u32) << 32) | 0x0200_0000);
    let e = Entry::new(Path::new("/games/Long shot/Longshot.x86_64"), NAME, "");
    assert_eq!(e.exe, "\"/games/Long shot/Longshot.x86_64\"", "quoted with a space, as Steam writes it");
    assert_eq!(e.start_dir, "\"/games/Long shot\"");
    assert_eq!(Entry::new(Path::new("/g/L"), NAME, "").exe, "/g/L");
    assert_eq!(argument(r#"/p/a "b""#), r#""/p/a \"b\"""#);
}

#[test]
fn a_tile_written_reads_back_and_is_never_added_twice() {
    let d = scratch("round-trip");
    let lib = library(&d);
    theirs(&lib, "Emu");
    let before = std::fs::read(&lib.vdf).unwrap();
    let mut e = Entry::new(&d.join("v1/Longshot.x86_64"), NAME, "--profile \"/p/ann\"");
    e.icon = "/s/grid/1_icon.ico".into();
    assert!(!in_library(&lib, &e));
    assert_eq!(add(&lib, &e).unwrap(), Done::Added(e.app_id()));
    let t = tiles(&lib);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].2, "Emu", "theirs kept, first");
    assert_eq!(t[1], ("1".into(), e.app_id(), NAME.into(), e.exe.clone(), "--profile \"/p/ann\"".into()));
    // (the file as a whole: Steam's layout, read back to the same bytes; their tile's bytes kept as they were)
    let after = std::fs::read(&lib.vdf).unwrap();
    assert_eq!(vdf::dump(&vdf::parse(&after).unwrap()), after);
    let theirs_bytes = &before[..before.len() - 2]; // (the library less its two closing 0x08s)
    assert!(after.starts_with(theirs_bytes));
    assert!(in_library(&lib, &e));
    // (again: nothing written, no second tile)
    assert_eq!(add(&lib, &e).unwrap(), Done::Unchanged(e.app_id()));
    assert_eq!(tiles(&lib).len(), 2);
    // (another profile in the launch options: the same tile, changed; its id kept)
    e.launch_options = "--profile \"/p/bo\"".into();
    assert_eq!(add(&lib, &e).unwrap(), Done::Updated(e.app_id()));
    let t = tiles(&lib);
    assert_eq!((t.len(), t[1].4.as_str()), (2, "--profile \"/p/bo\""));
    // (the library before the first change, kept once beside it)
    assert_eq!(std::fs::read(beside(&lib.vdf, BACKUP)).unwrap(), before);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_tile_of_that_name_not_ours_is_left_alone() {
    let d = scratch("name-taken");
    let lib = library(&d);
    theirs(&lib, NAME);
    let before = std::fs::read(&lib.vdf).unwrap();
    let e = Entry::new(&d.join("v1/Longshot.x86_64"), NAME, "");
    assert_eq!(add(&lib, &e).unwrap(), Done::NameTaken);
    assert_eq!(std::fs::read(&lib.vdf).unwrap(), before, "not a byte changed");
    assert!(!remove(&lib, &e).unwrap(), "and never removed");
    assert_eq!(std::fs::read(&lib.vdf).unwrap(), before);
    // (another name: its own tile beside theirs)
    let named = Entry::new(&d.join("v1/Longshot.x86_64"), "Longshot (Ann)", "");
    assert_eq!(add(&lib, &named).unwrap(), Done::Added(named.app_id()));
    assert_eq!(tiles(&lib).len(), 2);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn remove_takes_ours_out_and_leaves_the_rest() {
    let d = scratch("remove");
    let lib = library(&d);
    let e = Entry::new(&d.join("v1/Longshot.x86_64"), NAME, "");
    assert!(!remove(&lib, &e).unwrap(), "no library: nothing to remove");
    theirs(&lib, "Emu");
    add(&lib, &e).unwrap();
    assert!(beside(&lib.vdf, NOTE).is_file());
    assert!(remove(&lib, &e).unwrap());
    let t = tiles(&lib);
    assert_eq!((t.len(), t[0].2.as_str()), (1, "Emu"));
    assert!(!beside(&lib.vdf, NOTE).exists(), "the note goes with our last tile");
    assert!(!remove(&lib, &e).unwrap());
    // (a key freed in the middle is taken again: never two tiles under one key)
    add(&lib, &e).unwrap();
    let other = Entry::new(&d.join("v1/Longshot.x86_64"), "Longshot (Bo)", "");
    add(&lib, &other).unwrap();
    remove(&lib, &e).unwrap();
    add(&lib, &e).unwrap();
    let keys: Vec<String> = tiles(&lib).into_iter().map(|t| t.0).collect();
    assert_eq!(keys, ["0", "2", "1"]);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_moved_version_folder_keeps_its_tile_and_its_id() {
    let d = scratch("moved");
    let lib = library(&d);
    let old = Entry::new(&d.join("v1/Longshot.x86_64"), NAME, "");
    add(&lib, &old).unwrap();
    let new = Entry::new(&d.join("v2/Longshot.x86_64"), NAME, "");
    assert_ne!(old.app_id(), new.app_id());
    assert_eq!(tile_id(&lib, &new).unwrap(), old.app_id(), "the art goes under the id the tile carries");
    assert_eq!(add(&lib, &new).unwrap(), Done::Updated(old.app_id()));
    let t = tiles(&lib);
    assert_eq!((t.len(), t[0].1, t[0].3.as_str()), (1, old.app_id(), new.exe.as_str()));
    // (and again from a third place: the note still vouches for it by the id's origin)
    let third = Entry::new(&d.join("v3/Longshot.x86_64"), NAME, "");
    assert_eq!(add(&lib, &third).unwrap(), Done::Updated(old.app_id()));
    assert!(remove(&lib, &third).unwrap());
    assert!(tiles(&lib).is_empty());
    // (a tile the player pointed elsewhere in Steam keeps its id but is no longer ours)
    add(&lib, &old).unwrap();
    let mut root = vdf::parse(&std::fs::read(&lib.vdf).unwrap()).unwrap();
    let Some((_, Value::Map(t))) = root.map_mut("shortcuts").0.first_mut() else { panic!() };
    t.set("Exe", Value::Str("/home/p/my-script.sh".into()));
    std::fs::write(&lib.vdf, vdf::dump(&root)).unwrap();
    assert_eq!(add(&lib, &new).unwrap(), Done::NameTaken);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_library_that_does_not_read_whole_is_left_as_it_is() {
    let d = scratch("damaged");
    let lib = library(&d);
    theirs(&lib, "Emu");
    let whole = std::fs::read(&lib.vdf).unwrap();
    std::fs::write(&lib.vdf, &whole[..whole.len() - 3]).unwrap();
    let e = Entry::new(&d.join("v1/Longshot.x86_64"), NAME, "");
    assert!(matches!(add(&lib, &e), Err(Error::Unreadable(_))));
    assert!(matches!(remove(&lib, &e), Err(Error::Unreadable(_))));
    assert_eq!(std::fs::read(&lib.vdf).unwrap(), &whole[..whole.len() - 3]);
    // (bytes that hold no shortcuts map: not a library)
    std::fs::write(&lib.vdf, vdf::dump(&Map(vec![("other".into(), Value::Int(1))]))).unwrap();
    assert!(matches!(add(&lib, &e), Err(Error::Unreadable(_))));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
#[cfg(unix)]
fn a_library_that_is_a_link_is_written_through_it() {
    let d = scratch("link");
    let lib = library(&d);
    let shared = d.join("shared.vdf");
    theirs(&Library { home: lib.home.clone(), vdf: shared.clone() }, "Emu");
    std::os::unix::fs::symlink(&shared, &lib.vdf).unwrap();
    let e = Entry::new(&d.join("v1/Longshot.x86_64"), NAME, "");
    add(&lib, &e).unwrap();
    assert!(std::fs::symlink_metadata(&lib.vdf).unwrap().file_type().is_symlink(), "still a link");
    assert_eq!(tiles(&lib).len(), 2);
    std::fs::remove_file(&lib.vdf).unwrap();
    std::os::unix::fs::symlink(d.join("nowhere"), &lib.vdf).unwrap();
    assert!(matches!(add(&lib, &e), Err(Error::BrokenLink(_))));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn the_art_goes_under_both_names_only_where_the_slot_is_empty() {
    let d = scratch("art");
    let grid = d.join("grid");
    let id = app_id("/g/L", NAME);
    let (n32, n64) = ((id as u32).to_string(), grid_id64(id).to_string());
    std::fs::create_dir_all(&grid).unwrap();
    // (the player's portrait capsule, a .jpg under the 64-bit name; a cut publish of the hero)
    std::fs::write(grid.join(format!("{n64}p.jpg")), b"theirs").unwrap();
    std::fs::write(grid.join(format!("{n32}_hero.png.tmp")), b"cut").unwrap();
    let art: [Art; 3] = [("p", "png", b"portrait"), ("_hero", "png", b"hero"), ("_icon", "ico", b"icon")];
    assert_eq!(write_artwork(&grid, id, &art).unwrap(), 4);
    assert!(!grid.join(format!("{n32}p.png")).exists(), "their slot stays theirs");
    assert_eq!(std::fs::read(grid.join(format!("{n32}_hero.png"))).unwrap(), b"hero");
    assert_eq!(std::fs::read(grid.join(format!("{n64}_icon.ico"))).unwrap(), b"icon");
    // (again: everything held, nothing written, nothing replaced)
    std::fs::write(grid.join(format!("{n32}_hero.png")), b"chosen in Steam").unwrap();
    assert_eq!(write_artwork(&grid, id, &art).unwrap(), 0);
    assert_eq!(std::fs::read(grid.join(format!("{n32}_hero.png"))).unwrap(), b"chosen in Steam");
    let left: Vec<String> =
        std::fs::read_dir(&grid).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into()).collect();
    assert!(!left.iter().any(|f| f.ends_with(".part")), "{left:?}");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn steam_is_found_in_its_folders_and_the_signed_in_account_chosen() {
    let d = scratch("locate");
    let home = d.join("home");
    let env = |k: &str| match k {
        "HOME" => Some(home.to_string_lossy().into_owned()),
        _ => None,
    };
    let h = homes(&env, false, None);
    assert_eq!(h[0], home.join(".local/share/Steam"));
    assert!(h.contains(&home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam")), "Flatpak's");
    assert_eq!(locate(&h), None, "no Steam: nothing, and nothing made");
    assert!(!home.exists());
    // (Flatpak's alone, two accounts: loginusers.vdf's MostRecent wins over the newer folder)
    let steam = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
    for id in ["1001", "33811576"] {
        std::fs::create_dir_all(steam.join("userdata").join(id).join("config")).unwrap();
    }
    std::fs::create_dir_all(steam.join("config")).unwrap();
    let users = "\"users\"\n{\n\t\"76561198027709306\"\n\t{\n\t\t\"AccountName\"\t\t\"p\"\n\t\t\"MostRecent\"\t\t\"1\"\n\t}\n}\n";
    std::fs::write(steam.join("config/loginusers.vdf"), users).unwrap();
    // (76561198027709306 & 0xFFFFFFFF = 67443578: no such folder, so the newest)
    let found = locate(&h).unwrap();
    assert_eq!(found.home, steam);
    assert!(found.vdf.ends_with("config/shortcuts.vdf"));
    std::fs::create_dir_all(steam.join("userdata/67443578/config")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::create_dir_all(steam.join("userdata/1001/config/x")).unwrap();
    assert_eq!(locate(&h).unwrap().vdf, steam.join("userdata/67443578/config/shortcuts.vdf"), "MostRecent's");
    std::fs::remove_file(steam.join("config/loginusers.vdf")).unwrap();
    assert_eq!(locate(&h).unwrap().vdf, steam.join("userdata/1001/config/shortcuts.vdf"), "else the newest");
    assert_eq!(found.grid(), found.vdf.parent().unwrap().join("grid"));
    // (Windows: the registry's folder, then Program Files')
    let win = |k: &str| (k == "ProgramFiles(x86)").then(|| r"C:\Program Files (x86)".to_string());
    let w = homes(&win, true, Some(r"D:\Steam".into()));
    assert_eq!(w, [PathBuf::from(r"D:\Steam"), Path::new(r"C:\Program Files (x86)").join("Steam")]);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn steam_and_its_gaming_mode_hold_the_library() {
    for n in ["steam", "steam.exe", "Steam.exe", "steamrt64", "gamescope"] {
        assert!(is_steam(n), "{n}");
    }
    for n in ["steamwebhelper", "steam-runtime-launcher-service", "longshot", "steam.sh"] {
        assert!(!is_steam(n), "{n}");
    }
}
