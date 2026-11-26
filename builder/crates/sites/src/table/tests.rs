//! The reader's checks (tests/test_hooks.py's Reader and Forms, tests/test_hook_anchors.py's Reader and Split): a table
//! that follows the format reads; each rule broken stops with the file, the site and the rule.

use super::*;

pub(crate) const BUILD: &str = "0123456789abcdef0123456789abcdef01234567";

pub(crate) fn head() -> String {
    format!("format = 1\nbuild = \"{BUILD}\"\n")
}

pub(crate) const SITE: &str = r#"
[[site]]
name = "test.step"
doc = "A step"
function = "f_80001000"
at = "after"
offset = 0x8
signature = "d10043ff 9100?3fd f9400000/ffc003e0"
shape = "event"
thread = "worker"
args = ["x19:ptr:anim", "s0:f32:time", "w9:u32"]
"#;

/// SITE with keys replaced (`None`: the key taken out), new ones added at the end.
pub(crate) fn site(changes: &[(&str, Option<&str>)]) -> String {
    let mut left: Vec<(&str, Option<&str>)> = changes.to_vec();
    let mut out = Vec::new();
    for line in SITE.trim().lines() {
        let key = line.split_once(" = ").map(|(k, _)| k);
        match key.and_then(|k| left.iter().position(|(c, _)| *c == k)) {
            Some(i) => {
                let (k, v) = left.remove(i);
                if let Some(v) = v {
                    out.push(format!("{k} = {v}"));
                }
            }
            None => out.push(line.to_string()),
        }
    }
    out.extend(left.into_iter().filter_map(|(k, v)| v.map(|v| format!("{k} = {v}"))));
    format!("\n{}\n", out.join("\n"))
}

pub(crate) fn table(sites: &[&str]) -> String {
    head() + &sites.concat()
}

pub(crate) fn read(text: &str, engine: Option<&BTreeSet<String>>) -> Result<Vec<Site>, TableError> {
    parse_table(text, "game/sites/x/sites.toml", Some(BUILD), engine)
}

fn bad(text: &str, words: &[&str]) -> String {
    let e = read(text, None).expect_err(text).to_string();
    for w in words {
        assert!(e.contains(w), "{w:?} not in: {e}");
    }
    assert!(e.contains("game/sites/x/sites.toml"), "{e}");
    e
}

#[test]
fn a_table_reads() {
    let other = site(&[
        ("name", Some("\"test.other\"")),
        ("at", Some("\"entry\"")),
        ("offset", None),
        ("shape", Some("\"filter\"")),
        ("thread", Some("\"serial\"")),
        ("returns", Some("\"u32\"")),
    ]);
    let sites = read(&table(&[SITE, &other]), None).unwrap();
    assert_eq!(sites.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["test.step", "test.other"]);
    let (s, e) = (sites[0].code().unwrap(), sites[1].code().unwrap());
    assert_eq!((s.function, s.form, s.offset, s.signature_offset), (0x8000_1000, Form::After, Some(8), 0));
    assert_eq!(sites[0].module, "main");
    assert_eq!(s.signature.as_ref().unwrap().to_string(), "d10043ff 9100?3fd f9400000/ffc003e0");
    let args: Vec<String> = sites[0].args.iter().map(ToString::to_string).collect();
    assert_eq!(args, ["x19:ptr:anim", "s0:f32:time", "w9:u32"]);
    assert_eq!(sites[0].ident(), "test_step");
    assert_eq!((e.form, e.offset, sites[1].returns), (Form::Entry, Some(0), Returns::Value(ArgType::U32)));
}

#[test]
fn an_empty_table_reads() {
    assert!(read(&head(), None).unwrap().is_empty());
    let upper = parse_table(&head(), "t", Some(&BUILD.to_ascii_uppercase()), None).unwrap();
    assert!(upper.is_empty(), "the folder's name in capitals is the same build");
}

#[test]
fn the_table_itself() {
    bad("format = 1\nbuild = ", &["not TOML"]);
    bad(&head().replace("format = 1", "format = 2"), &["format = 1"]);
    bad(&head().replace(BUILD, "e0f3ccfa"), &["build"]);
    bad(&head().replace(BUILD, &format!("e{}", &BUILD[1..])), &["the folder's is"]);
    bad(&(head() + "sites = []\n"), &["unknown key", "sites"]);
    bad(&(head() + "site = 1\n"), &["[[site]]"]);
}

#[test]
fn each_rule_of_a_site() {
    let long = format!("\"{}\"", vec!["d10043ff"; 17].join(" "));
    let cases: Vec<(String, Vec<&str>)> = vec![
        (site(&[("nme", Some("\"x\""))]), vec!["unknown key", "nme"]),
        (site(&[("name", Some("\"Text.step\""))]), vec!["name"]),
        (site(&[("name", Some("\"text..step\""))]), vec!["name"]),
        (site(&[("name", Some("\"text.step-2\""))]), vec!["name"]),
        (site(&[("doc", Some("\"\""))]), vec!["doc"]),
        (site(&[("doc", None)]), vec!["doc"]),
        (site(&[("kind", Some("\"vtable\""))]), vec!["kind"]),
        (site(&[("shape", Some("\"hook\""))]), vec!["shape"]),
        (site(&[("thread", Some("\"main\""))]), vec!["thread"]),
        (site(&[("shape", Some("\"filter\""))]), vec!["serial"]),
        (site(&[("args", Some("[\"x31:ptr\"]"))]), vec!["no register x31"]),
        (site(&[("args", Some("[\"x19:f32\"]"))]), vec!["a x register holds"]),
        (site(&[("args", Some("[\"s0:u32\"]"))]), vec!["a s register holds"]),
        (site(&[("args", Some("[\"q0:f32\"]"))]), vec!["not an argument"]),
        (site(&[("args", Some("\"x0:ptr\""))]), vec!["args is a list"]),
        (site(&[("args", Some("[\"x0:ptr:a\", \"x1:ptr:a\"]"))]), vec!["share a name"]),
        (site(&[("returns", Some("\"string\""))]), vec!["returns"]),
        (site(&[("module", Some("\"sdk\""))]), vec!["module"]),
        (site(&[("function", None)]), vec!["function"]),
        (site(&[("function", Some("\"f_80b1e14\""))]), vec!["function"]),
        (site(&[("function", Some("\"0x80b1e140\""))]), vec!["function"]),
        (site(&[("at", Some("\"around\""))]), vec!["at"]),
        (site(&[("at", Some("\"entry\""))]), vec!["offset is for"]),
        (site(&[("offset", None)]), vec!["offset"]),
        (site(&[("offset", Some("6"))]), vec!["multiple of 4"]),
        (site(&[("offset", Some("-4"))]), vec!["offset"]),
        (site(&[("signature_offset", Some("2"))]), vec!["signature_offset"]),
        (site(&[("signature", None)]), vec!["signature"]),
        (site(&[("signature", Some("\"d10043ff 9100\""))]), vec!["signature", "not a word"]),
        (site(&[("signature", Some("\"d10043fg\""))]), vec!["signature", "not a word"]),
        (site(&[("signature", Some("\"????????\""))]), vec!["unmasked"]),
        (site(&[("signature", Some("\"d10043ff 00000003/00000001\""))]), vec!["bits its mask"]),
        (site(&[("signature", Some(&long))]), vec!["at most"]),
    ];
    for (text, words) in cases {
        let message = bad(&table(&[&text]), &words);
        assert!(message.contains("site "), "{message}");
    }
}

#[test]
fn names_are_one_each() {
    bad(&table(&[SITE, SITE]), &["test.step", "taken"]);
    let again = site(&[("name", Some("\"test.step\"")), ("offset", Some("0xc"))]);
    bad(&table(&[SITE, &again]), &["taken"]);
}

pub(crate) const VSLOT: &str = "\n[[site]]\nname = \"actor.update\"\nkind = \"vslot\"\ndoc = \"An actor's update\"\n\
                                vtable = 0x84000000\nslot = 5\nshape = \"event\"\nthread = \"worker\"\n\
                                args = [\"x0:ptr:actor\"]\n";

#[test]
fn a_vslot_site_reads() {
    let sites = read(&table(&[VSLOT]), None).unwrap();
    assert_eq!(sites[0].kind, Kind::Vslot { function: None, vtable: 0x8400_0000, slot: 5 });
    bad(&table(&[&VSLOT.replace("\"event\"", "\"filter\"").replace("\"worker\"", "\"serial\"")]), &["vslot"]);
    bad(&table(&[&VSLOT.replace("slot = 5", "slot = -1")]), &["slot"]);
    bad(&table(&[&VSLOT.replace("vtable = 0x84000000", "vtable = 0x84000004")]), &["vtable"]);
    bad(&table(&[&VSLOT.replace("slot = 5", "slot = 5\nsignature = \"d10043ff\"")]), &["unknown key", "signature"]);
}

/// A mul_f32 rule site, with keys changed.
pub(crate) fn rule(more: &[(&str, Option<&str>)]) -> String {
    let mut keys = vec![
        ("rule", Some("\"mul_f32\"")),
        ("value", Some("\"frame.seconds\"")),
        ("shape", Some("\"data\"")),
        ("args", Some("[\"s8:f32:out\", \"s1:f32:in\"]")),
    ];
    for &(k, v) in more {
        keys.retain(|(kk, _)| *kk != k);
        keys.push((k, v));
    }
    site(&keys)
}

#[test]
fn the_forms_rules_and_returns() {
    for (at, form) in [("before", Form::Before), ("replace", Form::Replace), ("branch", Form::Branch)] {
        let s = read(&table(&[&site(&[("at", Some(&format!("\"{at}\"")))])]), None).unwrap();
        assert_eq!((s[0].code().unwrap().form, s[0].code().unwrap().offset), (form, Some(8)));
    }
    let ret = read(&table(&[&site(&[("at", Some("\"return\"")), ("offset", None)])]), None).unwrap();
    assert_eq!(ret[0].code().unwrap().form, Form::Return);
    let enter = site(&[("at", Some("\"entry\"")), ("offset", None), ("can_return", Some("true"))]);
    assert!(read(&table(&[&enter]), None).unwrap()[0].code().unwrap().can_return);
    bad(&table(&[&site(&[("at", Some("\"return\""))])]), &["offset is for"]);
    bad(&table(&[&site(&[("at", Some("\"exit\""))])]), &["at"]);
    bad(&table(&[&site(&[("can_return", Some("true"))])]), &["can_return is for"]);
    let yes = site(&[("at", Some("\"entry\"")), ("offset", None), ("can_return", Some("\"yes\""))]);
    bad(&table(&[&yes]), &["true or false"]);
    let s = read(&table(&[&rule(&[])]), None).unwrap();
    assert_eq!(s[0].code().unwrap().rule, Some(RuleUse { rule: Rule::MulF32, value: "frame.seconds".into() }));
    bad(&table(&[&rule(&[("rule", Some("\"add_f32\""))])]), &["rule"]);
    bad(&table(&[&rule(&[("shape", Some("\"event\""))])]), &["data rule"]);
    bad(&table(&[&rule(&[("at", Some("\"replace\""))])]), &["inlined at"]);
    bad(&table(&[&rule(&[("value", None)])]), &["value"]);
    bad(&table(&[&rule(&[("args", Some("[\"s8:f32:out\"]"))])]), &["argument named in"]);
    bad(&table(&[&rule(&[("args", Some("[\"s8:f32:out\", \"x1:u64:in\"]"))])]), &["argument named in"]);
    bad(&table(&[&site(&[("value", Some("\"frame.seconds\""))])]), &["value is for a rule"]);
}

#[test]
fn allow_and_jump_read() {
    let s = read(&table(&[&site(&[("at", Some("\"jump\"")), ("to", Some("0x14"))])]), None).unwrap();
    let c = s[0].code().unwrap();
    assert_eq!((c.form, c.offset, c.to), (Form::Jump, Some(8), Some(0x14)));
    let s = read(&table(&[&site(&[("at", Some("\"allow\""))])]), None).unwrap();
    assert_eq!((s[0].code().unwrap().form, s[0].code().unwrap().to), (Form::Allow, None));
    bad(&table(&[&site(&[("at", Some("\"jump\""))])]), &["to (from the function's entry"]);
    bad(&table(&[&site(&[("at", Some("\"jump\"")), ("to", Some("0x13"))])]), &["a multiple of 4"]);
    bad(&table(&[&site(&[("to", Some("0x14"))])]), &["to is for at = \"jump\""]);
    let jump_return = site(&[("at", Some("\"jump\"")), ("to", Some("0x14")), ("can_return", Some("true"))]);
    bad(&table(&[&jump_return]), &["can_return is for"]);
}

#[test]
fn who_may_change_the_outcome_on_a_worker_thread() {
    let engine: BTreeSet<String> = ["test.engine".to_string()].into();
    let outcome: Vec<Vec<(&str, Option<&str>)>> = vec![
        vec![("at", Some("\"branch\""))],
        vec![("at", Some("\"allow\""))],
        vec![("at", Some("\"replace\""))],
        vec![("at", Some("\"jump\"")), ("to", Some("0x14"))],
        vec![("at", Some("\"entry\"")), ("offset", None), ("can_return", Some("true"))],
    ];
    for form in outcome {
        let mut named = form.clone();
        named.push(("name", Some("\"test.engine\"")));
        assert_eq!(read(&table(&[&site(&named)]), Some(&engine)).unwrap()[0].name, "test.engine");
        let e = read(&table(&[&site(&form)]), Some(&engine)).unwrap_err().to_string();
        for w in ["worker thread", "HOS_SITE_ENGINE", "data rule", "serial"] {
            assert!(e.contains(w), "{w}: {e}");
        }
        read(&table(&[&site(&form)]), None).expect("no list: the rule left out");
        let mut serial = form.clone();
        serial.push(("thread", Some("\"serial\"")));
        read(&table(&[&site(&serial)]), Some(&engine)).expect("on the serial thread any site may");
    }
    for at in ["\"after\"", "\"before\""] {
        read(&table(&[&site(&[("at", Some(at))])]), Some(&engine)).unwrap();
    }
    read(&table(&[&site(&[("at", Some("\"return\"")), ("offset", None)])]), Some(&engine)).unwrap();
}

pub(crate) const SIG_F: &str = "\"d10043ff\"";

/// A site of the anchor checks' shape: name, function, form, then its other keys as written.
pub(crate) fn one(name: &str, function: u64, at: &str, keys: &[(&str, &str)]) -> String {
    let mut lines = vec![
        "[[site]]".to_string(),
        format!("name = \"{name}\""),
        "doc = \"d\"".into(),
        format!("function = \"f_{function:08x}\""),
        format!("at = \"{at}\""),
        "shape = \"event\"".into(),
        "thread = \"worker\"".into(),
    ];
    lines.extend(keys.iter().map(|(k, v)| format!("{k} = {v}")));
    lines.join("\n") + "\n"
}

#[test]
fn points_ins_and_fingerprints_read() {
    let a = one(
        "a.b",
        0x8000_1000,
        "before",
        &[
            ("signature", SIG_F),
            ("point", "{ words = \"d503201f d65f03c0\", at = 1, from = \"a.c\", ordinal = 0, shift = -1 }"),
        ],
    );
    let b = one("a.c", 0x8000_1000, "after", &[("in", "\"a.b\""), ("offset", "4")]);
    let c = one(
        "a.d",
        0x8000_1000,
        "entry",
        &[
            ("fingerprint", "{ calls = [\"a.b\", \"_Znwm\"], strings = [\"x y\"], size = [1, 9] }"),
            ("require", "false"),
        ],
    );
    let sites = read(&table(&[&a, &b, &c]), None).unwrap();
    let (a, b, c) = (sites[0].code().unwrap(), sites[1].code().unwrap(), sites[2].code().unwrap());
    let p = a.point.as_ref().unwrap();
    assert_eq!(a.offset, None);
    assert_eq!((&p.start, p.ordinal, p.shift), (&Some(End::Site("a.c".into())), Some(0), -1));
    assert!(matches!(&p.kind, AnchorKind::Words { at: 1, .. }));
    assert!(a.needs_lift() && !b.needs_lift() && c.needs_lift());
    assert_eq!(
        (b.inside.as_deref(), b.offset, b.signature.is_none(), b.signature_offset),
        (Some("a.b"), Some(4), true, 0)
    );
    let fp = c.fingerprint.as_ref().unwrap();
    assert_eq!(fp.calls, ["a.b", "_Znwm"]);
    assert_eq!((fp.strings.as_slice(), fp.size), (&["x y".to_string()][..], Some((1, 9))));
    assert!(!sites[2].require);
    assert!(p.describe().contains("number 0 of the words d503201f d65f03c0 (word 1) after a.c, -1 instruction"));
}

#[test]
fn each_rule_of_points_ins_and_fingerprints() {
    let s = |keys: &[(&str, &str)]| one("test.site", 0x8000_1000, "before", keys);
    let e = |keys: &[(&str, &str)]| one("test.site", 0x8000_1000, "entry", keys);
    let cases: Vec<(String, Vec<&str>)> = vec![
        (s(&[("offset", "0")]), vec!["signature, in or fingerprint"]),
        (s(&[("signature", SIG_F), ("offset", "0"), ("in", "\"x.y\"")]), vec!["in names the function"]),
        (s(&[("in", "\"test.site\""), ("offset", "0")]), vec!["another site's name"]),
        (s(&[("in", "\"x.y\""), ("offset", "0")]), vec!["names the site x.y"]),
        (s(&[("fingerprint", "{ calls = [] }"), ("offset", "0")]), vec!["calls, strings or words"]),
        (s(&[("fingerprint", "{ calls = [\"a\"], size = [3, 1] }"), ("offset", "0")]), vec!["size"]),
        (s(&[("fingerprint", "{ calls = [\"a\"], arity = 2 }"), ("offset", "0")]), vec!["unknown key", "arity"]),
        (s(&[("signature", SIG_F), ("offset", "0"), ("point", "{ vcall = 8 }")]), vec!["not both"]),
        (s(&[("signature", SIG_F)]), vec!["offset"]),
        (e(&[("signature", SIG_F), ("point", "{ vcall = 8 }")]), vec!["point is for"]),
        (s(&[("signature", SIG_F), ("point", "{ vcall = 8, call = \"f\" }")]), vec!["one of words"]),
        (s(&[("signature", SIG_F), ("point", "{ vcall = 12 }")]), vec!["multiple of 8"]),
        (s(&[("signature", SIG_F), ("point", "{ words = \"d503201f\", at = 1 }")]), vec!["at:"]),
        (s(&[("signature", SIG_F), ("point", "{ call = \"f_80001000\", at = 0 }")]), vec!["at is for words"]),
        (s(&[("signature", SIG_F), ("point", "{ call = \"a b\" }")]), vec!["call:"]),
        (s(&[("signature", SIG_F), ("point", "{ string = \"\" }")]), vec!["string:"]),
        (
            s(&[("signature", SIG_F), ("point", "{ vcall = 8, from = { vcall = 8, to = \"x.y\" } }")]),
            vec!["no from or to of its own"],
        ),
        (s(&[("signature", SIG_F), ("point", "{ vcall = 8, ordinal = -1 }")]), vec!["ordinal"]),
        (s(&[("signature", SIG_F), ("point", "{ vcall = 8, of = 2 }")]), vec!["of is for an ordinal"]),
        (
            s(&[("signature", SIG_F), ("point", "{ vcall = 8, ordinal = 2, of = 2 }")]),
            vec!["of: how many matches", "more than the ordinal"],
        ),
        (s(&[("signature", SIG_F), ("point", "{ vcall = 8, shift = 1.5 }")]), vec!["shift"]),
        (s(&[("signature", SIG_F), ("point", "{ vcall = 8, from = \"x.y\" }")]), vec!["names the site x.y"]),
        (
            s(&[("in", "\"x.y\""), ("offset", "0"), ("signature_offset", "4")]),
            vec!["signature_offset is for a signature"],
        ),
        (s(&[("signature", SIG_F), ("offset", "0"), ("require", "1")]), vec!["require"]),
    ];
    for (text, words) in cases {
        let e = read(&table(&[&text]), None).expect_err(&text).to_string();
        for w in words {
            assert!(e.contains(w), "{w:?} not in {e}");
        }
    }
}

const DEFS: &str = r#"format = 2
[[site]]
name = "a.one"
doc = "d"
at = "after"
shape = "event"
thread = "worker"
args = ["x19:ptr:obj", "w9:u32"]

[[site]]
name = "a.two"
doc = "d"
shape = "event"
thread = "worker"
require = false
"#;

fn locs() -> String {
    format!(
        "format = 2\nbuild = \"{BUILD}\"\n[[site]]\nname = \"a.one\"\nfunction = \"f_80001000\"\noffset = 8\n\
         signature = \"d10043ff\"\n"
    )
}

fn joined(defs: &str, locs: &str) -> Result<Vec<Site>, TableError> {
    join(&parse_definitions(defs, "d.toml")?, &parse_locations(locs, "l.toml", Some(BUILD))?, "d.toml", "l.toml", None)
}

#[test]
fn definitions_and_locations_join() {
    let s = joined(DEFS, &locs()).unwrap();
    assert_eq!(s.len(), 1, "a.two is not required: this build leaves it out");
    let c = s[0].code().unwrap();
    assert_eq!((s[0].name.as_str(), c.form, c.offset), ("a.one", Form::After, Some(8)));
    let two = locs() + "[[site]]\nname = \"a.two\"\nfunction = \"f_80002000\"\nsignature = \"d503201f\"\n";
    let names: Vec<String> = joined(DEFS, &two).unwrap().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["a.one", "a.two"], "the definitions' order");
    let moved = locs() + "args = [\"x20:ptr:obj\", \"w8:u32\"]\n";
    let args: Vec<String> = joined(DEFS, &moved).unwrap()[0].args.iter().map(ToString::to_string).collect();
    assert_eq!(args, ["x20:ptr:obj", "w8:u32"], "the same arguments in this build's registers");
}

#[test]
fn each_rule_of_the_split() {
    let bad = |defs: &str, locs: &str, what: &str| {
        let e = joined(defs, locs).unwrap_err().to_string();
        assert!(e.contains(what), "{what:?} not in {e}");
        e
    };
    let none = format!("format = 2\nbuild = \"{BUILD}\"\n");
    assert!(bad(DEFS, &none, "not located in this build").contains("l.toml"));
    let three = locs() + "[[site]]\nname = \"a.three\"\nfunction = \"f_80001000\"\nsignature = \"d10043ff\"\n";
    bad(DEFS, &three, "does not define it");
    for args in ["[\"x20:ptr:other\", \"w8:u32\"]", "[\"x20:u64:obj\", \"w8:u32\"]", "[\"x20:ptr:obj\"]"] {
        bad(DEFS, &(locs() + &format!("args = {args}\n")), "args: the definition's arguments");
    }
    let e = parse_definitions(&(DEFS.to_string() + "offset = 4\n"), "d.toml").unwrap_err().to_string();
    assert!(e.contains("its locations.toml"), "{e}");
    let e = parse_locations(&(locs() + "shape = \"data\"\n"), "l.toml", Some(BUILD)).unwrap_err().to_string();
    assert!(e.contains("game/sites/sites.toml"), "{e}");
    let e = parse_definitions(&(DEFS.to_string() + "[[site]]\nname = \"a.one\"\n"), "d.toml").unwrap_err().to_string();
    assert!(e.contains("defined twice"), "{e}");
    assert!(parse_definitions(&DEFS.replace("format = 2", "format = 1"), "d.toml").is_err());
    assert!(parse_locations(&locs().replace(BUILD, &format!("e{}", &BUILD[1..])), "l.toml", Some(BUILD)).is_err());
}
