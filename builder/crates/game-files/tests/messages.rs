//! What the player reads when the game files' first steps fail: a .nup that is not there, and a game folder that
//! cannot be written to. Each is a player error with its stable code and a sentence that names the file or folder and
//! what to do.

use game_files::checks::check_writable;
use game_files::nup::Nup;
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("messages").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_nup_that_is_not_there() {
    let path = dir("nup").join("Zelda.nup");
    let Err(e) = Nup::open(&path) else { panic!("opened a missing file") };
    assert_eq!(e.code(), "nup_missing");
    let text = e.to_string();
    assert!(text.starts_with(&format!("Could not open your .nup file {}: ", path.display())), "{text}");
    assert!(
        text.ends_with(
            ". Check that the file is still there, reconnect its drive if needed, or choose the file again."
        ),
        "{text}"
    );
}

#[test]
fn a_game_folder_that_takes_files() {
    let game = dir("writable").join("new/Longshot");
    check_writable(&game).unwrap();
    assert!(game.is_dir(), "made");
    assert_eq!(std::fs::read_dir(&game).unwrap().count(), 0, "the probe is gone");
    std::fs::write(game.join(".longshot_write_check"), "left by a stopped build").unwrap();
    check_writable(&game).unwrap();
    assert!(!game.join(".longshot_write_check").exists(), "a probe left behind is replaced, then removed");
}

fn not_writable(game: &Path) -> String {
    let e = check_writable(game).expect_err("written");
    assert_eq!(e.code(), "game_folder_not_writable");
    let text = e.to_string();
    let example = if cfg!(windows) { r"C:\Games\Longshot" } else { "~/Games/Longshot" };
    assert!(text.starts_with(&format!("The game folder {} can't be written to (", game.display())), "{text}");
    assert!(
        text.ends_with(&format!("Choose a game folder of your own, for example {example}, and build again.")),
        "{text}"
    );
    text
}

#[test]
fn a_game_folder_under_a_file() {
    let d = dir("under_file");
    std::fs::write(d.join("file"), "").unwrap();
    let text = not_writable(&d.join("file/Longshot"));
    assert!(!text.contains("Program Files"), "not a protected folder: {text}");
}

#[cfg(unix)]
#[test]
fn a_read_only_game_folder() {
    use std::os::unix::fs::PermissionsExt;
    let game = dir("read_only").join("Longshot");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::set_permissions(&game, std::fs::Permissions::from_mode(0o555)).unwrap();
    // (root writes anyway: nothing to check then)
    let probe = std::fs::write(game.join("x"), "");
    if probe.is_err() {
        let text = not_writable(&game);
        assert!(text.contains("ermission denied"), "the system's reason: {text}");
    }
    std::fs::set_permissions(&game, std::fs::Permissions::from_mode(0o755)).unwrap();
}
