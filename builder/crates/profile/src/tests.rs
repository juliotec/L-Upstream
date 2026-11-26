//! The profile's text, its merge and the raw counts' reading, on made-up counts.

use super::*;

fn sample() -> Profile {
    let mut p = Profile { build: "abcd".into(), ..Profile::default() };
    let mut f = Function { hash: 0x1234, entries: 10, ..Function::default() };
    f.blocks.insert(0x8000_0000, 10);
    f.blocks.insert(0x8000_0010, 7);
    f.taken.insert(0x8000_0000, 3);
    p.functions.insert(0x8000_0000, f);
    p
}

#[test]
fn the_text_reads_back_as_it_was_written() {
    let p = sample();
    assert_eq!(Profile::parse(&p.to_text(), "t").unwrap(), p);
}

#[test]
fn counts_add_up_for_the_same_code_and_a_changed_function_takes_the_newer() {
    let mut a = sample();
    a.merge(&sample());
    let f = &a.functions[&0x8000_0000];
    assert_eq!((f.entries, f.blocks[&0x8000_0010], f.taken[&0x8000_0000]), (20, 14, 6));
    let mut changed = sample();
    changed.functions.get_mut(&0x8000_0000).unwrap().hash = 0x9999;
    a.merge(&changed);
    assert_eq!(a.functions[&0x8000_0000].entries, 10);
    assert_eq!(a.function(0x8000_0000, 0x1234), None, "the old code's counts are gone");
}

#[test]
fn never_ran_only_on_the_build_it_was_recorded_on() {
    let p = sample();
    assert!(!p.never_ran("abcd", 0x8000_0000, 0x1234));
    assert!(p.never_ran("abcd", 0x8000_1000, 0x55));
    assert!(!p.never_ran("other", 0x8000_1000, 0x55), "another build's profile says nothing of it");
}

#[test]
fn a_function_entered_in_its_middle_ran() {
    // (no entry count, but a block that ran: jumped into, not called)
    let mut p = sample();
    let mut f = Function { hash: 0x77, ..Function::default() };
    f.blocks.insert(0x8000_2010, 5);
    p.functions.insert(0x8000_2000, f);
    assert!(!p.never_ran("abcd", 0x8000_2000, 0x77));
    p.functions.insert(0x8000_3000, Function { hash: 0x88, ..Function::default() });
    assert!(p.never_ran("abcd", 0x8000_3000, 0x88), "a record with no counts at all did not run");
}

#[test]
fn raw_counts_become_a_profile_by_the_map() {
    let map = Map::parse(
        "build abcd\nu lifted_000\nf 80000000 0000000000001234\nb 80000000 0\nb 80000010 1\nt 80000000 2\n\
         f 80001000 0000000000000055\nb 80001000 3\n",
        "map",
    )
    .unwrap();
    assert_eq!(Map::parse(&map.to_text(), "again").unwrap(), map);
    let p = map.read_raw("unit lifted_000\n0 10\n1 7\n2 3\n", "raw").unwrap();
    assert_eq!(p, sample(), "the function that never ran has no record");
}

#[test]
fn the_hash_follows_every_word() {
    assert_ne!(code_hash([1, 2]), code_hash([2, 1]));
    assert_eq!(code_hash([0xd65f_03c0]), code_hash([0xd65f_03c0]));
}

#[test]
fn a_profile_reads_from_its_xz_file_and_a_changed_function_takes_none_of_its_counts() {
    let p = sample();
    let dir = std::env::temp_dir().join(format!("longshot-profile-xz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (plain, packed) = (dir.join("guest.profile"), dir.join("guest.profile.xz"));
    std::fs::write(&plain, p.to_text()).unwrap();
    let mut xz = Vec::new();
    lzma_rs::xz_compress(&mut p.to_text().as_bytes(), &mut xz).unwrap();
    std::fs::write(&packed, xz).unwrap();
    assert_eq!(Profile::load(&packed).unwrap(), p);
    assert_eq!(Profile::load(&plain).unwrap(), p);
    std::fs::write(&packed, b"\xFD7zXZ\x00 damaged").unwrap();
    assert!(Profile::load(&packed).is_err());
    let _ = std::fs::remove_dir_all(&dir);
    // (the function's words changed since the recording: no counts, not hot, not cold)
    let read = p;
    assert!(read.function(0x8000_0000, 0x4321).is_none());
    assert!(!read.never_ran("abcd", 0x8000_0000, 0x4321));
}

#[test]
fn a_branch_taken_more_often_than_its_block_ran_is_clamped_to_its_block() {
    // (in a profile's text, and in a run's raw counts: the counters race, so a taken count can pass its block's)
    let text = "longshot-guest-profile 1 build abcd\nf 80000000 0000000000001234 10\nb 80000000 10\nb 80000010 7\n\
                t 80000000 40\nt 80000010 7\n";
    let p = Profile::parse(text, "t").unwrap();
    let f = &p.functions[&0x8000_0000];
    assert_eq!((f.taken[&0x8000_0000], f.taken[&0x8000_0010]), (10, 7));
    // (a branch of a block with no count: never taken)
    let p = Profile::parse(
        "longshot-guest-profile 1 build -\nf 80000000 0000000000001234 1\nb 80000000 1\nt 80000020 5\n",
        "t",
    )
    .unwrap();
    assert_eq!(p.functions[&0x8000_0000].taken[&0x8000_0020], 0);
    let map = Map::parse(
        "build abcd\nu lifted_000\nf 80000000 0000000000001234\nb 80000000 0\nb 80000010 1\nt 80000000 2\n",
        "map",
    )
    .unwrap();
    let p = map.read_raw("unit lifted_000\n0 10\n1 7\n2 25\n", "raw").unwrap();
    assert_eq!(p.functions[&0x8000_0000].taken[&0x8000_0000], 10);
}
