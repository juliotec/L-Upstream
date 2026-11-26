//! `longshot-builder dev release recomplauncher <program> --out <dir> --root <checkout>`, as the build jobs run
//! it: the zip it writes holds the program under the name players start and the checkout's own recomplauncher.json,
//! byte for byte (what the zip is made of, member by member, is release_cmd.rs's
//! the_recomplauncher_zip_carries_the_program_and_recomplauncher_json).

use std::path::Path;

#[test]
fn the_command_zips_the_checkout_s_recomplauncher_json() {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("release_recomplauncher");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let program = d.join("longshot-builder-v3.2.0-linux-x64");
    std::fs::write(&program, b"the program").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_longshot-builder"))
        .args(["dev", "release", "recomplauncher"])
        .arg(&program)
        .arg("--out")
        .arg(&d)
        .arg("--root")
        .arg(&checkout)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let zip = Path::new(String::from_utf8(out.stdout).unwrap().trim()).to_path_buf();
    assert_eq!(zip, d.join("longshot-builder-v3.2.0-linux-x64-recomplauncher.zip"));
    let bytes = std::fs::read(&zip).unwrap();
    let json = std::fs::read(checkout.join("recomplauncher.json")).unwrap();
    // (a stored zip: each member's name and bytes as they are)
    let has = |what: &[u8]| bytes.windows(what.len()).any(|w| w == what);
    assert!(has(b"Longshot.x86_64") && has(b"the program"), "the program players start");
    assert!(has(b"recomplauncher.json") && has(&json), "the checkout's recomplauncher.json, as it is");
    let _ = std::fs::remove_dir_all(&d);
}
