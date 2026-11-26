//! The shader stamp over a small gpu/vk tree: it moves when a stamped file's code changes, stays when only a stamped
//! file's comments or line ends change, and stays when a file it does not cover (vk_menu.cpp) changes.

use build_compile::generated::shader_stamp::{files, stamp};
use std::path::{Path, PathBuf};

fn tree(name: &str) -> PathBuf {
    let vk = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shader_stamp").join(name).join("gpu/vk");
    let _ = std::fs::remove_dir_all(&vk);
    for (rel, text) in [
        ("eden/recompiler.cpp", "int translate(int x) {\n    return x + 1;\n}\n"),
        ("sirit/emit.h", "#pragma once\nint emit();\n"),
        ("compat/notes.txt", "a  b\n"),
        ("vk_shader.cpp", "#include \"vk_state.h\"\nint shader;\n"),
        ("vk_bank.cpp", "int bank;\n"),
        ("vk_state.h", "struct State { int a; };\n"),
        ("vk_menu.cpp", "int menu = 1;\n"),
    ] {
        let p = vk.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    vk
}

fn edit(vk: &Path, rel: &str, text: &str) -> String {
    std::fs::write(vk.join(rel), text).unwrap();
    stamp(vk).unwrap()
}

#[test]
fn the_stamp_covers_the_translator_and_nothing_else() {
    let vk = tree("cover");
    let names: Vec<String> =
        files(&vk).iter().map(|f| f.strip_prefix(&vk).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(
        names,
        ["compat/notes.txt", "eden/recompiler.cpp", "sirit/emit.h", "vk_shader.cpp", "vk_bank.cpp", "vk_state.h"]
    );
    let first = stamp(&vk).unwrap();
    assert_eq!(first.len(), 16);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));

    assert_eq!(edit(&vk, "vk_menu.cpp", "int menu = 2;\n"), first, "vk_menu.cpp is not stamped");
    let comment = "// the translator\nint translate(int x) {\n  return x + 1; /* one more */\n}\n";
    assert_eq!(edit(&vk, "eden/recompiler.cpp", comment), first, "comments and indentation are not code");
    let crlf = "int translate(int x) {\r\n    return x + 1;\r\n}\r\n";
    assert_eq!(edit(&vk, "eden/recompiler.cpp", crlf), first, "Windows line ends count as Unix ones");
    assert_eq!(edit(&vk, "compat/notes.txt", "a  b\r\n"), first, "in a file stamped byte for byte too");

    let changed = edit(&vk, "eden/recompiler.cpp", "int translate(int x) {\n    return x + 2;\n}\n");
    assert_ne!(changed, first, "a stamped file's code moves it");
    let notes = edit(&vk, "compat/notes.txt", "a b\n");
    assert_ne!(notes, changed, "a file that is not C, byte for byte");
    assert_ne!(edit(&vk, "vk_state.h", "struct State { int a, b; };\n"), notes);
}
