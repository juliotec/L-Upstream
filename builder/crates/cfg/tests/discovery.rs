//! Discovery on a made-up image (the instructions assembled by llvm-mc; nothing from the game): a function with a
//! bounded jump table, a call to a function named `setjmp`, and a PLT stub through a GOT slot.

use cfg::{Discovery, Inputs};
use image::{Image, Module, Segment};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const BASE: u64 = 0x1000;

/// The program, at BASE:
/// ```text
/// 1000 cmp w0, #2              ; the bound: x0 indexes 3 entries
/// 1004 b.hi 1030
/// 1008 adrp x1, 1000
/// 100c add x1, x1, #0x100     ; the table at 1100
/// 1010 ldrb w2, [x1, w0, uxtw]
/// 1014 adr x3, 1020           ; the targets' base
/// 1018 add x3, x3, x2, lsl #2
/// 101c br x3
/// 1020 ret                    ; case 0
/// 1024 b 1030                 ; case 1
/// 1028 bl 1040                ; case 2: calls setjmp
/// 102c ret
/// 1030 ret                    ; the default
/// 1040 ret                    ; setjmp
/// 1050 adrp x16, 1000         ; a PLT stub: x17 from the GOT slot at 1108 (which holds 1040)
/// 1054 ldr x17, [x16, #0x108]
/// 1058 add x16, x16, #0x108
/// 105c br x17
/// ```
fn program() -> Image {
    program_with(0, 0)
}

/// The program with the word at `pc` replaced by `word` (pc 0: none replaced).
fn program_with(pc: u64, word: u32) -> Image {
    program_with_words(&[(pc, word)])
}

/// The program with each (pc, word) of `changes` in place of its word.
fn program_with_words(changes: &[(u64, u32)]) -> Image {
    let mut words: [(u64, u32); 18] = [
        (0x1000, 0x7100_081f),
        (0x1004, 0x5400_0168),
        (0x1008, 0x9000_0001),
        (0x100c, 0x9104_0021),
        (0x1010, 0x3860_4822),
        (0x1014, 0x1000_0063),
        (0x1018, 0x8b02_0863),
        (0x101c, 0xd61f_0060),
        (0x1020, 0xd65f_03c0),
        (0x1024, 0x1400_0003),
        (0x1028, 0x9400_0006),
        (0x102c, 0xd65f_03c0),
        (0x1030, 0xd65f_03c0),
        (0x1040, 0xd65f_03c0),
        (0x1050, 0x9000_0010),
        (0x1054, 0xf940_8611),
        (0x1058, 0x9104_2210),
        (0x105c, 0xd61f_0220),
    ];
    for &(pc, word) in changes {
        if let Some(slot) = words.iter_mut().find(|(at, _)| *at == pc) {
            slot.1 = word;
        }
    }
    let mut mem = vec![0u8; 0x200];
    for (pc, w) in words {
        let at = (pc - BASE) as usize;
        mem[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }
    mem[0x100..0x103].copy_from_slice(&[0, 1, 2]); // the table's entries (words from 1020)
    mem[0x108..0x110].copy_from_slice(&0x1040u64.to_le_bytes()); // the GOT slot
    let module = Module {
        name: "main".into(),
        base: BASE,
        size: 0x200,
        build_id: String::new(),
        segments: vec![
            Segment { name: ".text".into(), addr: 0x1000, size: 0x100 },
            Segment { name: ".rodata".into(), addr: 0x1100, size: 0x100 },
        ],
    };
    Image::new(BASE, mem, vec![module]).unwrap()
}

fn inputs() -> Inputs {
    Inputs {
        entries: BTreeSet::from([0x1000, 0x1040, 0x1050]),
        hooks: BTreeMap::new(),
        eh_starts: vec![0x1000, 0x1040, 0x1050],
        names: HashMap::from([(0x1040, "setjmp".to_string())]),
    }
}

#[test]
fn finds_the_table_the_stub_and_the_setjmp_owner() {
    let img = program();
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());

    assert_eq!(p.jump_tables[&0x101c], vec![0x1020, 0x1024, 0x1028]);
    let f = &p.functions[&0x1000];
    assert_eq!(f.runs, vec![0x1000..0x1034]);
    assert_eq!(f.labels, BTreeSet::from([0x1020, 0x1024, 0x1028, 0x1030]));
    assert_eq!(p.functions[&0x1040].runs, vec![0x1040..0x1044]);

    assert_eq!(p.plt[&0x1050], cfg::plan::Plt { target: 0x1040, slot: 0x1108 });

    // the call of setjmp makes its return address an entry, walked as a function of its own
    assert_eq!(p.setjmp_owners[&0x1000], vec![0x102c]);
    assert!(p.entries.contains(&0x102c));
    assert_eq!(p.functions[&0x102c].runs, vec![0x102c..0x1030]);
    assert_eq!(d.stats.counts.get("jt_bounded"), Some(&1));
}

#[test]
fn an_entry_inside_cuts_the_walk() {
    // 1030 an entry of its own: the function stops before it, and branches to it are no labels
    let img = program();
    let mut inp = inputs();
    inp.entries.insert(0x1030);
    let mut d = Discovery::new(&img, inp);
    d.run();
    let p = d.plan("image.bin".as_ref());
    let f = &p.functions[&0x1000];
    assert_eq!(f.runs, vec![0x1000..0x1030]);
    assert!(!f.labels.contains(&0x1030));
    assert_eq!(p.functions[&0x1030].runs, vec![0x1030..0x1034]);
}

#[test]
fn an_unbounded_table_reads_until_a_target_leaves_the_function() {
    // Without the compare (a NOP in its place) there is no bound: the table is read until an entry's target leaves
    // the function. Its bytes are 0, 1, 2, then five 0s (1020 five times more: kept, repeats and all, as the kit keeps
    // them within one table), then the GOT slot's 0x40 (1120: past the code), where it stops.
    let img = program_with(0x1000, 0xd503_201f);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    let mut expected = vec![0x1020, 0x1024, 0x1028];
    expected.extend([0x1020; 5]);
    assert_eq!(p.jump_tables[&0x101c], expected);
    assert_eq!(d.stats.counts.get("jt_unbounded"), Some(&1));
}

#[test]
fn an_index_masked_to_two_bits_bounds_its_table_at_four() {
    // `and w0, w0, #3` in the compare's place: no compare bounds the index, but what made it does, so four entries are
    // read (0, 1, 2 and the first of the five 0s), not until a target leaves the function
    let img = program_with(0x1000, 0x1200_0400);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    assert_eq!(p.jump_tables[&0x101c], vec![0x1020, 0x1024, 0x1028, 0x1020]);
    assert_eq!(d.stats.counts.get("jt_bounded"), Some(&1));
}

/// The table read with no bound: until an entry leaves the code (the GOT slot's first byte, 0x40, at the eighth)
const UNBOUNDED: [u64; 8] = [0x1020, 0x1024, 0x1028, 0x1020, 0x1020, 0x1020, 0x1020, 0x1020];

#[test]
fn a_mask_the_table_s_block_can_be_reached_without_bounds_nothing() {
    // `cbnz w1, 1008; and w0, w0, #1` in the compare's place: the load's block is reached from the CBNZ too, with w0
    // unmasked, so the AND bounds nothing (not 2 entries)
    let img = program_with_words(&[(0x1000, 0x3500_0041), (0x1004, 0x1200_0000)]);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    assert_eq!(p.jump_tables[&0x101c], UNBOUNDED);
}

#[test]
fn a_mask_every_way_into_the_table_s_block_comes_through_bounds_it() {
    // `and w0, w0, #3; cbz w1, 1008`, the load's block reached two ways (the CBZ and its fall-through), both from the
    // AND: four entries
    let img = program_with_words(&[
        (0x1000, 0x1200_0400),
        (0x1004, 0x3400_0021),
        (0x1008, 0x9000_0001),
        (0x100c, 0x9104_0021),
        (0x1010, 0x3860_4822),
        (0x1014, 0x1000_0063),
        (0x1018, 0x8b02_0863),
    ]);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    assert_eq!(p.jump_tables[&0x101c], vec![0x1020, 0x1024, 0x1028, 0x1020]);
}

#[test]
fn a_mask_after_the_table_s_load_bounds_nothing() {
    // `ldrb w2, [x1, w0, uxtw]; adr x3, 1020; and w0, w5, #3; add x3, x3, x2, lsl #2; nop; br x3`: w0 is masked after
    // the load used it, so the AND says nothing of the index (not 4 entries)
    let img = program_with_words(&[
        (0x1000, 0x9000_0001),
        (0x1004, 0x9104_0021),
        (0x1008, 0x3860_4822),
        (0x100c, 0x1000_00a3),
        (0x1010, 0x1200_04a0),
        (0x1014, 0x8b02_0863),
        (0x1018, 0xd503_201f),
    ]);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    assert_eq!(p.jump_tables[&0x101c], UNBOUNDED);
}

#[test]
fn a_compare_of_another_register_does_not_cut_a_masked_table_short() {
    // `cmp w5, #1; b.hi 1030` (a range check of w5: 2) then `and w0, w0, #3; adr x1, 1100`: the index is masked to
    // four entries, the window's last compare says two of another register; the larger is read (not cases 2 and 3 left
    // to the switch's default, an indirect jump to no function)
    let img = program_with_words(&[(0x1000, 0x7100_04bf), (0x1008, 0x1200_0400), (0x100c, 0x1000_07a1)]);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    assert_eq!(p.jump_tables[&0x101c], vec![0x1020, 0x1024, 0x1028, 0x1020]);
    assert_eq!(d.stats.counts.get("jt_bounded"), Some(&1));
}

#[test]
fn a_compare_s_shifted_immediate_bounds_by_its_value() {
    // `cmp w0, #1, lsl #12`: 4096 entries, past what a table is taken to hold, so no bound (read until an entry leaves
    // the function), not two
    let img = program_with(0x1000, 0x7140_041f);
    let mut d = Discovery::new(&img, inputs());
    d.run();
    let p = d.plan("image.bin".as_ref());
    assert_eq!(p.jump_tables[&0x101c], UNBOUNDED);
}
