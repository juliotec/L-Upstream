//! The resolver, the anchors, the fingerprints and the placement, on made-up code and a made-up lift (tests/test_hooks.py's
//! Resolver, Placement and Forms, tests/test_hook_anchors.py's Anchors, Fingerprints and WithoutTheLift): real
//! instruction words in a made-up image, the functions a plan.

use super::*;
use crate::table::tests::{SIG_F, SITE, VSLOT, one, read, rule, site, table};
use cfg::plan::Function as PlanFunction;
use image::{Module, Segment};
use std::collections::{BTreeMap, BTreeSet};

const F: u64 = 0x8000_1000;
const G: u64 = 0x8000_2000;
const H: u64 = 0x8000_3000;
const HI: u64 = 0x8000_4000;
const NOP: u32 = 0xd503_201f;
const RET: u32 = 0xd65f_03c0;
const BLR8: u32 = 0xd63f_0100;
const LDR8_OBJ: u32 = 0xf940_0008; // ldr x8, [x0]
const LDR8_SLOT: u32 = 0xf940_c908; // ldr x8, [x8, #0x190]

fn rel(pc: u64, t: u64) -> u32 {
    (t.wrapping_sub(pc) as i64 / 4) as u32
}
fn bl(pc: u64, t: u64) -> u32 {
    0x9400_0000 | (rel(pc, t) & 0x03ff_ffff)
}
fn b(pc: u64, t: u64) -> u32 {
    0x1400_0000 | (rel(pc, t) & 0x03ff_ffff)
}
fn bcond(pc: u64, t: u64) -> u32 {
    0x5400_0001 | ((rel(pc, t) & 0x7ffff) << 5)
}
fn cbz(pc: u64, t: u64) -> u32 {
    0x3400_0009 | ((rel(pc, t) & 0x7ffff) << 5)
}
fn br(reg: u32) -> u32 {
    0xd61f_0000 | (reg << 5)
}
fn adrp(pc: u64, addr: u64, rd: u32) -> u32 {
    let imm = ((addr & !0xfff) as i64 - (pc & !0xfff) as i64) >> 12;
    0x9000_0000 | (((imm & 3) as u32) << 29) | ((((imm >> 2) & 0x7ffff) as u32) << 5) | rd
}
fn add_lo(addr: u64, rd: u32, rn: u32) -> u32 {
    0x9100_0000 | (((addr & 0xfff) as u32) << 10) | (rn << 5) | rd
}

/// A made-up lift: the code from F to HI (zeros where no function is), its functions, PLT stubs and entries.
#[derive(Default)]
struct Fx {
    words: BTreeMap<u64, u32>,
    plan: Plan,
}

impl Fx {
    fn new() -> Fx {
        Fx::default()
    }

    /// A function of these words at `entry`, its labels the branch targets inside it.
    fn func(mut self, entry: u64, words: &[u32]) -> Fx {
        let pcs: Vec<u64> = (0..words.len() as u64).map(|i| entry + 4 * i).collect();
        for (&pc, &w) in pcs.iter().zip(words) {
            self.words.insert(pc, w);
        }
        let labels = pcs
            .iter()
            .zip(words)
            .filter_map(|(&pc, &w)| match InsnKind::of(w, pc) {
                InsnKind::B(t) | InsnKind::BCond(t) | InsnKind::Cbz(t) | InsnKind::Tbz(t) => Some(t),
                _ => None,
            })
            .filter(|t| pcs.contains(t))
            .collect();
        self.plan.functions.insert(entry, PlanFunction::from_pcs(pcs, labels));
        self.plan.entries.insert(entry);
        self
    }

    /// A function at `entry` whose body is these instructions (code another function's words already are).
    fn body(mut self, entry: u64, pcs: &[u64]) -> Fx {
        self.plan.functions.insert(entry, PlanFunction::from_pcs(pcs.iter().copied(), BTreeSet::new()));
        self.plan.entries.insert(entry);
        self
    }

    fn word(mut self, pc: u64, w: u32) -> Fx {
        self.words.insert(pc, w);
        self
    }

    fn plt(mut self, stub: u64, target: u64) -> Fx {
        self.plan.plt.insert(stub, cfg::plan::Plt { target, slot: 0 });
        self.plan.entries.insert(stub);
        self
    }

    fn entry(mut self, e: u64) -> Fx {
        self.plan.entries.insert(e);
        self
    }

    fn setjmp(mut self, e: u64) -> Fx {
        self.plan.setjmp_owners.insert(e, Vec::new());
        self
    }

    fn label(mut self, entry: u64, pc: u64) -> Fx {
        if let Some(f) = self.plan.functions.get_mut(&entry) {
            f.labels.insert(pc);
        }
        self
    }

    fn code(&self) -> Vec<u32> {
        (0..(HI - F) / 4).map(|i| self.words.get(&(F + 4 * i)).copied().unwrap_or(0)).collect()
    }

    fn image(&self) -> Image {
        let mem: Vec<u8> = self.code().iter().flat_map(|w| w.to_le_bytes()).collect();
        let module = Module {
            name: "main".into(),
            base: F,
            size: HI - F,
            build_id: crate::table::tests::BUILD.into(),
            segments: vec![Segment { name: ".text".into(), addr: F, size: HI - F }],
        };
        Image::new(F, mem, vec![module]).unwrap()
    }
}

fn space<'a>(fx: &Fx, data: &'a [u8], exports: &[(&str, u64)]) -> Space<'a> {
    Space {
        base: F,
        code: fx.code(),
        image: data,
        image_base: F,
        exports: exports.iter().map(|(n, a)| ((*n).to_string(), *a)).collect(),
    }
}

fn sites(texts: &[&str]) -> Vec<Site> {
    read(&table(texts), None).unwrap()
}

/// resolve() then locate() of these sites over the made-up lift.
fn locate_all(texts: &[&str], fx: &Fx, sp: &Space<'_>) -> Result<(Vec<Site>, Found), ResolveError> {
    let s = sites(texts);
    let image = fx.image();
    let lift = Lift::new(&image, &fx.plan);
    let mut found = resolve(&s, sp)?;
    locate(&s, &lift, &mut found, sp)?;
    Ok((s, found))
}

fn found_at(pairs: &[(&str, u64)]) -> Found {
    pairs.iter().map(|(n, a)| ((*n).to_string(), Location { addr: *a, ..Location::default() })).collect()
}

fn placed(texts: &[&str], fx: &Fx, found: &Found) -> Result<Placement, ResolveError> {
    let image = fx.image();
    place(&sites(texts), &Lift::new(&image, &fx.plan), found, &HostList::default())
}

fn straight(entry: u64, n: usize) -> Vec<u32> {
    let mut w = vec![NOP; n - 1];
    w.push(RET);
    let _ = entry;
    w
}

// ---- the resolver, without the lift

#[test]
fn a_signature_resolves_exactly_once() {
    let fx = Fx::new().word(F, NOP).word(F + 4, 0xd100_43ff).word(F + 8, 0x9100_43fd).word(F + 12, 0xf940_0c08);
    let s = sites(&[SITE]);
    let found = resolve(&s, &space(&fx, &[], &[])).unwrap();
    assert_eq!(found["test.step"].addr, F + 4);
}

#[test]
fn not_found_or_twice_stops_naming_the_site() {
    let s = sites(&[SITE]);
    let twice = Fx::new()
        .word(F, 0xd100_43ff)
        .word(F + 4, 0x9100_43fd)
        .word(F + 8, 0xf940_0c08)
        .word(F + 12, 0xd100_43ff)
        .word(F + 16, 0x9100_43fd)
        .word(F + 20, 0xf940_0c08);
    let e = resolve(&s, &space(&twice, &[], &[])).unwrap_err().to_string();
    assert!(e.contains("test.step: found 2 times"), "{e}");
    let none = Fx::new().word(F, 0xd100_43ff).word(F + 4, 0x9100_43fd);
    let e = resolve(&s, &space(&none, &[], &[])).unwrap_err().to_string();
    assert!(e.contains("test.step: not found"), "{e}");
    let mut many = Fx::new();
    for i in 0..20 {
        many = many.word(F + 4 * i, 0x1122_3344);
    }
    let e = resolve(&sites(&[&site(&[("signature", Some("\"11223344\""))])]), &space(&many, &[], &[]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("at least 8 times"), "{e}");
}

#[test]
fn guards_are_checked_where_the_signature_puts_the_function() {
    let fx = Fx::new()
        .word(F, NOP)
        .word(F + 4, 0xd100_43ff)
        .word(F + 8, 0x9100_43fd)
        .word(F + 12, 0xf940_0c08)
        .word(F + 16, RET)
        .word(F + 20, 0xb942_1279);
    let sp = space(&fx, &[], &[]);
    let guarded = |off: &str| site(&[("guards", Some(&format!("[{{ offset = {off}, signature = \"b9421279\" }}]")))]);
    assert_eq!(resolve(&sites(&[&guarded("0x10")]), &sp).unwrap()["test.step"].addr, F + 4);
    let e = resolve(&sites(&[&guarded("0xc")]), &sp).unwrap_err().to_string();
    assert!(e.contains("its guard at +0xc of the function differs"), "{e}");
    for bad in ["\"x\"", "[{ offset = 0x14 }]", "[{ offset = 6, signature = \"00000000\" }]"] {
        assert!(read(&table(&[&site(&[("guards", Some(bad))])]), None).is_err(), "{bad}");
    }
}

#[test]
fn a_signature_found_nowhere_names_its_nearest_misses() {
    let fx = Fx::new().word(F, 0xd100_43ff).word(F + 4, 0x9100_43fd).word(F + 8, 0xf940_0c09);
    let s =
        sites(&[&one("test.site", F, "after", &[("signature", "\"d10043ff 910043fd f9400c08\""), ("offset", "0")])]);
    let e = resolve(&s, &space(&fx, &[], &[])).unwrap_err().to_string();
    assert!(e.contains("not found; nearest, all its words but one: 0x80001000 (word 2 differs)"), "{e}");
}

#[test]
fn a_site_in_another_sites_function_at_an_offset() {
    let fx =
        Fx::new().word(F, NOP).word(F + 4, 0xd100_43ff).word(F + 8, NOP).word(F + 12, 0xb942_1279).word(F + 16, RET);
    let sp = space(&fx, &[], &[]);
    let a = one("test.a", F, "entry", &[("signature", SIG_F)]);
    let b = |off: &str| {
        one(
            "test.b",
            F,
            "after",
            &[
                ("in", "\"test.a\""),
                ("offset", "4"),
                ("guards", &format!("[{{ offset = {off}, signature = \"b9421279\" }}]")),
            ],
        )
    };
    let found = resolve(&sites(&[&a, &b("8")]), &sp).unwrap();
    assert_eq!((found["test.a"].addr, found["test.b"].addr), (F + 4, F + 4));
    let e = resolve(&sites(&[&a, &b("4")]), &sp).unwrap_err().to_string();
    assert!(e.contains("in test.a's function, but its guard at +0x4 of the function differs"), "{e}");
}

// ---- anchors

fn anchor_fx() -> Fx {
    // f: sub sp; nop; bl g; nop; ldr x8,[x0]; ldr x8,[x8,#0x190]; blr x8; nop; bl g; nop; ret
    let f = [0xd100_43ff, NOP, bl(F + 8, G), NOP, LDR8_OBJ, LDR8_SLOT, BLR8, NOP, bl(F + 32, G), NOP, RET];
    Fx::new().func(F, &f).func(G, &[0xd100_83ff, RET]).func(H, &[0xd100_c3ff, RET])
}

fn point(anchor: &str, more: &[&str], fx: &Fx, sp: &Space<'_>) -> Result<u64, String> {
    let s = one("test.site", F, "before", &[("signature", SIG_F), ("point", anchor)]);
    let mut texts = vec![s.as_str()];
    texts.extend(more);
    let (_, found) = locate_all(&texts, fx, sp).map_err(|e| e.to_string())?;
    Ok(found["test.site"].offset.unwrap_or_default())
}

fn fails(anchor: &str, words: &[&str], fx: &Fx) -> String {
    let sp = space(fx, &[], &[]);
    let e = point(anchor, &[], fx, &sp).unwrap_err();
    for w in words {
        assert!(e.contains(w), "{w:?} not in {e}");
    }
    e
}

#[test]
fn words_shift_and_ordinal() {
    let fx = anchor_fx();
    let sp = space(&fx, &[], &[]);
    assert_eq!(point("{ words = \"f9400008 f940c908 d63f0100\", at = 2 }", &[], &fx, &sp), Ok(0x18));
    assert_eq!(point("{ words = \"f9400008 f940c908\", shift = 2 }", &[], &fx, &sp), Ok(0x18));
    assert_eq!(point("{ words = \"d503201f\", ordinal = 2 }", &[], &fx, &sp), Ok(0x1c));
    fails(
        "{ words = \"d503201f\" }",
        &["found 4 times", "+0x4, +0xc, +0x1c, +0x24", "an ordinal, a slice or more words"],
        &fx,
    );
    fails("{ words = \"d503201f\", ordinal = 4 }", &["4 matches", "no number 4"], &fx);
    // (of: the matches counted, so one more or one fewer moves no point unnoticed)
    assert_eq!(point("{ words = \"d503201f\", ordinal = 2, of = 4 }", &[], &fx, &sp), Ok(0x1c));
    fails("{ words = \"d503201f\", ordinal = 2, of = 3 }", &["4 matches in f_80001000", "not the 3 it expects"], &fx);
    fails("{ words = \"d503201f\", ordinal = 2, of = 5 }", &["4 matches in f_80001000", "not the 5 it expects"], &fx);
    fails("{ words = \"12345678\" }", &["not found in f_80001000"], &fx);
    fails("{ words = \"d65f03c0\", shift = 1 }", &["not an instruction"], &fx);
}

#[test]
fn calls_by_function_site_and_symbol() {
    let fx = anchor_fx();
    let sp = space(&fx, &[], &[]);
    assert_eq!(point("{ call = \"f_80002000\", ordinal = 1, shift = 1 }", &[], &fx, &sp), Ok(0x24));
    let g_site = one("test.g", G, "entry", &[("signature", "\"d10083ff\"")]);
    assert_eq!(point("{ call = \"test.g\", ordinal = 0 }", &[&g_site], &fx, &sp), Ok(0x8));
    fails("{ call = \"f_80003000\" }", &["not found"], &fx);
    fails("{ call = \"nosuch\" }", &["no site, function (f_<address>) or exported symbol is named nosuch"], &fx);
    // (a symbol: the export's address, reached through a PLT stub the lift knows)
    let stub = 0x8000_3800;
    let fx = Fx::new().func(F, &[0xd100_43ff, bl(F + 4, stub), RET]).func(H, &[0xd100_c3ff, RET]).plt(stub, H);
    let sp = space(&fx, &[], &[("_ZN2nn2os11SleepThreadE", H)]);
    assert_eq!(point("{ call = \"_ZN2nn2os11SleepThreadE\" }", &[], &fx, &sp), Ok(0x4));
}

#[test]
fn a_call_through_a_vtable_slot() {
    let fx = anchor_fx();
    let sp = space(&fx, &[], &[]);
    assert_eq!(point("{ vcall = 0x190 }", &[], &fx, &sp), Ok(0x18));
    fails("{ vcall = 0x198 }", &["not found"], &fx);
    // (the register's nearest load decides: x8 loaded again after the slot's load is not that slot's call)
    let fx = Fx::new().func(F, &[0xd100_43ff, LDR8_SLOT, 0xf940_0508, BLR8, RET]);
    fails("{ vcall = 0x190 }", &["not found"], &fx);
    let sp = space(&fx, &[], &[]);
    assert_eq!(point("{ vcall = 8 }", &[], &fx, &sp), Ok(0xc));
    // (nor one written by another instruction after the load: `mov x8, x9` calls what x9 holds)
    let fx = Fx::new().func(F, &[0xd100_43ff, LDR8_SLOT, 0xaa09_03e8, BLR8, RET]);
    fails("{ vcall = 0x190 }", &["not found"], &fx);
}

#[test]
fn a_string() {
    let mut data = vec![0u8; 0x3000];
    data.extend_from_slice(b"\0Title_00\0Other\0");
    let addr = F + 0x3001;
    let fx = Fx::new().func(F, &[0xd100_43ff, adrp(F + 4, addr, 1), NOP, add_lo(addr, 1, 1), RET]);
    let sp = space(&fx, &data, &[]);
    assert_eq!(point("{ string = \"Title_00\" }", &[], &fx, &sp), Ok(0xc));
    assert!(point("{ string = \"Title\" }", &[], &fx, &sp).is_err(), "not a whole string");
    assert_eq!(string_addresses(&sp, "Other"), BTreeSet::from([F + 0x300a]));
}

#[test]
fn a_slice() {
    let fx = anchor_fx();
    let sp = space(&fx, &[], &[]);
    let mark = one("test.mark", F, "after", &[("in", "\"test.site\""), ("offset", "0xc")]);
    assert_eq!(point("{ words = \"d503201f\", from = \"test.mark\", ordinal = 0 }", &[&mark], &fx, &sp), Ok(0x1c));
    assert_eq!(point("{ words = \"d503201f\", to = \"test.mark\" }", &[&mark], &fx, &sp), Ok(0x4));
    let both = "{ words = \"d503201f\", from = { vcall = 0x190 }, to = { call = \"f_80002000\", ordinal = 1 } }";
    assert_eq!(point(both, &[], &fx, &sp), Ok(0x1c));
    let other = one("test.other", H, "entry", &[("signature", "\"d100c3ff\"")]);
    let e = point("{ words = \"d503201f\", from = \"test.other\" }", &[&other], &fx, &sp).unwrap_err();
    assert!(e.contains("test.other's point is in f_80003000"), "{e}");
}

#[test]
fn waits_cycles_and_failures() {
    let fx = anchor_fx();
    let sp = space(&fx, &[], &[]);
    let a = one("test.a", F, "after", &[("in", "\"test.b\""), ("point", "{ words = \"d503201f\", ordinal = 0 }")]);
    let b = one("test.b", F, "after", &[("in", "\"test.a\""), ("point", "{ words = \"d503201f\", ordinal = 0 }")]);
    let e = locate_all(&[&a, &b], &fx, &sp).unwrap_err().to_string();
    assert!(e.contains("a cycle"), "{e}");
    let broken = one("test.broken", F, "after", &[("signature", SIG_F), ("point", "{ words = \"12345678\" }")]);
    let uses = one("test.uses", F, "after", &[("in", "\"test.broken\""), ("offset", "4")]);
    let later = one(
        "test.later",
        F,
        "before",
        &[("signature", SIG_F), ("point", "{ words = \"d503201f\", from = \"test.broken\" }")],
    );
    let e = locate_all(&[&broken, &uses, &later], &fx, &sp).unwrap_err().to_string();
    assert!(e.contains("2 site(s)"), "test.uses needs test.broken's function alone, which is found: {e}");
    assert!(e.contains("test.later: it names test.broken, which does not resolve"), "{e}");
}

#[test]
fn the_shape_of_jump_takeoff_velocity() {
    // (the same set-velocity call four times; another site's point between the second and the third; the site is the
    // second after it)
    let call = [LDR8_OBJ, LDR8_SLOT, BLR8];
    let mut f = vec![0xd100_43ff];
    f.extend(call);
    f.push(NOP);
    f.extend(call);
    f.push(0xf940_7a98);
    f.extend(call);
    f.push(NOP);
    f.extend(call);
    f.push(RET);
    let fx = Fx::new().func(F, &f);
    let sp = space(&fx, &[], &[]);
    let drive = "[[site]]\nname = \"test.drive\"\ndoc = \"d\"\nfunction = \"f_80001000\"\nat = \"after\"\nshape = \"data\"\n\
                 thread = \"worker\"\nsignature = \"f9407a98\"\nsignature_offset = 0x20\noffset = 0x20\n";
    let takeoff = one(
        "test.takeoff",
        F,
        "before",
        &[("in", "\"test.drive\""), ("point", "{ from = \"test.drive\", vcall = 0x190, ordinal = 1, of = 2 }")],
    );
    let (s, found) = locate_all(&[drive, &takeoff], &fx, &sp).unwrap();
    let at = &found["test.takeoff"];
    assert_eq!(at.offset, Some(4 * (f.len() as u64 - 2)), "not the last copy's blr");
    assert_eq!(at.addr, F);
    assert!(
        at.how.as_deref().unwrap_or_default().contains(
            "in test.drive's function; point: number 1 of 2 of the call through vtable slot 0x190 after test.drive"
        ),
        "{:?}",
        at.how
    );
    // (a third copy after the other site: the second is no longer surely the one, so the lift stops naming it)
    let mut more = f[..f.len() - 1].to_vec();
    more.extend(call);
    more.push(RET);
    let more_fx = Fx::new().func(F, &more);
    let more_sp = space(&more_fx, &[], &[]);
    let e = locate_all(&[drive, &takeoff], &more_fx, &more_sp).unwrap_err().to_string();
    assert!(
        e.contains(
            "test.takeoff: its point, number 1 of 2 of the call through vtable slot 0x190 after test.drive: 3 matches in \
             f_80001000"
        ),
        "{e}"
    );
    let image = fx.image();
    let p = place(&s, &Lift::new(&image, &fx.plan), &found, &HostList::default()).unwrap();
    assert_eq!(p.sites[1].point, Point::Before { pc: F + 4 * (f.len() as u64 - 2) });
}

// ---- fingerprints

fn fp_fx() -> Fx {
    let (s1, s2) = (F + 0x3001, F + 0x3010);
    Fx::new()
        .func(F, &[0xd100_43ff, bl(F + 4, H), adrp(F + 8, s1, 1), add_lo(s1, 1, 1), RET])
        .func(G, &[0xd100_83ff, bl(G + 4, H), NOP, NOP, NOP, NOP, RET])
        .func(H, &[0xd100_c3ff, adrp(H + 4, s2, 1), add_lo(s2, 1, 1), RET])
}

fn fp_data() -> Vec<u8> {
    let mut data = vec![0u8; 0x3000];
    data.extend_from_slice(b"\0menu\0");
    data.extend_from_slice(&[0; 10]);
    data.extend_from_slice(b"save\0");
    data
}

fn fingerprint_of(fp: &str) -> Result<(u64, String), String> {
    let fx = fp_fx();
    let data = fp_data();
    let sp = space(&fx, &data, &[]);
    let s = one("test.site", F, "entry", &[("fingerprint", fp)]);
    let (_, found) = locate_all(&[&s], &fx, &sp).map_err(|e| e.to_string())?;
    let at = &found["test.site"];
    Ok((at.addr, at.how.clone().unwrap_or_default()))
}

#[test]
fn a_fingerprint_found_exactly_once() {
    assert_eq!(fingerprint_of("{ calls = [\"f_80003000\"], strings = [\"menu\"] }").unwrap().0, F);
    assert_eq!(fingerprint_of("{ calls = [\"f_80003000\"], size = [6, 9] }").unwrap().0, G);
    assert_eq!(fingerprint_of("{ strings = [\"save\"] }").unwrap().0, H);
    assert_eq!(fingerprint_of("{ words = \"d503201f d503201f\" }").unwrap().0, G);
    assert_eq!(
        fingerprint_of("{ calls = [\"f_80003000\"], blocks = [1, 1], size = [1, 5] }").unwrap(),
        (F, "fingerprint".into())
    );
}

#[test]
fn a_fingerprint_found_several_times_or_none() {
    let fail = |fp: &str, words: &[&str]| {
        let e = fingerprint_of(fp).unwrap_err();
        for w in words {
            assert!(e.contains(w), "{w:?} not in {e}");
        }
    };
    fail("{ calls = [\"f_80003000\"] }", &["matches 2 functions", "f_80001000, f_80002000"]);
    fail(
        "{ calls = [\"f_80003000\"], strings = [\"save\"] }",
        &["no function passes", "f_80003000 fails calls f_80003000"],
    );
    fail("{ strings = [\"nowhere\"] }", &["no function makes the string 'nowhere'"]);
    fail("{ calls = [\"f_80003000\"], size = [100, 200] }", &["fails has 100 to 200 instructions"]);
}

#[test]
fn a_fingerprint_beside_a_signature() {
    let fx = fp_fx();
    let data = fp_data();
    let sp = space(&fx, &data, &[]);
    let agree = one("test.site", F, "entry", &[("signature", SIG_F), ("fingerprint", "{ strings = [\"menu\"] }")]);
    let (_, found) = locate_all(&[&agree], &fx, &sp).unwrap();
    assert_eq!(found["test.site"].how.as_deref(), Some("signature and fingerprint"));
    let differ = one("test.site", F, "entry", &[("signature", SIG_F), ("fingerprint", "{ strings = [\"save\"] }")]);
    let e = locate_all(&[&differ], &fx, &sp).unwrap_err().to_string();
    assert!(e.contains("its signature finds f_80001000, its fingerprint f_80003000"), "{e}");
}

// ---- placement

#[test]
fn after_an_instruction_and_at_an_entry() {
    let fx = Fx::new().func(F, &straight(F, 6));
    let enter = site(&[("name", Some("\"test.enter\"")), ("at", Some("\"entry\"")), ("offset", None)]);
    let p = placed(&[SITE, &enter], &fx, &found_at(&[("test.step", F), ("test.enter", F)])).unwrap();
    assert_eq!(p.sites.len(), 2);
    assert_eq!(p.sites[0].point, Point::After { pc: F + 8 });
    assert_eq!(p.sites[0].hook, Hook::Callback);
    assert_eq!(p.sites[1].point, Point::Entry { can_return: false });
    assert_eq!(
        p.log,
        [
            "hooks: test.step after f_80001000+0x8 [by signature]",
            "hooks: test.enter at f_80001000's entry [by signature]",
            "hooks: 2 site(s), each found exactly once and placed (2 by signature)",
        ]
    );
}

#[test]
fn the_signature_may_start_inside_the_function() {
    let fx = Fx::new().func(F, &straight(F, 6));
    let s = site(&[("signature_offset", Some("0x8")), ("offset", Some("0x10"))]);
    let p = placed(&[&s], &fx, &found_at(&[("test.step", F + 8)])).unwrap();
    assert_eq!(p.sites[0].point, Point::After { pc: F + 0x10 });
}

#[test]
fn what_does_not_fit_stops() {
    let mut body = straight(F, 6);
    body[2] = cbz(F + 8, F + 0x14);
    let cases = [
        (Fx::new().func(F, &body), F, "is a cbz, not a plain instruction"),
        (Fx::new().func(F, &straight(F, 2)), F, "is not an instruction of the function"),
        (Fx::new().func(F + 4, &straight(F + 4, 6)), F + 4, "finds f_80001004, the table names f_80001000"),
        (Fx::new().func(F, &straight(F, 6)), F + 0x40, "where the lift found none"),
    ];
    for (fx, at, what) in cases {
        let e = placed(&[SITE], &fx, &found_at(&[("test.step", at)])).unwrap_err().to_string();
        assert!(e.contains("test.step: ") && e.contains(what), "{what:?} not in {e}");
    }
}

#[test]
fn a_point_in_a_loop_is_reported() {
    let mut body = straight(F, 6);
    body[3] = bcond(F + 0xc, F + 4); // (b.ne back over the point at +8)
    let p = placed(&[SITE], &Fx::new().func(F, &body), &found_at(&[("test.step", F)])).unwrap();
    assert!(p.log[0].contains("in a loop"), "{}", p.log[0]);
    body[3] = cbz(F + 0xc, F + 0x10); // (forward only)
    let p = placed(&[SITE], &Fx::new().func(F, &body), &found_at(&[("test.step", F)])).unwrap();
    assert!(!p.log[0].contains("in a loop"), "{}", p.log[0]);
    body[3] = br(16); // (a jump table back to it)
    let mut fx = Fx::new().func(F, &body);
    fx.plan.jump_tables.insert(F + 0xc, vec![F + 4, F + 0x10]);
    assert!(placed(&[SITE], &fx, &found_at(&[("test.step", F)])).unwrap().log[0].contains("in a loop"));
    body[1] = b(F + 4, F + 0x10); // (a jump over the point: the table's way back never reaches it)
    let mut fx = Fx::new().func(F, &body);
    fx.plan.jump_tables.insert(F + 0xc, vec![F + 4, F + 0x10]);
    assert!(!placed(&[SITE], &fx, &found_at(&[("test.step", F)])).unwrap().log[0].contains("in a loop"));
}

#[test]
fn code_shared_with_another_function_is_said() {
    // (g jumps into f's code: the lift writes that code in g too)
    let fx = Fx::new()
        .func(F, &straight(F, 6))
        .word(G, NOP)
        .word(G + 4, b(G + 4, F + 4))
        .body(G, &[G, G + 4, F + 4, F + 8, F + 12, F + 16]);
    let p = placed(&[SITE], &fx, &found_at(&[("test.step", F)])).unwrap();
    assert!(p.log[0].contains("1 other function(s) share"), "{}", p.log[0]);
    assert_eq!(p.sites[0].holders, [F, G]);
}

const VALUES: &str = "HOS_VALUE(frame_seconds, \"frame.seconds\", uint32_t, hos_frame_seconds_bits)\n\
                      HOS_VALUE(frame_probe, \"frame.probe\", uint64_t, hos_step_probe)\n";

#[test]
fn each_form_is_placed() {
    let mut body = straight(F, 8);
    body[3] = cbz(F + 0xc, F + 0x18);
    let fx = Fx::new().func(F, &body);
    let texts = [
        site(&[("name", Some("\"t.before\"")), ("at", Some("\"before\"")), ("offset", Some("0xc"))]),
        site(&[("name", Some("\"t.replace\"")), ("at", Some("\"replace\"")), ("offset", Some("0x10"))]),
        site(&[("name", Some("\"t.branch\"")), ("at", Some("\"branch\"")), ("offset", Some("0xc"))]),
        site(&[("name", Some("\"t.ret\"")), ("at", Some("\"return\"")), ("offset", None)]),
        site(&[
            ("name", Some("\"t.enter\"")),
            ("at", Some("\"entry\"")),
            ("offset", None),
            ("can_return", Some("true")),
        ]),
        rule(&[("name", Some("\"t.rule\"")), ("offset", Some("0x14"))]),
        site(&[
            ("name", Some("\"t.note\"")),
            ("at", Some("\"return\"")),
            ("offset", None),
            ("rule", Some("\"note_u64\"")),
            ("value", Some("\"frame.probe\"")),
            ("shape", Some("\"data\"")),
            ("args", Some("[\"x19:ptr:what\"]")),
        ]),
    ];
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let s = sites(&texts);
    let found: Found = s.iter().map(|x| (x.name.clone(), Location { addr: F, ..Location::default() })).collect();
    let image = fx.image();
    let p = place(&s, &Lift::new(&image, &fx.plan), &found, &HostList::parse(VALUES)).unwrap();
    let points: Vec<Point> = p.sites.iter().map(|x| x.point).collect();
    assert_eq!(
        points,
        [
            Point::Before { pc: F + 0xc },
            Point::Replace { pc: F + 0x10, callee: None },
            Point::Branch { pc: F + 0xc },
            Point::Return,
            Point::Entry { can_return: true },
            Point::After { pc: F + 0x14 },
            Point::Return,
        ]
    );
    assert_eq!(
        p.sites[5].hook,
        Hook::Rule { rule: Rule::MulF32, value: "frame.seconds".into(), symbol: "hos_frame_seconds_bits".into() }
    );
    assert_eq!(
        p.sites[6].hook,
        Hook::Rule { rule: Rule::NoteU64, value: "frame.probe".into(), symbol: "hos_step_probe".into() }
    );
}

#[test]
fn what_does_not_fit_a_form() {
    let fx = Fx::new().func(F, &straight(F, 8));
    let found = found_at(&[("test.step", F)]);
    for (text, what) in [
        (site(&[("at", Some("\"branch\""))]), "is a op, not a conditional branch"),
        (site(&[("at", Some("\"replace\"")), ("offset", Some("0x1c"))]), "is a ret, not a plain instruction or a call"),
    ] {
        let e = placed(&[text.as_str()], &fx, &found).unwrap_err().to_string();
        assert!(e.contains(what), "{what:?} not in {e}");
    }
    let replace_after = [SITE, &site(&[("name", Some("\"test.again\"")), ("at", Some("\"replace\""))])];
    let found2 = found_at(&[("test.step", F), ("test.again", F)]);
    let e = placed(&replace_after, &fx, &found2).unwrap_err().to_string();
    assert!(e.contains("one or the other"), "{e}");
    let p = placed(&[&site(&[("at", Some("\"before\"")), ("offset", Some("0x1c"))])], &fx, &found).unwrap();
    assert_eq!(p.sites[0].point, Point::Before { pc: F + 0x1c }, "before takes any instruction, a ret too");
}

#[test]
fn allow_jump_and_a_replaced_call() {
    let mut body = straight(F, 8);
    body[3] = cbz(F + 0xc, F + 0x18);
    body[4] = bl(F + 0x10, 0x8000_2000);
    // (the bl's target a PLT stub: the lift calls what it calls; another function holds f+8, f+0x14 and f+0x18)
    let fx = Fx::new().func(F, &body).body(0x8000_1100, &[F + 8, F + 0x14, F + 0x18]).plt(0x8000_2000, H).entry(H);
    let texts = [
        site(&[("name", Some("\"t.allow\"")), ("at", Some("\"allow\"")), ("offset", Some("0xc"))]),
        site(&[("name", Some("\"t.jump\"")), ("at", Some("\"jump\"")), ("offset", Some("0x8")), ("to", Some("0x14"))]),
        site(&[("name", Some("\"t.call\"")), ("at", Some("\"replace\"")), ("offset", Some("0x10"))]),
    ];
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let found = found_at(&[("t.allow", F), ("t.jump", F), ("t.call", F)]);
    let p = placed(&texts, &fx, &found).unwrap();
    assert_eq!(p.sites[0].point, Point::Allow { pc: F + 0xc });
    assert_eq!(p.sites[1].point, Point::Jump { pc: F + 8, to: F + 0x14 });
    assert_eq!(p.sites[2].point, Point::Replace { pc: F + 0x10, callee: Some(H) });
    assert!(p.labels[&F].contains(&(F + 0x14)), "the jump's target is a label of the function");
    assert!(p.labels[&0x8000_1100].contains(&(F + 0x14)), "and of the other function the code is lifted in");
    let e =
        placed(&[&site(&[("at", Some("\"allow\"")), ("offset", Some("0x8"))])], &fx, &found_at(&[("test.step", F)]))
            .unwrap_err()
            .to_string();
    assert!(e.contains("is a op, not a conditional branch"), "{e}");
    let e = placed(
        &[&site(&[("at", Some("\"jump\"")), ("to", Some("0x40"))])],
        &Fx::new().func(F, &body),
        &found_at(&[("test.step", F)]),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("where it jumps, is not an instruction"), "{e}");
    let both = [
        site(&[("name", Some("\"t.a\"")), ("at", Some("\"jump\"")), ("to", Some("0x14"))]),
        site(&[("name", Some("\"t.b\"")), ("at", Some("\"before\""))]),
    ];
    let both: Vec<&str> = both.iter().map(String::as_str).collect();
    let e = placed(&both, &Fx::new().func(F, &body), &found_at(&[("t.a", F), ("t.b", F)])).unwrap_err().to_string();
    assert!(e.contains("one site before an instruction"), "{e}");
}

#[test]
fn what_a_replaced_call_allow_and_jump_refuse() {
    let mut body = straight(F, 8);
    body[3] = cbz(F + 0xc, F + 0x18);
    body[4] = bl(F + 0x10, 0x8000_2000);
    let call = site(&[("name", Some("\"t.call\"")), ("at", Some("\"replace\"")), ("offset", Some("0x10"))]);
    let found = found_at(&[("t.call", F)]);
    for (fx, what) in [
        (Fx::new().func(F, &body), "not a function the lift calls plainly"),
        (Fx::new().func(F, &body).entry(0x8000_2000).setjmp(0x8000_2000), "a setjmp owner"),
    ] {
        let e = placed(&[&call], &fx, &found).unwrap_err().to_string();
        assert!(e.contains(what), "{what:?} not in {e}");
    }
    for (first, second) in [("allow", "branch"), ("branch", "allow")] {
        let pair = [
            site(&[("name", Some("\"t.a\"")), ("at", Some(&format!("\"{first}\""))), ("offset", Some("0xc"))]),
            site(&[("name", Some("\"t.b\"")), ("at", Some(&format!("\"{second}\""))), ("offset", Some("0xc"))]),
        ];
        let pair: Vec<&str> = pair.iter().map(String::as_str).collect();
        let e = placed(&pair, &Fx::new().func(F, &body), &found_at(&[("t.a", F), ("t.b", F)])).unwrap_err().to_string();
        assert!(e.contains("one per branch"), "{e}");
    }
    for (first, second) in [("after", "replace"), ("replace", "after")] {
        // (C after an instruction the other replaces: either order)
        let pair = [
            site(&[("name", Some("\"t.a\"")), ("at", Some(&format!("\"{first}\""))), ("offset", Some("0x8"))]),
            site(&[("name", Some("\"t.b\"")), ("at", Some(&format!("\"{second}\""))), ("offset", Some("0x8"))]),
        ];
        let pair: Vec<&str> = pair.iter().map(String::as_str).collect();
        let e = placed(&pair, &Fx::new().func(F, &body), &found_at(&[("t.a", F), ("t.b", F)])).unwrap_err().to_string();
        assert!(e.contains("one or the other"), "{first} then {second}: {e}");
    }
    let other = Fx::new().func(F, &body).body(0x8000_1100, &[F + 8, F + 0x18]); // (no f+0x14 there)
    let e = placed(&[&site(&[("at", Some("\"jump\"")), ("to", Some("0x14"))])], &other, &found_at(&[("test.step", F)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("which does not have"), "{e}");
}

#[test]
fn a_jump_to_another_function_s_entry_is_refused() {
    // (f+0x14 is also an entry: the lifted code would tail-call it from the jump, not go to the instruction)
    let fx = Fx::new().func(F, &straight(F, 8)).entry(F + 0x14);
    let e = placed(&[&site(&[("at", Some("\"jump\"")), ("to", Some("0x14"))])], &fx, &found_at(&[("test.step", F)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("is a function's entry"), "{e}");
}

#[test]
fn a_jump_back_is_a_loop_and_a_label_after_a_call_warns() {
    let mut body = straight(F, 8);
    body[3] = bl(F + 0xc, 0x8000_2000);
    let found = found_at(&[("test.step", F)]);
    let jump = site(&[("at", Some("\"jump\"")), ("offset", Some("0x14")), ("to", Some("0x4"))]);
    assert!(placed(&[&jump], &Fx::new().func(F, &body), &found).unwrap().log[0].contains("in a loop"));
    let before = site(&[("at", Some("\"before\"")), ("offset", Some("0x10"))]);
    let labelled = Fx::new().func(F, &body).label(F, F + 0x10); // (the instruction after the call is a label)
    assert!(placed(&[&before], &labelled, &found).unwrap().log[0].contains("WARNING: after a call, but a label"));
    assert!(!placed(&[&before], &Fx::new().func(F, &body), &found).unwrap().log[0].contains("WARNING"));
}

// ---- apply: the tables' folder, the host's list

/// A folder of tables, as game/ holds them, under the build's target directory (on disk).
struct TempTables(std::path::PathBuf);

impl TempTables {
    fn new(name: &str, defs: &str, locs: &str, host: &str) -> TempTables {
        let root = target_dir().join("sites-tests").join(name);
        let game = root.join("game");
        let build = game.join("sites").join(crate::table::tests::BUILD);
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(game.join("sites").join("sites.toml"), defs).unwrap();
        std::fs::write(build.join("locations.toml"), locs).unwrap();
        std::fs::write(game.join("sites.def"), host).unwrap();
        TempTables(game)
    }
}

/// A one-file table split as the repository keeps it: (definitions, locations).
fn split(text: &str) -> (String, String) {
    let defs_keys = [
        "name",
        "kind",
        "doc",
        "at",
        "shape",
        "thread",
        "args",
        "returns",
        "module",
        "can_return",
        "rule",
        "value",
        "require",
    ];
    let (mut defs, mut locs) = (Vec::new(), Vec::new());
    for l in text.lines() {
        let key = l.split_once(" = ").map_or(l.trim(), |(k, _)| k);
        match key {
            "format" => {
                defs.push("format = 2".to_string());
                locs.push("format = 2".to_string());
            }
            "build" => locs.push(l.to_string()),
            "[[site]]" | "name" | "" => {
                defs.push(l.to_string());
                locs.push(l.to_string());
            }
            k if defs_keys.contains(&k) => defs.push(l.to_string()),
            _ => locs.push(l.to_string()),
        }
    }
    (defs.join("\n") + "\n", locs.join("\n") + "\n")
}

#[test]
fn apply_resolves_and_places_and_checks_the_host() {
    let fx = Fx::new().func(F, &[NOP, NOP, 0xd100_43ff, 0x9100_43fd, 0xf940_0c08, RET]);
    let image = fx.image();
    let (defs, locs) = split(&table(&[&site(&[("signature_offset", Some("0x8"))])]));
    let t = TempTables::new("apply-ok", &defs, &locs, "/* x */\nHOS_SITE_ENGINE(test_step, \"test.step\", fn)\n");
    let (s, p) = apply(&Tables::new(&t.0), &image, &fx.plan).unwrap().unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(p.sites[0].point, Point::After { pc: F + 8 });
    assert!(p.log[0].contains("hooks: test.step after f_80001000+0x8"), "{}", p.log[0]);
    let t = TempTables::new("apply-no-slot", &defs, &locs, "");
    let e = apply(&Tables::new(&t.0), &image, &fx.plan).unwrap_err().to_string();
    assert!(e.contains("game/sites.def has no HOS_SITE line for test.step"), "{e}");
    let (defs2, locs2) = split(&table(&[&site(&[("shape", Some("\"filter\""))])]));
    let t = TempTables::new("apply-bad-table", &defs2, &locs2, "");
    let e = apply(&Tables::new(&t.0), &image, &fx.plan).unwrap_err().to_string();
    assert!(e.contains("serial"), "a table that does not read stops the lift as well: {e}");
}

#[test]
fn the_outcome_lines_are_the_worker_sites_that_change_the_outcome() {
    // (a branch site on a worker thread: an engine feature's, and the host's HOS_SITE_OUTCOME lines must name exactly it,
    // so the host refuses it a callback set at run time)
    let fx = Fx::new().func(F, &straight(F, 6));
    let image = fx.image();
    let (defs, locs) = split(&table(&[&site(&[("at", Some("\"branch\""))])]));
    let engine = "HOS_SITE_ENGINE(test_step, \"test.step\", fn)\n";
    for (i, (host, stops)) in [
        (engine.to_string(), true),
        (format!("{engine}HOS_SITE_OUTCOME(test_step, \"test.step\")\n"), false),
        (
            format!(
                "{engine}HOS_SITE_OUTCOME(test_step, \"test.step\")\nHOS_SITE_OUTCOME(test_other, \"test.other\")\n"
            ),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let t = TempTables::new(&format!("outcome-{i}"), &defs, &locs, &host);
        // (past this check the made-up code has no conditional branch there: what stops it then is not this rule)
        let said = apply(&Tables::new(&t.0), &image, &fx.plan).err().map(|e| e.to_string()).unwrap_or_default();
        assert_eq!(said.contains("HOS_SITE_OUTCOME"), stops, "{host}: {said}");
    }
    // (who may change a worker thread's outcome: the engine sites of the list apply's tables hold)
    let t = TempTables::new("outcome-plain", &defs, &locs, "HOS_SITE(test_step, \"test.step\")\n");
    let e = apply(&Tables::new(&t.0), &image, &fx.plan).unwrap_err().to_string();
    assert!(e.contains("changes the game's outcome"), "{e}");
    let t = TempTables::new("outcome-engine", &defs, &locs, engine);
    let e = apply(&Tables::new(&t.0), &image, &fx.plan).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(!e.contains("changes the game's outcome on a worker thread: such a site"), "{e}");
}

#[test]
fn a_rule_needs_its_value_on_the_host() {
    let fx = Fx::new().func(F, &[0xd100_43ff, 0x9100_43fd, 0xf940_0c08, NOP]);
    let image = fx.image();
    let (defs, locs) = split(&table(&[&rule(&[])]));
    for (i, (host, ok)) in [
        ("", false),
        ("HOS_VALUE(frame_seconds, \"frame.seconds\", uint64_t, sym)\n", false),
        ("HOS_VALUE(frame_seconds, \"frame.seconds\", uint32_t, hos_frame_seconds_bits)\n", true),
    ]
    .into_iter()
    .enumerate()
    {
        let t = TempTables::new(&format!("rule-{i}"), &defs, &locs, host);
        let got = apply(&Tables::new(&t.0), &image, &fx.plan);
        if ok {
            let (_, p) = got.unwrap().unwrap();
            assert!(matches!(&p.sites[0].hook, Hook::Rule { symbol, .. } if symbol == "hos_frame_seconds_bits"));
        } else {
            let e = got.unwrap_err().to_string();
            assert!(e.contains("HOS_VALUE"), "{e}");
        }
    }
}

#[test]
fn a_vslot_site_is_not_placed_yet() {
    let fx = Fx::new();
    let (defs, locs) = split(&table(&[VSLOT]));
    let t = TempTables::new("vslot", &defs, &locs, "HOS_SITE(actor_update, \"actor.update\")\n");
    let e = apply(&Tables::new(&t.0), &fx.image(), &fx.plan).unwrap_err().to_string();
    assert!(e.contains("actor.update") && e.contains("not implemented"), "{e}");
}

#[test]
fn no_table_places_nothing() {
    let fx = Fx::new().func(F, &straight(F, 4));
    let root = target_dir().join("sites-tests").join("none").join("game");
    std::fs::create_dir_all(&root).unwrap();
    assert!(apply(&Tables::new(&root), &fx.image(), &fx.plan).unwrap().is_none());
}

/// Cargo's target folder (CARGO_TARGET_DIR, else the workspace root's target/, which .gitignore leaves out): the tests'
/// tables go there, never inside the checkout's tracked tree.
fn target_dir() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |t| root.join(t))
}
