//! The game's executable: its name on each OS, and the Windows game's resources (`exe_resources`, which its link
//! takes): the manifest is always among them, as `longshot_manifest.res` in the build's folder with
//! `res::manifest_res()`'s bytes, beside the icon when the tree has one (tests/test_utf8_paths.py checks the manifest
//! itself).

use super::*;

fn options(name: &str) -> Options {
    let d = crate::testdir(name);
    for sub in ["root", "work", "gen", "build"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
    }
    Options {
        root: d.join("root"),
        work: d.join("work"),
        gen_dir: d.join("gen"),
        build_dir: d.join("build"),
        mode: Mode::Game,
        jobs: Some(1),
        store: None,
        settings: Settings::from_lookup(|_| None),
        ir_text: None,
        early: None,
        gate: None,
    }
}

#[test]
fn the_windows_game_links_the_manifest_with_or_without_an_icon() {
    let o = options("exe-resources");
    let l = Layout::of(&o);
    let manifest = l.out.join("longshot_manifest.res");
    assert_eq!(exe_resources(&o, &l).unwrap(), vec![manifest.clone()], "no icon in the tree: the manifest alone");
    assert_eq!(std::fs::read(&manifest).unwrap(), generated::res::manifest_res());

    // (an icon: Longshot's brand first, the manifest still last)
    let png = b"\x89PNG\r\n\x1a\nxyz";
    let mut ico = vec![0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    std::fs::create_dir_all(o.root.join("assets")).unwrap();
    std::fs::write(o.root.join("assets/longshot.ico"), &ico).unwrap();
    assert_eq!(exe_resources(&o, &l).unwrap(), vec![o.work.join("longshot_brand.res"), manifest]);
    let _ = std::fs::remove_dir_all(o.root.parent().unwrap());
}

#[test]
fn the_linux_game_is_named_longshot() {
    // (NVIDIA's Linux driver files its shader cache by the program's file name alone: tests/test_driver_cache_folder.py)
    assert_eq!(game_exe_name(true), "longshot");
    assert_eq!(game_exe_name(false), "longshot.exe");
}
