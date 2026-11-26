use super::*;

/// A scratch folder of the test's own (`<target>/test-scratch/launcher-profile-<name>-<pid>`: on the disk, never the
/// system's temporary folder, which may be memory).
fn scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |t| root.join(t));
    let d = target.join("test-scratch").join(format!("launcher-profile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The launcher's own check file's profile (LongshotLauncher's ProfileModelChecks), with a key of the builder's and
/// the Mod API's `"mods"` (a key this crate keeps without reading).
const ANA: &str = r##"{ "v": 1, "name": "Ana", "builder_note": {"kept": [1, 2], "z": 0, "a": 1}, "mods": [ {"id": "local/a", "on": false, "granted": ["network"]}, {"id": "b"} ],
  "upscalers": ["dlss", "xess"], "game": "/games/oot.nup", "start_slot": "slot_02" }"##;

#[test]
fn the_known_keys_are_read() {
    let p = Profile::parse(ANA).unwrap();
    assert_eq!(p.name().as_deref(), Some("Ana"));
    assert_eq!(p.game().as_deref(), Some("/games/oot.nup"));
    assert_eq!(p.start_slot(), Some(Ok(2)));
    assert_eq!(p.upscalers(), Ok(vec![Upscaler::Dlss, Upscaler::Xess]));
    assert_eq!(p.version(), None, "none: the launcher's default");
    assert_eq!((p.color(), p.last_played(), p.copy_of()), (None, None, None));
}

#[test]
fn a_round_trip_keeps_unknown_keys_and_their_order() {
    let mut p = Profile::parse(ANA).unwrap();
    let out = p.to_text();
    assert_eq!(Profile::parse(&out).unwrap().to_text(), out, "written again: the same");
    let order: Vec<&str> = p.keys().collect();
    assert_eq!(order, ["v", "name", "builder_note", "mods", "upscalers", "game", "start_slot"]);
    assert!(out.contains(r#""builder_note": {"kept": [1, 2], "z": 0, "a": 1}"#), "an unknown value as written: {out}");
    // (what is changed changes in its place; a new key goes last)
    p.set_string("name", Some("Ana B"));
    p.set_string("color", Some("#7CC242"));
    p.set_upscalers(&[Upscaler::Fsr4, Upscaler::Fsr4]);
    let again = Profile::parse(&p.to_text()).unwrap();
    let order: Vec<&str> = again.keys().collect();
    assert_eq!(order, ["v", "name", "builder_note", "mods", "upscalers", "game", "start_slot", "color"]);
    assert_eq!(again.upscaler_names(), ["fsr4"]);
    assert_eq!(again.raw("builder_note").unwrap().get(), r#"{"kept": [1, 2], "z": 0, "a": 1}"#);
    assert_eq!(
        again.raw("mods").unwrap().get(),
        r#"[ {"id": "local/a", "on": false, "granted": ["network"]}, {"id": "b"} ]"#,
        "the Mod API's mods as they were"
    );
    p.set_string("color", None);
    assert!(!p.keys().any(|k| k == "color"));
    // (a key of another's put as its JSON, in its place)
    p.put("mods", serde_json::value::RawValue::from_string("[]".into()).unwrap());
    assert_eq!(p.keys().position(|k| k == "mods"), Some(3));
    assert_eq!(p.raw("mods").unwrap().get(), "[]");
}

#[test]
fn not_a_profile() {
    for (t, want) in [
        ("[]", None),
        ("{\"v\": 1, \"v\": 1}", Some(Error::Twice("v".into()))),
        ("{\"v\": 2}", Some(Error::Newer(2))),
        ("{\"v\": \"1\"}", Some(Error::Shape("\"v\" is not 1".into()))),
    ] {
        let got = Profile::parse(t).unwrap_err();
        if let Some(w) = want {
            assert_eq!(got, w, "{t}");
        } else {
            assert!(matches!(got, Error::Json(_)), "{t}: {got:?}");
        }
    }
    assert!(Profile::parse("\u{feff}{\"v\": 1}").is_ok(), "a byte order mark before it");
    assert!(Profile::parse("{\"name\": \"x\"}").is_ok(), "no \"v\": v1");
    // (a key it does not know is never checked: the Mod API's mods, of whatever shape)
    assert!(Profile::parse("{\"v\": 1, \"mods\": {}}").is_ok());
    assert_eq!(
        Profile::parse("{\"v\": 1, \"upscalers\": [\"dlss\", \"dlss5\"]}").unwrap().upscalers(),
        Err("dlss5".into())
    );
    assert_eq!(Error::Newer(2).to_string(), "it is a newer profile (\"v\": 2) than this one reads");
}

#[test]
fn start_slots() {
    let slot = |v: &str| Profile::parse(&format!("{{\"v\": 1, \"start_slot\": {v}}}")).unwrap().start_slot();
    assert_eq!(slot("\"slot_07\""), Some(Ok(7)));
    assert_eq!(slot("\"SLOT_99\""), Some(Ok(99)));
    assert_eq!(slot("\" slot_05 \""), Some(Ok(5)));
    assert_eq!(slot("\"3\""), Some(Ok(3)));
    assert_eq!(slot("12"), Some(Ok(12)));
    assert_eq!(slot("null"), None);
    assert!(matches!(slot("\"slot_100\""), Some(Err(_))));
    assert!(matches!(slot("\"slot_\""), Some(Err(_))));
    assert!(matches!(slot("-1"), Some(Err(_))));
    assert!(matches!(slot("2.0"), Some(Err(_))));
    assert!(matches!(slot("true"), Some(Err(_))));
    assert_eq!(Profile::new().start_slot(), None);
}

#[test]
fn written_whole_and_read_back() {
    let d = scratch("write");
    let l = Layout::new(&d);
    assert_eq!(l.read(), Err(Error::Missing));
    let mut p = Profile::new();
    p.set_string("name", Some("Default"));
    p.set_string("game", Some("/g/oot.nup"));
    p.write(&l.file()).unwrap();
    assert_eq!(l.read().unwrap(), Profile::parse(&p.to_text()).unwrap());
    assert!(!d.join("profile.json.new").exists(), "the work file moved over it");
    assert_eq!(p.to_text(), "{\n  \"v\": 1,\n  \"name\": \"Default\",\n  \"game\": \"/g/oot.nup\"\n}\n");
    std::fs::write(l.file(), vec![b' '; FILE_MAX as usize]).unwrap();
    assert_eq!(l.read(), Err(Error::TooLarge));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn licences_are_set_in_place_and_keep_what_they_do_not_know() {
    let mut p = Profile::parse(r#"{"v": 1, "licenses": {"acme": "p0", "amd": 3}, "name": "Ana"}"#).unwrap();
    assert_eq!(p.licenses().into_iter().collect::<Vec<_>>(), [("acme".to_string(), "p0".to_string())], "text only");
    p.set_license("nvidia", "pin1");
    p.set_license("nvidia", "pin2");
    let again = Profile::parse(&p.to_text()).unwrap();
    assert_eq!(again.keys().collect::<Vec<_>>(), ["v", "licenses", "name"], "in its place");
    let l = again.licenses();
    assert_eq!((l.get("nvidia").map(String::as_str), l.get("acme").map(String::as_str)), (Some("pin2"), Some("p0")));
    assert!(again.raw("licenses").unwrap().get().contains(r#""amd":3"#), "a value it can't read kept");
    let mut q = Profile::new();
    q.set_license("intel", "x");
    assert_eq!(q.licenses().len(), 1);
}

#[test]
fn the_layout() {
    let l = Layout::new("/p/ana");
    assert!(l.dir().is_absolute());
    assert_eq!(l.file(), l.dir().join("profile.json"));
    assert_eq!(l.settings(), l.dir().join("settings.ini"));
    assert_eq!(l.saves(), l.dir().join("saves"));
    assert_eq!(l.mods(), l.dir().join("mods"));
    assert!(Layout::new("rel").dir().is_absolute(), "made whole");
}
