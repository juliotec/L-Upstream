//! Each check's decision, from facts made here (no Vulkan, no registry, no real disk asked).

use super::*;

fn gpu(name: &str, api: u32, kind: u32, driver_version: u32) -> Gpu {
    Gpu {
        name: name.into(),
        api,
        kind,
        driver_version,
        vendor: 0x1002,
        device: 0x73df,
        driver_id: 0,
        driver_name: String::new(),
        driver_info: String::new(),
    }
}

fn answered(gpus: Vec<Gpu>) -> Probe {
    Probe::Answered(Vulkan {
        loader_file: Some("libvulkan.so.1".into()),
        loader: Some(vulkan::version(1, 4)),
        instance: true,
        instance_result: 0,
        gpus,
    })
}

fn statuses(c: &[DoctorCheck]) -> Vec<(&str, &str)> {
    c.iter().map(|c| (c.id, c.status)).collect()
}

/// A folder of the test's own, under the target folder.
fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/test-scratch")
        .join(format!("doctor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn vulkan_as_the_game_asks_it_at_its_start() {
    let none = NoGpu::default();
    let v13 = vulkan::version(1, 3);
    // (a good PC: the discrete GPU picked over the integrated one, its driver said)
    let mut card = gpu("Radeon RX 6700 XT", vulkan::version(1, 4), 2, 0x8001a0);
    card.driver_name = "AMD proprietary driver".into();
    card.driver_info = "26.10.1".into();
    let mut apu = gpu("Radeon 680M", v13, 1, 9);
    apu.device = 0x1681;
    let c = vulkan_checks(&answered(vec![apu, card.clone()]), &none, true);
    assert_eq!(statuses(&c), [("vulkan", "ok"), ("gpu", "ok"), ("driver", "ok")]);
    assert!(c[1].text.starts_with("Radeon RX 6700 XT, Vulkan 1.4 (1 more listed)"), "{}", c[1].text);
    assert!(c[2].text.contains("AMD proprietary driver 26.10.1"), "{}", c[2].text);
    // (no loader; an old loader; no instance)
    let no_loader = Probe::Answered(Vulkan::default());
    let c = vulkan_checks(&no_loader, &none, true);
    assert_eq!(statuses(&c), [("vulkan", "fail")]);
    assert!(c[0].text.contains("vulkan-1.dll") && c[0].fix.contains("NVIDIA, AMD or Intel"));
    let old = Probe::Answered(Vulkan { loader: Some(vulkan::version(1, 2)), ..Vulkan::default() });
    assert!(vulkan_checks(&old, &none, false)[0].text.contains("1.2: the game needs 1.3"));
    let refused = Probe::Answered(Vulkan { loader: Some(v13), instance_result: -9, ..Vulkan::default() });
    assert!(vulkan_checks(&refused, &none, false)[0].text.contains("error -9"));
    // (a driver that crashed or hung in the asking)
    let c = vulkan_checks(&Probe::Failed("crashed (signal 11)".into()), &none, false);
    assert_eq!(statuses(&c), [("vulkan", "fail")]);
    assert!(c[0].fix.starts_with("Reinstall"));
    // (no GPU: Linux's why, as the game's start says it)
    let rt = NoGpu { cause: "nvidia-rt", cards: "10de:2d05".into() };
    let c = vulkan_checks(&answered(vec![]), &rt, false);
    assert_eq!(statuses(&c), [("vulkan", "ok"), ("gpu", "fail")]);
    assert!(c[1].text.contains("real-time kernel") && c[1].text.contains("10de:2d05"), "{}", c[1].text);
    // (a GPU below 1.3 alone)
    let c = vulkan_checks(&answered(vec![gpu("Old GPU", vulkan::version(1, 2), 2, 1)]), &none, false);
    assert_eq!(statuses(&c), [("vulkan", "ok"), ("gpu", "fail")]);
    assert!(c[1].text.contains("Old GPU offers Vulkan 1.2"));
    // (a software renderer alone)
    let c = vulkan_checks(&answered(vec![gpu("llvmpipe (LLVM 20.1.8, 256 bits)", v13, 4, 1)]), &none, false);
    assert_eq!(c[2].status, "fail");
    // (the same card twice, by two drivers: the newer one used, said)
    let c = vulkan_checks(&answered(vec![gpu("RX 7700S", v13, 2, 100), gpu("RX 7700S", v13, 2, 200)]), &none, true);
    assert_eq!(c[2].status, "warn");
    assert_eq!(picked(&[gpu("a", v13, 2, 100), gpu("b", v13, 2, 200)]).unwrap().name, "b");
}

#[test]
fn no_gpu_reads_as_the_game_s_header_does() {
    let d = scratch("no-gpu");
    let card = |slot: &str, class: &str, vendor: &str, device: &str| {
        let dir = d.join("sys/bus/pci/devices").join(slot);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in [("class", class), ("vendor", vendor), ("device", device)] {
            std::fs::write(dir.join(f), format!("{v}\n")).unwrap();
        }
    };
    assert_eq!(no_gpu(&d), NoGpu { cause: "unknown", cards: String::new() });
    card("0000:01:00.0", "0x030000", "0x10de", "0x2d05");
    card("0000:00:1f.0", "0x060100", "0x8086", "0x7a04");
    assert_eq!(no_gpu(&d), NoGpu { cause: "nvidia-unloaded", cards: "10de:2d05".into() });
    std::fs::create_dir_all(d.join("proc/sys/kernel")).unwrap();
    std::fs::write(d.join("proc/sys/kernel/version"), "#1 SMP PREEMPT_RT Tue Nov 4\n").unwrap();
    assert_eq!(no_gpu(&d).cause, "nvidia-rt");
    std::fs::create_dir_all(d.join("sys/module/nvidia")).unwrap();
    assert_eq!(no_gpu(&d).cause, "nvidia-vulkan");
    card("0000:05:00.0", "0x030000", "0x1002", "0x164e");
    assert_eq!(no_gpu(&d), NoGpu { cause: "unknown", cards: "10de:2d05 1002:164e".into() }, "another card too");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn disk_space_by_drive_the_places_on_one_adding_up() {
    let room = |what, need, free, volume: &str| Room {
        what,
        path: PathBuf::from(format!("/{what}")),
        need,
        free,
        volume: volume.into(),
    };
    let [cache, builds, profile] = needs(true, true);
    assert_eq!((cache, builds, profile), (3 * GB + 6 * GB / 10, GB, GB / 10), "a release's first build");
    assert_eq!(needs(false, false)[0], 2 * GB);
    // (one drive with 4.5 GB: the three need 4.7 together)
    let one = [
        room("cache", cache, Some(4 * GB + GB / 2), "a"),
        room("game folder", builds, Some(4 * GB + GB / 2), "a"),
        room("profile", profile, Some(4 * GB + GB / 2), "a"),
    ];
    let c = disk_check(&one);
    assert_eq!(c.status, "warn");
    assert!(c.text.contains("4.5 GB free") && c.text.contains("about 4.7 GB"), "{}", c.text);
    // (the cache on a drive of its own with room: fine)
    let two = [
        room("cache", cache, Some(10 * GB), "b"),
        room("game folder", builds, Some(2 * GB), "a"),
        room("profile", profile, Some(2 * GB), "a"),
    ];
    assert_eq!(disk_check(&two).status, "ok");
    assert_eq!(disk_check(&[room("cache", cache, None, "a")]).status, "unknown");
}

#[test]
fn the_game_copy_found_missing_unreadable_or_another_version() {
    let p = PathBuf::from("/g/Game.nup");
    assert_eq!(copy_check(&GameCopy::NotSet(None)).status, "warn");
    assert!(copy_check(&GameCopy::NotSet(None)).fix.contains("--game"));
    assert_eq!(copy_check(&GameCopy::NotSet(Some("newer format".into()))).status, "fail");
    let c = copy_check(&GameCopy::Missing(p.clone()));
    assert!(c.status == "fail" && c.text.contains("/g/Game.nup") && c.fix.contains("--game"));
    assert_eq!(copy_check(&GameCopy::Unreadable(p.clone(), "locked".into())).status, "fail");
    let ok = GameCopy::Checked { path: p.clone(), ok: true, text: "the supported build".into() };
    assert_eq!(copy_check(&ok).status, "ok");
    let other = GameCopy::Checked { path: p, ok: false, text: "build 1234".into() };
    assert!(copy_check(&other).fix.contains("supported version"));
}

#[test]
fn a_profile_s_copy_is_read_and_checked() {
    let d = scratch("copy");
    let prof = d.join("profile");
    assert_eq!(game_copy(&prof), GameCopy::NotSet(None), "no profile yet");
    std::fs::create_dir_all(&prof).unwrap();
    std::fs::write(prof.join("profile.json"), r#"{"v":1,"name":"Ann","game":"gone.nup"}"#).unwrap();
    assert_eq!(game_copy(&prof), GameCopy::Missing(prof.join("gone.nup")));
    std::fs::write(prof.join("gone.nup"), b"not a nup").unwrap();
    assert!(matches!(game_copy(&prof), GameCopy::Unreadable(..)), "{:?}", game_copy(&prof));
    std::fs::write(prof.join("profile.json"), r#"{"v":9}"#).unwrap();
    assert!(matches!(game_copy(&prof), GameCopy::NotSet(Some(_))));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn folders_writable_or_said() {
    let d = scratch("folders");
    assert_eq!(writable(&d), Ok(()));
    assert_eq!(writable(&d.join("not/made/yet")), Ok(()), "the folder above it");
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "nothing left behind");
    let ok = [("cache", d.clone(), Ok(())), ("profile", d.clone(), Ok(()))];
    assert_eq!(folders_check(&ok).status, "ok");
    let bad = [("cache", d.clone(), Ok(())), ("game folder", PathBuf::from("/ro"), Err("Permission denied".into()))];
    let c = folders_check(&bad);
    assert!(c.status == "fail" && c.text.contains("game folder (/ro): Permission denied"), "{}", c.text);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn long_paths_on_or_the_deepest_path_short_enough() {
    let short = Path::new(r"C:\Games\Longshot\cache");
    assert_eq!(long_paths_check(Some(true), short).status, "ok");
    assert_eq!(long_paths_check(Some(false), short).status, "ok");
    let near = PathBuf::from(format!(r"C:\{}", "a".repeat(MAX_PATH - DEEPEST_IN_CACHE - 15)));
    assert_eq!(long_paths_check(Some(false), &near).status, "warn");
    let long = PathBuf::from(format!(r"C:\{}", "a".repeat(MAX_PATH)));
    let c = long_paths_check(None, &long);
    assert!(c.status == "fail" && c.fix.contains("LongPathsEnabled"));
    assert_eq!(long_paths_check(Some(true), &long).status, "ok");
}

#[test]
fn defender_read_only_and_explained() {
    let cache = Path::new(r"C:\Games\Longshot\cache");
    let off = Defender { present: true, realtime_off: true, exclusions: None };
    assert_eq!(defender_check(&off, cache).status, "ok");
    let excluded =
        Defender { present: true, realtime_off: false, exclusions: Some(vec![r"c:\games\longshot\".into()]) };
    assert_eq!(defender_check(&excluded, cache).status, "ok");
    let other = Defender { present: true, realtime_off: false, exclusions: Some(vec![r"C:\Games\Long".into()]) };
    let c = defender_check(&other, cache);
    assert_eq!(c.status, "warn", "C:\\Games\\Long is not the cache's folder");
    assert!(
        c.fix.contains("thousands of files")
            && c.fix.contains("Add-MpPreference")
            && c.fix.contains("changes no setting")
    );
    let hidden = Defender { present: true, realtime_off: false, exclusions: None };
    assert_eq!(defender_check(&hidden, cache).status, "unknown");
    assert_eq!(defender_check(&Defender::default(), cache).status, "unknown");
    // (from the registry: the key there, real-time on, the exclusions shown to an administrator)
    let admin = |k: &str| -> (Vec<(String, String)>, Vec<String>) {
        match k {
            r"SOFTWARE\Microsoft\Windows Defender" => {
                (vec![("ProductStatus".into(), "0".into())], vec!["Exclusions".into()])
            }
            r"SOFTWARE\Microsoft\Windows Defender\Exclusions" => (vec![], vec!["Paths".into(), "Processes".into()]),
            r"SOFTWARE\Policies\Microsoft\Windows Defender\Exclusions\Paths" => {
                (vec![(r"C:\Games".into(), "0".into())], vec![])
            }
            _ => (vec![], vec![]),
        }
    };
    assert_eq!(
        defender(admin),
        Defender { present: true, realtime_off: false, exclusions: Some(vec![r"C:\Games".into()]) }
    );
    let user = |k: &str| -> (Vec<(String, String)>, Vec<String>) {
        match k {
            r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection" => {
                (vec![("DisableRealtimeMonitoring".into(), "1".into())], vec![])
            }
            r"SOFTWARE\Microsoft\Windows Defender" => (vec![("ProductStatus".into(), "0".into())], vec![]),
            _ => (vec![], vec![]),
        }
    };
    assert_eq!(defender(user), Defender { present: true, realtime_off: true, exclusions: None });
}

#[test]
fn the_game_running_is_the_profile_s_lock() {
    let p = Path::new("/p/ann");
    assert_eq!(running_check(false, Some(7), p).status, "ok");
    let c = running_check(true, Some(4242), p);
    assert!(c.status == "warn" && c.text.contains("process 4242"));
    // (and the lock read as play holds it)
    let d = scratch("running");
    let held = build_core::lock::ProfileLock::take(&d).unwrap().unwrap();
    let ann = d.clone();
    let seen = std::thread::spawn(move || build_core::lock::ProfileLock::held(&ann)).join().unwrap();
    assert!(seen);
    drop(held);
    std::fs::remove_dir_all(&d).unwrap();
}
