//! The host's a64_config.h ([`lift::whole::files::config_h`]): stores always mark their pages, whatever the lift's
//! options (the switch that built the game with Windows' write watch instead is gone:
//! tests/test_switches_removed.py), and every option's own line follows what it was given.

use lift::whole::files::config_h;

#[test]
fn stores_always_mark_their_pages_and_each_choice_is_written() {
    for (polls, mark_bitmap) in [(true, true), (false, false), (true, false)] {
        let o = llvm::Options { polls, mark_bitmap, ..llvm::Options::default() };
        let h = config_h(&o);
        assert!(h.contains("\n#define A64_WRITE_TRACK 1 "), "{h}");
        assert!(h.contains(&format!("\n#define A64_POLLING {} ", u8::from(polls))), "{h}");
        assert!(h.contains(&format!("\n#define A64_MARK_BITMAP {} ", u8::from(mark_bitmap))), "{h}");
        assert!(!h.contains("WRITE_WATCH"), "{h}");
    }
}
