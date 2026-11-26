// The lifter's key's sources (build.rs's LIFTER_SOURCES), included by build.rs and by the test that checks the list
// against lift's dependencies (src/tests.rs: a crate lift depends on that is in neither list fails it).

/// The crates whose sources shape the lifted code: all of each one's src/.
const CRATES: &[&str] = &["a64", "cfg", "sem", "mir", "passes", "llvm", "profile", "lift"];

/// Single files of other crates: the image as the lift reads it (image's lib.rs), and the edits' translation into mid IR
/// (code.rs: statements and wrappers). (What the edits are, each site's code and each patch's, is in each function's own
/// key, FunctionEdits::key: one patch changed is not a new lifter.)
const FILES: &[&str] = &["image/src/lib.rs", "edits/src/code.rs"];

/// The crates lift depends on that are not in the lifter's key, and why.
#[allow(dead_code)]
const NOT_IN_KEY: &[(&str, &str)] = &[
    ("edits", "EDITS_SOURCES, in the whole lift's key, and each function's edits in its own (code.rs is in FILES)"),
    ("sites", "EDITS_SOURCES, in the whole lift's key, and each function's edits in its own"),
    ("build-core", "the keys' hashing and the reports: neither is in the lifted code"),
    ("image", "its reader, lib.rs, is in FILES; its build (build.rs) makes image.bin and image.json, the lift's input, not its code"),
    ("game-files", "the image's build reads the exefs and the patches with it: the lift's input, not its code"),
];
