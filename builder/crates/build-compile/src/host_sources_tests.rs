//! The host's C files the game compiles ([`host_sources`]): the game's, game/ beside host/, among them (the frame
//! phases, game/phases.c, which tests/test_game_phases.py builds and runs, and the menu rows, game/menu/), on both OSes.

use super::*;

#[test]
fn the_game_compiles_game_s_files_on_both_oses() {
    let d = crate::testdir("host-sources");
    let (host, posix) = (d.join("host"), d.join("host/posix"));
    for f in [
        "host/hos_main.c",
        "host/pgo_rt.c",
        "host/display.c",
        "host/dav1d_av1.c",
        "host/hos.h",
        "game/phases.c",
        "game/phases.h",
        "game/menu/rows.c",
    ] {
        let p = d.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    let names = |linux, dav1d| -> Vec<String> {
        host_sources(&host, &posix, linux, dav1d)
            .iter()
            .map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    };
    assert_eq!(names(false, false), ["host/display.c", "host/hos_main.c", "game/phases.c", "game/menu/rows.c"]);
    assert_eq!(
        names(false, true),
        ["host/dav1d_av1.c", "host/display.c", "host/hos_main.c", "game/phases.c", "game/menu/rows.c"]
    );
    assert_eq!(
        names(true, false),
        [
            "host/hos_main.c",
            "game/phases.c",
            "game/menu/rows.c",
            "host/posix/win32_posix.c",
            "host/posix/display_sdl.c",
            "host/posix/linux_stubs.c",
            "host/posix/prof.c"
        ]
    );
    // (the checkout's own)
    let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../host");
    for linux in [false, true] {
        assert!(
            host_sources(&real, &real.join("posix"), linux, false).iter().any(|p| p.ends_with("game/phases.c")),
            "game/phases.c is not compiled (linux: {linux})"
        );
    }
}
