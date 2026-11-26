//! `exefs_patches.txt`'s exact lines: the game reads them (game/mods.c exefs_status) by the prefix
//! `<mod>\t<file>\t<8 hex digits of FNV-1a>\t`, then the kind's word (code and data built in, empty built in with no
//! write, other another version's, anything else unusable with the note after a tab), so a change of their shape here
//! leaves every mod "waiting for the next build". tests/test_mods_sizes.py gives the game the list made here.

use game_files::patches::{Kind, PatchFile, fnv32, list_text};

fn file(mod_name: &str, file: &str, data: &[u8], kind: Kind, note: &str, on: bool) -> PatchFile {
    PatchFile {
        mod_name: mod_name.into(),
        file: file.into(),
        hash: format!("{:08x}", fnv32(data)),
        kind,
        note: note.into(),
        on,
        writes: vec![(0x10, 0xAA)],
    }
}

#[test]
fn the_lines_the_game_reads() {
    assert_eq!(fnv32(b""), 0x811C_9DC5);
    assert_eq!(fnv32(b"@nsobid-00\n"), 0xd2bb_95a7, "FNV-1a, 32 bits, as game/mods.c fnv32_file");
    let files = [
        file("01-first", "built.pchtxt", b"@nsobid-00\n", Kind::Code, "", true),
        file("01-first", "data.ips", b"PATCH", Kind::Data, "", true),
        file("02-second", "old.ips", b"x", Kind::Other, "", true),
        file("02-second", "none.pchtxt", b"y", Kind::Empty, "", true),
        file("03-bad", "big.ips", b"z", Kind::Unsupported, "a\tnote with a tab", true),
        file("03-bad", "hook.pchtxt", b"w", Kind::Refused, "stops the hook site Player_Update", true),
        file("04-off", "x.ips", b"v", Kind::Off, "", false),
    ];
    let h = |d: &[u8]| format!("{:08x}", fnv32(d));
    assert_eq!(
        list_text(&files),
        [
            "# The mods' ExeFS patch files this game was built with (longshot-builder): mod, file, FNV-1a of its bytes, \
             kind, note"
                .to_string(),
            "01-first\tbuilt.pchtxt\td2bb95a7\tcode\t".to_string(),
            format!("01-first\tdata.ips\t{}\tdata\t", h(b"PATCH")),
            format!("02-second\told.ips\t{}\tother\t", h(b"x")),
            format!("02-second\tnone.pchtxt\t{}\tempty\t", h(b"y")),
            format!("03-bad\tbig.ips\t{}\tunsupported\ta note with a tab", h(b"z")),
            format!("03-bad\thook.pchtxt\t{}\trefused\tstops the hook site Player_Update", h(b"w")),
        ]
        .join("\n")
            + "\n",
        "an off mod's file is not listed: the game says it waits for the next build"
    );
    assert_eq!(list_text(&[]).lines().count(), 1, "the header alone");
}
