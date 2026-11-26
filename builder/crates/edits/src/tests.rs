//! The edits' planning on synthetic images and placements (nothing of the game's).

use super::*;
use sites::{Hook, Placed, Placement, Point};

const BASE: u64 = 0x8000_0000;
const ADD_X0_1: u32 = 0x9100_0400;
const RET: u32 = 0xd65f_03c0;

fn image(words: &[u32], build_id: &str) -> Image {
    let mut mem = Vec::new();
    for w in words {
        mem.extend_from_slice(&w.to_le_bytes());
    }
    let size = mem.len() as u64;
    let module = image::Module {
        name: "main".into(),
        base: BASE,
        size,
        build_id: build_id.into(),
        segments: vec![image::Segment { name: ".text".into(), addr: BASE, size }],
    };
    Image::new(BASE, mem, vec![module]).unwrap()
}

fn plan(functions: &[(u64, u64)]) -> Plan {
    let mut p = Plan::default();
    for &(entry, n) in functions {
        p.entries.insert(entry);
        p.functions.insert(entry, cfg::plan::Function::from_pcs((0..n).map(|i| entry + 4 * i), BTreeSet::new()));
    }
    p
}

fn site(name: &str, entry: u64, point: Point) -> Placed {
    Placed {
        name: name.into(),
        ident: sites::ident(name),
        entry,
        point,
        hook: Hook::Callback,
        args: Vec::new(),
        holders: if point.pc().is_some() { vec![entry] } else { Vec::new() },
    }
}

fn planned(img: &Image, p: &Plan, sites: Vec<Placed>) -> Planned {
    let placement = Placement { sites, ..Placement::default() };
    plan_edits(&Inputs { image: img, plan: p, placement: Some(&placement), entry_hooks: &[] }).unwrap()
}

#[test]
fn entry_hooks_are_read_from_the_host_s_lines() {
    let text = "#include \"hos.h\"\nENTRY_HOOK_SYM(memcpy) {\n  ENTRY_HOOK_SYM(nested)\nENTRY_HOOK_SYM_FROM(memset, 8) {\n\
                #define ENTRY_HOOK_SYM(sym) int entry_sym_##sym(Ctx *c)\nENTRY_HOOK_SYM_FROM(bad, x)\n";
    let hooks = entry_hooks::parse(text);
    assert_eq!(
        hooks,
        vec![
            entry_hooks::EntryHook { symbol: "memcpy".into(), least: 0 },
            entry_hooks::EntryHook { symbol: "memset".into(), least: 8 },
        ]
    );
}

#[test]
fn an_entry_hook_needs_the_sdk_s_function() {
    let img = image(&[ADD_X0_1, RET], "");
    let p = plan(&[(BASE, 2)]);
    let (found, notes) = entry_hooks::place(&[entry_hooks::EntryHook { symbol: "memcpy".into(), least: 0 }], &img, &p);
    assert!(found.is_empty());
    assert_eq!(notes, vec!["memcpy: no module defines it".to_string()]);
}

#[test]
fn each_site_lands_in_its_function_alone_and_its_key_changes_alone() {
    let img = image(&[ADD_X0_1, RET, ADD_X0_1, RET], "");
    let p = plan(&[(BASE, 2), (BASE + 8, 2)]);
    let a = site("t.a", BASE, Point::After { pc: BASE });
    let b = site("t.b", BASE + 8, Point::Before { pc: BASE + 8 });
    let one = planned(&img, &p, vec![a.clone(), b.clone()]).edits;
    let again = planned(&img, &p, vec![a.clone(), b.clone()]).edits;
    assert_eq!(one[&BASE].key(), again[&BASE].key(), "the same edits, the same key");
    assert_eq!(one.keys().copied().collect::<Vec<_>>(), vec![BASE, BASE + 8]);
    // (site b moved to an after site: only its function's edits and key change)
    let b2 = site("t.b", BASE + 8, Point::After { pc: BASE + 8 });
    let two = planned(&img, &p, vec![a, b2]).edits;
    assert_eq!(one[&BASE], two[&BASE]);
    assert_eq!(one[&BASE].key(), two[&BASE].key());
    assert_ne!(one[&(BASE + 8)].key(), two[&(BASE + 8)].key());
}

#[test]
fn several_sites_at_one_place_keep_the_table_s_order() {
    let img = image(&[ADD_X0_1, RET], "");
    let p = plan(&[(BASE, 2)]);
    let e = planned(
        &img,
        &p,
        vec![site("t.first", BASE, Point::After { pc: BASE }), site("t.second", BASE, Point::After { pc: BASE })],
    )
    .edits;
    let text = format!("{:?}", e[&BASE].after[&BASE]);
    assert!(text.find("t_first").unwrap() < text.find("t_second").unwrap(), "{text}");
}

#[test]
fn two_replacements_of_one_instruction_stop_the_plan() {
    let img = image(&[ADD_X0_1, RET], "");
    let p = plan(&[(BASE, 2)]);
    let r = |n| site(n, BASE, Point::Replace { pc: BASE, callee: None });
    let placement = Placement { sites: vec![r("t.one"), r("t.two")], ..Placement::default() };
    let err = plan_edits(&Inputs { image: &img, plan: &p, placement: Some(&placement), entry_hooks: &[] }).err();
    assert!(matches!(err, Some(EditError::Clash { .. })), "{err:?}");
}

#[test]
fn an_edit_on_a_plt_stub_stops_the_plan() {
    // (a stub is lifted as its jump: a site's code there would never run)
    let img = image(&[ADD_X0_1, RET], "");
    let mut p = plan(&[(BASE, 2)]);
    p.plt.insert(BASE, cfg::plan::Plt { target: BASE + 0x100, slot: BASE + 0x200 });
    let placement =
        Placement { sites: vec![site("t.stub", BASE, Point::Entry { can_return: false })], ..Placement::default() };
    let err = plan_edits(&Inputs { image: &img, plan: &p, placement: Some(&placement), entry_hooks: &[] }).err();
    assert!(matches!(&err, Some(EditError::Clash { why, .. }) if why.contains("PLT stub")), "{err:?}");
}

#[test]
fn return_sites_share_one_wrapper_in_order() {
    let img = image(&[ADD_X0_1, RET], "");
    let p = plan(&[(BASE, 2)]);
    let e = planned(&img, &p, vec![site("t.one", BASE, Point::Return), site("t.two", BASE, Point::Return)]).edits;
    let Some(w @ Wrap::Return(after)) = &e[&BASE].wrap else { panic!("{e:?}") };
    assert_eq!(after.len(), 2);
    let f = w.wrapper(BASE);
    f.verify().unwrap();
    assert!(f.wrapper);
    assert_eq!(f.name(), format!("f_{BASE:08x}"));
}

#[test]
fn the_patches_are_for_their_build_alone() {
    let img = image(&[ADD_X0_1, RET], "0123456789012345678901234567890123456789");
    let p = plan(&[(BASE, 2)]);
    let out = planned(&img, &p, Vec::new());
    assert!(out.edits.is_empty());
    assert!(out.notes.iter().any(|n| n.starts_with("patches: none for build")), "{:?}", out.notes);
    // (the supported build's id, but not its code: a patch not found stops the plan)
    let img = image(&[ADD_X0_1, RET], patches::BUILD);
    let placement = Placement::default();
    let err = plan_edits(&Inputs { image: &img, plan: &p, placement: Some(&placement), entry_hooks: &[] }).err();
    assert!(matches!(err, Some(EditError::Patch { .. })), "{err:?}");
}

#[test]
fn every_patch_shape_is_what_the_decoder_prints_for_its_instruction() {
    // (Shape::Asm compares the decoder's text: each shape's instruction, assembled here, must print as it does, so a
    // change to the printer fails here rather than finding no patch in the game)
    let words: &[(&str, u32)] = &[
        ("mov x8, #2560", 0xd281_4008),
        ("movk x8, #1440, lsl #32", 0xf2c0_b408),
        ("stur x8, [x21, #20]", 0xf801_42a8),
        ("add x21, x0, #724", 0x910b_5015),
        ("orr x20, xzr, x1", 0xaa01_03f4),
        ("ldp w24, w25, [x0, #0]", 0x2940_6418),
        ("ldrb w8, [x19, #1565]", 0x3958_7668),
        ("blr x8", 0xd63f_0100),
        ("mov w8, #1156579328", 0x52a8_9e08),
        ("str w8, [x19, #3668]", 0xb90e_5668),
        ("mov w8, #1149698048", 0x52a8_90e8),
    ];
    let shapes: Vec<&str> = patches::patches()
        .iter()
        .filter_map(|p| p.find.as_ref())
        .flat_map(|f| f.shapes.iter())
        .filter_map(|s| match s {
            patches::Shape::Asm(t) => Some(*t),
            patches::Shape::Bl(_) => None,
        })
        .collect();
    assert!(!shapes.is_empty());
    for s in shapes {
        let &(_, w) = words.iter().find(|(t, _)| *t == s).unwrap_or_else(|| panic!("no word here for the shape {s:?}"));
        let printed = a64::decode(w).expect("an instruction").to_string();
        assert_eq!(printed, s, "the decoder prints {w:#010x} as {printed:?}");
        assert!(patches::matches(patches::Shape::Asm(s), w, 0, &|t| t), "{s}");
    }
}
