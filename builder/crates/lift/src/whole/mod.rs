//! The whole lift: from the game's image to every file of `gen/` the compile builds, with nothing of the old lifter.
//!
//! [`run`] discovers the functions (`cfg`), places the hook sites (`sites`), lifts every function (this crate, the
//! passes, the LLVM IR writer) into units cut by compile cost ([`units`]), writes the function table, the boot
//! constants, the import stubs and the runtime ([`files`]), and keeps a key of everything that made them, so an
//! unchanged lift is skipped and a changed one rewrites only the units it changed. The gen/ folder's contract is in
//! docs/dev/reference/builder.md ("The gen/ folder").
//!
//! The game's edits (the hook sites, the supported build's patches, the entry hooks: the `edits` crate) are written into
//! each function as it is built ([`edits::Edits`]), and each function's key holds its edits' key. An instruction the
//! semantics do not cover is a trap at its address ([`crate::Lifted::unsupported`]): the guest stops there if it
//! reaches it, the lift goes on, and its log counts those forms. An edit that cannot be written stops the lift.

pub mod edits;
pub mod files;
pub mod host;
pub mod store;
pub mod units;

use crate::{Program, Skip};
use build_core::key::Key;
use build_core::report::{Reporter, Stage};
use build_core::{Error, IoContext, Result, fsx};
use edits::Edits;
use host::Host;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use units::{Listed, Sizing, Unit};

/// The lift's key file in gen/: the key, then a line per count (written last: a lift that stopped has none).
pub const KEY_FILE: &str = "lift.key";
/// The units' list in gen/ ([`units::list_text`]).
pub const UNITS_FILE: &str = "units.txt";
/// The changed functions in gen/ ([`edits::Edits::list_text`]): `<entry> <edits' key> <what changes it>` a line.
pub const EDITS_FILE: &str = "edits.txt";

/// What the guest profile does in this lift.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Profiling {
    /// None: no counts, no hot units.
    #[default]
    Off,
    /// Every unit counts its blocks and branches (`profile_map.txt` says what each counter is).
    Generate,
    /// Branch weights and entry counts from this profile, its never-run functions `cold`, and the functions that ran
    /// most, together `share` of everything it counted (at most `max`), in hot units.
    Use {
        /// The profile's file.
        path: PathBuf,
        /// The hot functions' share of the counted runs (0.95).
        share: f64,
        /// At most this many hot functions (4000).
        max: usize,
    },
}

/// What a lift is asked to do.
#[derive(Clone, Debug)]
pub struct LiftOptions {
    /// The image's folder (`image.bin`, `image.json`).
    pub image: PathBuf,
    /// The folder the lift writes (gen/).
    pub gen_dir: PathBuf,
    /// The host's sources (`host/`): the HLE functions it defines and its entry hooks. None: neither.
    pub host: Option<PathBuf>,
    /// The site tables (`game`). None: no sites.
    pub tables: Option<PathBuf>,
    /// `main.npdm`, for the boot constants (`image.c`). None: no `image.c`.
    pub npdm: Option<PathBuf>,
    /// More entries (addresses a log said the game reached, say).
    pub entries: BTreeSet<u64>,
    /// BLRs called directly when they go to their usual target: BLR address to target.
    pub devirt: BTreeMap<u64, u64>,
    /// The guest profile.
    pub profiling: Profiling,
    /// What the IR asks of the build.
    pub codegen: llvm::Options,
    /// Every store keeps its own page mark (no marks shared).
    pub mark_each: bool,
    /// The guest state's reads and writes left as the semantics made them.
    pub no_simplify: bool,
    /// Which functions are split into pieces for the compiler, and how ([`passes::split`]).
    pub split: passes::SplitOptions,
    /// How big units are.
    pub sizing: Sizing,
    /// Workers lifting units side by side.
    pub jobs: usize,
    /// A folder of lifts kept for other work folders (by key); None: none.
    pub store: Option<PathBuf>,
    /// Lift again even when the key says nothing changed.
    pub force: bool,
    /// The units' texts written to gen/ (`lifted_*.ll`, about 5.7 GB on the game): the lift's own command, a
    /// developer reading or diffing them, an instrumented lift (its profile map is made as they are written). Off, the
    /// lift lists each unit with its key and the compile has its text made as it needs it ([`Texts`]).
    pub keep_text: bool,
}

impl LiftOptions {
    /// The options of a lift of the image in `image` into `gen`, everything else at its default.
    pub fn new(image: impl Into<PathBuf>, gen_dir: impl Into<PathBuf>) -> LiftOptions {
        LiftOptions {
            image: image.into(),
            gen_dir: gen_dir.into(),
            host: None,
            tables: None,
            npdm: None,
            entries: BTreeSet::new(),
            devirt: BTreeMap::new(),
            profiling: Profiling::Off,
            codegen: llvm::Options::default(),
            mark_each: false,
            no_simplify: false,
            split: passes::SplitOptions::default(),
            sizing: Sizing::default(),
            jobs: build_core::jobs::default_jobs(),
            store: None,
            force: false,
            keep_text: true,
        }
    }

    /// Whether the units' texts are written to gen/: asked for, or an instrumented lift.
    pub fn keeps_text(&self) -> bool {
        self.keep_text || matches!(self.profiling, Profiling::Generate)
    }
}

/// How the lift's files came to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// Already there: the key had not changed.
    Unchanged,
    /// Taken from the store.
    Restored,
    /// Lifted now (some units may have been kept).
    Lifted,
}

/// What a lift did.
#[derive(Clone, Debug)]
pub struct LiftOutcome {
    /// Its key.
    pub key: String,
    /// How its files came to be.
    pub how: How,
    /// The functions discovered (every one is in the units).
    pub functions: usize,
    /// The functions an edit changes.
    pub edited: usize,
    /// The units, as `units.txt` lists them.
    pub units: Vec<Listed>,
    /// How many of them were lifted now (the others kept from the last lift, by their keys).
    pub units_lifted: usize,
    /// How many unit files were written (a unit lifted again to the same text is left as it was).
    pub units_written: usize,
    /// How long it took.
    pub seconds: f64,
}

/// The key of the lifter itself: a hash of the sources of the crates that shape the lifted code and of the runtime, of
/// the compiler that built them (`rustc -V`) and of the Cargo.lock entries of the registry crates they are built with
/// (build.rs).
const LIFTER: &str = env!("LIFTER_SOURCES");

/// The key of the edits' data compiled into the lifter (the patches, the sites' placement and code: build.rs). A change
/// of it is a lift to do, whose functions' own keys say which units it changes.
const EDITS: &str = env!("EDITS_SOURCES");

/// The files of the site tables a lift of the build `build_id` reads, into `k`.
fn tables_key(k: &mut Key, tables: &Path, build_id: &str) -> Result<()> {
    let t = sites::Tables::new(tables);
    let id: String = build_id.to_ascii_lowercase().chars().take(40).collect();
    for path in [t.definitions(), t.locations(&id), t.host_list()] {
        if path.is_file() {
            k.file(tables, &path)?;
        } else {
            k.text("absent", &path.strip_prefix(tables).unwrap_or(&path).to_string_lossy());
        }
    }
    Ok(())
}

/// The options that change every function's IR.
fn codegen_text(o: &LiftOptions) -> String {
    format!("{:?} mark_each={} no_simplify={} split={:?}", o.codegen, o.mark_each, o.no_simplify, o.split)
}

/// The key of everything a lift's output is made of, read before anything is lifted.
fn whole_key(o: &LiftOptions, image: &image::Image, host: &Host, base: &str) -> Result<String> {
    let mut k = Key::new();
    k.text("base", base).text("host", &host.key_text());
    for name in ["image.bin", "image.json"] {
        k.file(&o.image, &o.image.join(name))?;
    }
    k.text("plan", &format!("{:?} {:?} {:?}", o.sizing, o.entries, o.devirt));
    if let Some(t) = &o.tables {
        let build = image.module("main").map(|m| m.build_id.clone()).unwrap_or_default();
        tables_key(&mut k, t, &build)?;
    }
    match &o.npdm {
        Some(p) => k.part("npdm", &std::fs::read(p).at(p)?),
        None => k.text("npdm", "none"),
    };
    k.text("edits", EDITS);
    match &o.profiling {
        Profiling::Off => k.text("profile", "off"),
        Profiling::Generate => k.text("profile", "generate"),
        Profiling::Use { path, share, max } => {
            k.part("profile", &std::fs::read(path).at(path)?).text("hot", &format!("{share} {max}"))
        }
    };
    Ok(k.hex())
}

/// The key every unit's starts from: the lifter, the options that change every function's IR and whether the units
/// are instrumented (not the image: each function's key holds what of it the function reads; nor a profile used:
/// each function's key holds its counts).
fn base_key(o: &LiftOptions) -> String {
    let mut k = Key::new();
    k.text("lifter", LIFTER).text("options", &codegen_text(o));
    // (an instrumented unit counts into `@lsp.counts`: a plain lift into this gen/ keeps neither it nor its object)
    k.text("instrumented", if matches!(o.profiling, Profiling::Generate) { "yes" } else { "no" });
    k.hex()
}

/// The gen/ files a lift writes besides the units.
const OTHER_FILES: &[&str] = &[
    "table.c",
    "imports.c",
    "functions.txt",
    UNITS_FILE,
    EDITS_FILE,
    "rt/a64.h",
    "rt/a64_config.h",
    "rt/a64_rt.c",
    "rt/a64_fp.c",
];

/// Whether gen/ holds the lift of `key`, whole: its key file says so and every file it lists is there, the units at
/// their sizes (with `texts`; without, the units listed, their texts made when needed).
fn unchanged(gen_dir: &Path, key: &str, texts: bool) -> bool {
    let Ok(text) = std::fs::read_to_string(gen_dir.join(KEY_FILE)) else { return false };
    if text.lines().next() != Some(key) {
        return false;
    }
    let Ok(list) = std::fs::read_to_string(gen_dir.join(UNITS_FILE)) else { return false };
    OTHER_FILES.iter().all(|f| gen_dir.join(f).is_file())
        && units::parse_list(&list).values().all(|u| {
            !texts
                || (!u.is_pending()
                    && std::fs::metadata(gen_dir.join(format!("{}.ll", u.name))).is_ok_and(|m| m.len() == u.bytes))
        })
}

/// The outcome a key file and units list describe (a lift skipped or restored).
fn described(gen_dir: &Path, key: String, how: How, t0: Instant) -> LiftOutcome {
    let text = std::fs::read_to_string(gen_dir.join(KEY_FILE)).unwrap_or_default();
    let count = |what: &str| text.lines().find_map(|l| l.strip_prefix(what)?.parse().ok()).unwrap_or(0);
    let (functions, edited) = (count("functions "), count("edited "));
    let list = std::fs::read_to_string(gen_dir.join(UNITS_FILE)).unwrap_or_default();
    let units = units::parse_list(&list).into_values().collect();
    LiftOutcome {
        key,
        how,
        functions,
        edited,
        units,
        units_lifted: 0,
        units_written: 0,
        seconds: t0.elapsed().as_secs_f64(),
    }
}

/// The hex entries listed in a file, one a line (`#` comments and blank lines left out).
pub fn read_entries(path: &Path) -> Result<BTreeSet<u64>> {
    let text = std::fs::read_to_string(path).at(path)?;
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let w = l.split_whitespace().next().unwrap_or(l);
            u64::from_str_radix(w.trim_start_matches("0x"), 16)
                .map_err(|_| Error::format(path.display().to_string(), format!("not a hex address: {l:?}")))
        })
        .collect()
}

/// The pairs of hex addresses listed in a file, two a line (a BLR and its usual target).
pub fn read_pairs(path: &Path) -> Result<BTreeMap<u64, u64>> {
    let text = std::fs::read_to_string(path).at(path)?;
    let mut out = BTreeMap::new();
    for l in text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty()) {
        let hex = |w: &str| u64::from_str_radix(w.trim_start_matches("0x"), 16).ok();
        let mut it = l.split_whitespace();
        match (it.next().and_then(hex), it.next().and_then(hex)) {
            (Some(a), Some(b)) => out.insert(a, b),
            _ => return Err(Error::format(path.display().to_string(), format!("not two hex addresses: {l:?}"))),
        };
    }
    Ok(out)
}

/// The discovery of `image`, with the extra entries and devirtualised calls of `o`.
pub fn discover(o: &LiftOptions, image: &image::Image) -> Result<cfg::plan::Plan> {
    let mut inputs = cfg::entries::inputs(image).map_err(|e| Error::format("the image's modules", e.to_string()))?;
    inputs.entries.extend(o.entries.iter().copied().filter(|&e| image.in_code(e)));
    let mut d = cfg::Discovery::new(image, inputs);
    d.run();
    let mut plan = d.plan(&o.image.join("image.bin"));
    plan.devirt = o.devirt.clone();
    Ok(plan)
}

/// The lift stops, before anything is lifted, at a setjmp owner with more setjmp calls than a host setjmp slot holds
/// (each call of it would be refused as its unit is lifted, [`Skip::SetjmpReturns`]).
fn check_setjmp_owners(plan: &cfg::plan::Plan) -> Result<()> {
    match plan.setjmp_owners.iter().find(|(_, rets)| rets.len() > mir::SETJMP_RETS) {
        Some((&owner, rets)) => Err(Error::format("the lift", Skip::setjmp_returns_text(owner, rets.len()))),
        None => Ok(()),
    }
}

/// The edits of a lift: the sites placed (with `o.tables`), the host's entry hooks, the patches for the image's build.
pub fn edits(o: &LiftOptions, image: &image::Image, host: &Host, plan: &cfg::plan::Plan) -> Result<Edits> {
    let placement = match &o.tables {
        Some(t) => match sites::apply(&sites::Tables::new(t), image, plan) {
            Ok(found) => found.map(|(_, p)| p),
            Err(e) => return Err(Error::format("the hook sites", e.to_string())),
        },
        None => None,
    };
    Edits::new(image, plan, placement, host)
}

/// One unit's text, made.
struct Made {
    text: String,
    /// Each function's share of the IR, in bytes (its compile cost: [`units::compile_cost`]).
    sizes: Vec<u64>,
    functions: usize,
    counted: Vec<profile::Counted>,
    marks: passes::MarkStats,
    simplified: passes::SimplifyStats,
    /// The functions split, and the pieces they were split into.
    split: usize,
    pieces: usize,
    /// How many times each form lowering does not do is in its code ([`crate::Lifted::unsupported`])
    unsupported: BTreeMap<&'static str, usize>,
}

impl Made {
    /// The unit's line in `units.txt`.
    fn listed(&self, unit: &Unit) -> Listed {
        Listed {
            name: unit.name.clone(),
            cost: units::compile_cost(&self.sizes),
            bytes: self.text.len() as u64,
            key: unit.key.clone(),
            content: build_core::key::sha1_hex(self.text.as_bytes()),
            functions: self.functions,
        }
    }
}

/// Everything a unit's text is made from, worked out once: the image, its discovery and edits, the profile's counts,
/// and the units planned with their keys.
pub struct Session {
    o: LiftOptions,
    image: image::Image,
    plan: cfg::plan::Plan,
    edits: Edits,
    /// The game's main module's build (the profile's never-run functions are only said for it).
    build: String,
    profile: Option<profile::Profile>,
    hashes: BTreeMap<u64, u64>,
    planned: Vec<Unit>,
    /// The whole lift's key.
    key: String,
}

impl Session {
    /// Discovery, the edits, the profile and the units' plan, for the lift of `key` (`r` hears what was found).
    fn new(
        o: &LiftOptions,
        image: image::Image,
        host: &Host,
        base: String,
        key: String,
        r: &Reporter,
    ) -> Result<Session> {
        let t0 = Instant::now();
        let plan = discover(o, &image)?;
        check_setjmp_owners(&plan)?;
        r.log(format!(
            "lift: {} functions, {} instructions, {} jump tables ({:.1} s)",
            plan.functions.len(),
            plan.functions.values().map(cfg::plan::Function::len).sum::<usize>(),
            plan.jump_tables.len(),
            t0.elapsed().as_secs_f64()
        ));
        let edits = edits(o, &image, host, &plan)?;
        if let Some(p) = &edits.placement {
            r.log(format!("lift: {} hook sites placed", p.sites.len()));
        }
        for n in &edits.notes {
            r.log(format!("lift: {n}"));
        }
        r.log(format!("lift: {} functions changed by the edits", edits.functions.len()));
        let build = image.module("main").map(|m| m.build_id.clone()).unwrap_or_default();
        let hashes: BTreeMap<u64, u64> = plan
            .functions
            .iter()
            .map(|(&e, f)| (e, profile::code_hash(f.pcs().map(|pc| image.insn(pc).unwrap_or(0)))))
            .collect();
        let (profile, hot) = match &o.profiling {
            Profiling::Use { path, share, max } => {
                let p = profile::Profile::load(path).map_err(|e| Error::format("the guest profile", e.to_string()))?;
                let hot = hot_functions(&p, &plan, &|e| hashes[&e], *share, *max);
                (Some(p), hot)
            }
            _ => (None, BTreeSet::new()),
        };
        let mut s = Session { o: o.clone(), image, plan, edits, build, profile, hashes, planned: Vec::new(), key };
        let program = s.program();
        let key_of = |entry: u64| -> String {
            let r = s.runs(entry);
            let text = format!("{}\n{}\n{} {:?}", program.inputs(entry), s.edits.key(entry), r.cold, r.counts);
            build_core::key::sha1_hex(text.as_bytes())
        };
        let list: Vec<(u64, u64)> = s.plan.functions.iter().map(|(&e, f)| (e, f.len() as u64)).collect();
        let cold: BTreeSet<u64> = list.iter().map(|&(e, _)| e).filter(|&e| s.runs(e).cold).collect();
        // (each function's key on the lift's workers: what its lift reads, written out and hashed, for all 294,518
        // functions of the game, was most of an unchanged-unit lift's time on one thread)
        let entries: Vec<u64> = list.iter().map(|&(e, _)| e).collect();
        let per = entries.len().div_ceil(o.jobs.max(1)).max(1);
        let keys: BTreeMap<u64, String> = std::thread::scope(|sc| {
            let parts: Vec<_> = entries
                .chunks(per)
                .map(|c| sc.spawn(|| c.iter().map(|&e| (e, key_of(e))).collect::<Vec<_>>()))
                .collect();
            parts.into_iter().flat_map(|h| h.join().expect("a key worker panicked")).collect()
        });
        let planned = units::plan(&list, &hot, &cold, o.sizing, &base, &|e| keys[&e].clone());
        s.planned = planned;
        Ok(s)
    }

    /// The lift's key.
    pub fn key(&self) -> &str {
        &self.key
    }

    fn program(&self) -> Program<'_> {
        Program::new(&self.image, &self.plan, &self.edits.functions)
    }

    /// What the profile says of the function at `entry`.
    fn runs(&self, entry: u64) -> llvm::Runs<'_> {
        let hash = self.hashes[&entry];
        match &self.profile {
            Some(p) => {
                llvm::Runs { hash, counts: p.function(entry, hash), cold: p.never_ran(&self.build, entry, hash) }
            }
            None => llvm::Runs { hash, ..llvm::Runs::default() },
        }
    }

    fn instrument(&self) -> bool {
        matches!(self.o.profiling, Profiling::Generate)
    }

    /// One unit's text.
    fn make(&self, unit: &Unit) -> Result<Made> {
        let program = self.program();
        let mut m = if self.instrument() {
            llvm::Module::instrumented(self.o.codegen, &unit.name)
        } else {
            llvm::Module::new(self.o.codegen)
        };
        // (its file's name inside: an object's file symbol is the same whether clang read the file or the text)
        m.set_source_filename(&format!("{}.ll", unit.name));
        let mut sizes = Vec::new();
        let (mut marks, mut simplified) = (passes::MarkStats::default(), passes::SimplifyStats::default());
        let (mut split, mut pieces) = (0usize, 0usize);
        let mut unsupported: BTreeMap<&'static str, usize> = BTreeMap::new();
        for &entry in &unit.functions {
            let mut lifted = match program.lift(entry) {
                Ok(l) => l,
                Err(Skip::NoFunction) => unreachable!("a discovered function"),
                Err(Skip::Edit { pc, what }) => {
                    return Err(Error::format(format!("f_{entry:08x}"), format!("an edit at {pc:#x}: {what}")));
                }
                Err(Skip::SetjmpReturns { owner, count }) => {
                    return Err(Error::format(format!("f_{entry:08x}"), Skip::setjmp_returns_text(owner, count)));
                }
            };
            for &form in &lifted.unsupported {
                *unsupported.entry(form).or_default() += 1;
            }
            for f in lifted.functions_mut() {
                if !self.o.mark_each {
                    marks += passes::share_marks(f);
                }
                if !self.o.no_simplify {
                    simplified += passes::simplify(f);
                }
                f.verify().map_err(|e| Error::format(format!("the IR of {}", f.name()), e.to_string()))?;
            }
            let before = m.size();
            let runs = self.runs(entry);
            let mut body = passes::split(lifted.body, self.o.split);
            if body.len() > 1 {
                split += 1;
                pieces += body.len() - 1;
                for p in &body {
                    p.verify().map_err(|e| Error::format(format!("the IR of {}", p.name()), e.to_string()))?;
                }
                mir::verify_pieces(&body)
                    .map_err(|e| Error::format(format!("the pieces of {}", e.function), e.to_string()))?;
            }
            // (a split function: its pieces first, then the trampoline of its name)
            body.rotate_left(1);
            for f in &body {
                m.add(f, runs);
            }
            if let Some(w) = &lifted.wrapper {
                // (the wrapper of a body: f_<entry>, in the body's unit; a profile counts the body)
                m.add(w, llvm::Runs { hash: runs.hash, ..llvm::Runs::default() });
            }
            sizes.push((m.size() - before) as u64);
        }
        let counted = m.counted().to_vec();
        let functions = m.len();
        let text = m.finish();
        Ok(Made { text, sizes, functions, counted, marks, simplified, split, pieces, unsupported })
    }

    /// The text of the unit whose file is `name` (`lifted_*.ll`), made now, and its line in `units.txt`.
    pub fn unit_text(&self, name: &str) -> Result<(String, Listed)> {
        let stem = name.strip_suffix(".ll").unwrap_or(name);
        let unit = self
            .planned
            .iter()
            .find(|u| u.name == stem)
            .ok_or_else(|| Error::format("the lift", format!("{name} is not a unit of this lift: lift again")))?;
        let made = self.make(unit)?;
        let listed = made.listed(unit);
        Ok((made.text, listed))
    }
}

/// The units' texts a lift did not write ([`LiftOptions::keep_text`] off), made when the compile asks for one: from the
/// lift's [`Session`] when it ran, else from a session worked out again at the first ask (an unchanged lift's units
/// whose objects the store no longer holds). What was made is noted in `units.txt` by [`Texts::finish`], so the next
/// build knows each unit's content key without its text.
pub struct Texts {
    o: LiftOptions,
    session: OnceLock<std::result::Result<Arc<Session>, String>>,
    made: Mutex<BTreeMap<String, Listed>>,
}

impl Texts {
    /// The texts of the lift `o` asks for (`session`: the one it ran, if it did).
    pub fn new(o: LiftOptions, session: Option<Arc<Session>>) -> Texts {
        let once = OnceLock::new();
        if let Some(s) = session {
            let _ = once.set(Ok(s));
        }
        Texts { o, session: once, made: Mutex::new(BTreeMap::new()) }
    }

    fn session(&self) -> Result<Arc<Session>> {
        let s = self.session.get_or_init(|| {
            let quiet = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
            let again = || -> Result<Session> {
                let image =
                    image::Image::load(&self.o.image).map_err(|e| Error::format("the game's image", e.to_string()))?;
                let host = match &self.o.host {
                    Some(h) => Host::read(h)?,
                    None => Host::default(),
                };
                let base = base_key(&self.o);
                let key = whole_key(&self.o, &image, &host, &base)?;
                Session::new(&self.o, image, &host, base, key, &quiet)
            };
            again().map(Arc::new).map_err(|e| e.to_string())
        });
        s.clone().map_err(|e| Error::format("the lift, made again for a unit's text", e))
    }

    /// The text of the unit whose file is `name`, made now (and noted for [`Texts::finish`]). A unit of another lift
    /// than the one gen/ holds now is an error: the lift is done again.
    pub fn text(&self, name: &str) -> Result<String> {
        let s = self.session()?;
        let listed_key = std::fs::read_to_string(self.o.gen_dir.join(KEY_FILE)).unwrap_or_default();
        if listed_key.lines().next() != Some(s.key()) {
            return Err(Error::format("the lift", "gen/ holds another lift than the one asked for: lift again"));
        }
        let (text, listed) = s.unit_text(name)?;
        self.made.lock().unwrap_or_else(|p| p.into_inner()).insert(listed.name.clone(), listed);
        Ok(text)
    }

    /// `units.txt` with the units made since noted: their content keys, sizes and costs (nothing when none were made).
    pub fn finish(&self) -> Result<()> {
        let made = std::mem::take(&mut *self.made.lock().unwrap_or_else(|p| p.into_inner()));
        if made.is_empty() {
            return Ok(());
        }
        let path = self.o.gen_dir.join(UNITS_FILE);
        let mut list = units::parse_list(&std::fs::read_to_string(&path).unwrap_or_default());
        for (name, l) in made {
            // (only where the list still names that unit with that key: a later lift's list is left as it is)
            if list.get(&name).is_some_and(|old| old.key == l.key) {
                list.insert(name, l);
            }
        }
        let all: Vec<Listed> = list.into_values().collect();
        fsx::write_if_changed(&path, units::list_text(&all).as_bytes()).map(|_| ())
    }
}

/// One unit's lift, written.
struct Done {
    listed: Listed,
    written: bool,
    counted: Vec<profile::Counted>,
    marks: passes::MarkStats,
    simplified: passes::SimplifyStats,
    /// The functions split, and the pieces they were split into.
    split: usize,
    pieces: usize,
    unsupported: BTreeMap<&'static str, usize>,
}

/// One unit lifted and its file written (unless the file there has its text already).
fn write_unit(s: &Session, previous: &BTreeMap<String, Listed>, unit: &Unit) -> Result<Done> {
    let made = s.make(unit)?;
    let listed = made.listed(unit);
    let path = s.o.gen_dir.join(format!("{}.ll", unit.name));
    // (the same text as the file there: left as it is, its time too, so the compile sees nothing new)
    let same = previous.get(&unit.name).is_some_and(|p| {
        p.content == listed.content && std::fs::metadata(&path).is_ok_and(|m| m.len() == listed.bytes)
    });
    if !same {
        fsx::write_atomic(&path, made.text.as_bytes())?;
    }
    let Made { counted, marks, simplified, split, pieces, unsupported, .. } = made;
    Ok(Done { listed, written: !same, counted, marks, simplified, split, pieces, unsupported })
}

/// The hot functions of a profile: those that ran most, hottest first, until they hold `share` of everything counted
/// (at most `max`).
fn hot_functions(
    profile: &profile::Profile,
    plan: &cfg::plan::Plan,
    hash_of: &dyn Fn(u64) -> u64,
    share: f64,
    max: usize,
) -> BTreeSet<u64> {
    let mut ranked: Vec<(u64, u64)> = plan
        .functions
        .keys()
        .filter_map(|&e| profile.function(e, hash_of(e)).map(|f| (f.weight(), e)))
        .filter(|&(w, _)| w > 0)
        .collect();
    ranked.sort_by_key(|&(w, e)| (std::cmp::Reverse(w), e));
    let total: u64 = ranked.iter().map(|&(w, _)| w).sum();
    let (mut sum, mut hot) = (0u64, BTreeSet::new());
    for &(w, e) in &ranked {
        if hot.len() >= max || (sum as f64) >= share * total as f64 {
            break;
        }
        sum += w;
        hot.insert(e);
    }
    hot
}

/// The whole lift (see the module's docs). `r` hears its progress and log.
pub fn run(o: &LiftOptions, r: &Reporter) -> Result<LiftOutcome> {
    run_with(o, r).map(|(out, _)| out)
}

/// The whole lift, and its [`Session`] when it was worked out (a lift done now; not one unchanged or restored), for
/// [`Texts`].
pub fn run_with(o: &LiftOptions, r: &Reporter) -> Result<(LiftOutcome, Option<Arc<Session>>)> {
    let t0 = Instant::now();
    r.stage(Stage::Lift, "Translating the game's code");
    if !o.image.join("image.bin").is_file() || !o.image.join("image.json").is_file() {
        return Err(Error::player(
            "image_missing",
            format!("The game's image is missing from {} (image.bin, image.json).", o.image.display()),
        ));
    }
    let image = image::Image::load(&o.image).map_err(|e| Error::format("the game's image", e.to_string()))?;
    let host = match &o.host {
        Some(h) => Host::read(h)?,
        None => Host::default(),
    };
    let base = base_key(o);
    let key = whole_key(o, &image, &host, &base)?;
    std::fs::create_dir_all(&o.gen_dir).at(&o.gen_dir)?;
    let keep_text = o.keeps_text();
    if !o.force && unchanged(&o.gen_dir, &key, keep_text) {
        if !keep_text {
            remove_unit_files(&o.gen_dir, &BTreeSet::new())?;
        }
        r.log(format!("lift: unchanged (key {}), {:.1} s", &key[..12], t0.elapsed().as_secs_f64()));
        return Ok((described(&o.gen_dir, key, How::Unchanged, t0), None));
    }
    // (the lift of this key marked in use in the store until the lift is done: another build's prune leaves it)
    let _in_use = o.store.as_deref().map(|s| store::in_use(s, &key)).transpose()?;
    if !o.force
        && let Some(store) = &o.store
        && store::restore(store, &key, &o.gen_dir)?
        && unchanged(&o.gen_dir, &key, keep_text)
    {
        r.log(format!("lift: taken from the store (key {})", &key[..12]));
        return Ok((described(&o.gen_dir, key, How::Restored, t0), None));
    }
    let _ = std::fs::remove_file(o.gen_dir.join(KEY_FILE)); // (written last again: a lift that stops leaves none)

    let session = Session::new(o, image, &host, base, key.clone(), r)?;
    let s = &session;
    let instrument = s.instrument();
    let planned = &s.planned;

    // (the units the last lift listed with these keys are kept; an instrumented lift writes every unit again, as its
    // profile map is made from the units' counters. Without the texts kept, a unit is made only when the compile asks
    // for its text: listed here with its key, its estimated cost and no content key yet)
    let previous = units::parse_list(&std::fs::read_to_string(o.gen_dir.join(UNITS_FILE)).unwrap_or_default());
    let mut kept: Vec<Listed> = Vec::new();
    let mut todo: Vec<&Unit> = Vec::new();
    for u in planned {
        let file = o.gen_dir.join(format!("{}.ll", u.name));
        let reuse = !instrument
            && previous.get(&u.name).is_some_and(|p| {
                p.key == u.key && (!keep_text || std::fs::metadata(&file).is_ok_and(|m| m.len() == p.bytes))
            });
        if reuse {
            kept.push(previous[&u.name].clone());
        } else if keep_text {
            todo.push(u);
        } else {
            kept.push(units::Listed::pending(u));
        }
    }
    todo.sort_by_key(|u| std::cmp::Reverse(u.cost));
    let names: BTreeSet<String> =
        if keep_text { planned.iter().map(|u| format!("{}.ll", u.name)).collect() } else { BTreeSet::new() };
    remove_unit_files(&o.gen_dir, &names)?;
    let pending = kept.iter().filter(|l| l.is_pending()).count();
    r.log(format!(
        "lift: {} units, {} kept from the last lift, {}{}",
        planned.len(),
        kept.len() - pending,
        if keep_text { format!("{} to write", todo.len()) } else { format!("{pending} new") },
        if keep_text { "" } else { " (their texts are made as the compile needs them, not kept)" }
    ));

    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let results: Mutex<Vec<Result<(String, Done)>>> = Mutex::new(Vec::new());
    let workers = o.jobs.max(1).min(todo.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(u) = todo.get(i) else { break };
                    let done = write_unit(s, &previous, u).map(|d| (u.name.clone(), d));
                    let failed = done.is_err();
                    results.lock().expect("a worker panicked").push(done);
                    let n = finished.fetch_add(1, Ordering::Relaxed) + 1;
                    r.progress(n as f64 / todo.len() as f64);
                    if failed {
                        next.store(todo.len(), Ordering::Relaxed); // (the others stop after their unit)
                    }
                }
            });
        }
    });
    let mut map: BTreeMap<String, Vec<profile::Counted>> = BTreeMap::new();
    let (mut marks, mut simplified) = (passes::MarkStats::default(), passes::SimplifyStats::default());
    let mut all = kept;
    let mut units_written = 0;
    let (mut split, mut pieces) = (0usize, 0usize);
    let mut unsupported: BTreeMap<&'static str, usize> = BTreeMap::new();
    for done in results.into_inner().expect("a worker panicked") {
        let (name, d) = done?;
        for (form, n) in &d.unsupported {
            *unsupported.entry(form).or_default() += n;
        }
        marks += d.marks;
        simplified += d.simplified;
        units_written += usize::from(d.written);
        split += d.split;
        pieces += d.pieces;
        if instrument {
            map.insert(name, d.counted);
        }
        all.push(d.listed);
    }
    let units_lifted = todo.len();

    // (the other files: small, made from the plan and the image)
    let unit_of: BTreeMap<u64, &str> =
        planned.iter().flat_map(|u| u.functions.iter().map(move |&e| (e, u.name.as_str()))).collect();
    let functions_text = files::functions_txt(&s.plan, &|e| unit_of.get(&e).map(|s| s.to_string()));
    let (imports, stubs, nvn) = files::imports_c(&s.image, &host);
    let mut out: Vec<(String, String)> = vec![
        ("table.c".into(), files::table_c(&s.plan)),
        ("imports.c".into(), imports),
        ("functions.txt".into(), functions_text),
        (EDITS_FILE.into(), s.edits.list_text()),
        ("rt/a64_config.h".into(), files::config_h(&o.codegen)),
    ];
    out.extend(files::RUNTIME.iter().map(|(n, t)| (format!("rt/{n}"), (*t).to_string())));
    match &o.npdm {
        Some(p) => {
            let npdm =
                files::Npdm::parse(&std::fs::read(p).at(p)?).map_err(|e| Error::format(p.display().to_string(), e))?;
            out.push(("image.c".into(), files::image_c(&s.image, &npdm)));
        }
        None => {
            fsx::remove_file(&o.gen_dir.join("image.c"))?;
            r.warn("no_npdm", "No main.npdm was given: gen/image.c (the boot constants) is not written.");
        }
    }
    if instrument {
        out.push(("profile_map.txt".into(), profile::Map { build: s.build.clone(), units: map }.to_text()));
    } else {
        fsx::remove_file(&o.gen_dir.join("profile_map.txt"))?;
    }
    out.push((UNITS_FILE.into(), units::list_text(&all)));
    for (name, text) in &out {
        fsx::write_if_changed(&o.gen_dir.join(name), text.as_bytes())?;
    }
    let ir: u64 = all.iter().map(|u| u.bytes).sum();
    let largest = all.iter().map(|u| u.bytes).max().unwrap_or(0);
    r.log(format!(
        "lift: {} functions in {} units ({} MB of IR{}, the largest unit {} MB), {} of them edited; {stubs} import \
         stubs, {nvn} NVN functions routed",
        s.plan.functions.len(),
        all.len(),
        ir >> 20,
        if all.iter().any(Listed::is_pending) { ", some estimated" } else { "" },
        largest >> 20,
        s.edits.functions.len()
    ));
    if split > 0 {
        r.log(format!("lift: {split} functions too big for the compiler split into {pieces} pieces"));
    }
    if !unsupported.is_empty() {
        // (each a trap in the lifted code: the guest stops there if it runs it)
        let forms: Vec<String> = unsupported.iter().map(|(form, n)| format!("{n} {form}")).collect();
        r.log(format!(
            "lift: instructions lowering does not do, each a trap where it is (in the units lifted now): {}",
            forms.join(", ")
        ));
    }
    if units_lifted > 0 {
        r.log(format!(
            "lift: page marks shared by {} groups for {} of {} stores; simplified: {} reads forwarded, {} pure ops \
             shared, {} dead writes, {} dead values",
            marks.groups,
            marks.grouped,
            marks.stores,
            simplified.forwarded,
            simplified.shared,
            simplified.dead_writes,
            simplified.dead_values
        ));
    }
    let edited = s.edits.functions.len();
    let functions = s.plan.functions.len();
    let key_text = format!("{key}\nfunctions {functions}\nedited {edited}\nunits {}\n", all.len());
    fsx::write_atomic(&o.gen_dir.join(KEY_FILE), key_text.as_bytes())?;
    if let Some(store) = &o.store {
        store::save(store, &key, &o.gen_dir)?;
    }
    let seconds = t0.elapsed().as_secs_f64();
    r.log(format!("lift: done in {seconds:.1} s ({units_lifted} units lifted, {units_written} written)"));
    let outcome =
        LiftOutcome { key, how: How::Lifted, functions, edited, units: all, units_lifted, units_written, seconds };
    Ok((outcome, Some(Arc::new(session))))
}

/// The `lifted_*.ll` files in gen/ but those `keep` names, deleted.
fn remove_unit_files(gen_dir: &Path, keep: &BTreeSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(gen_dir).at(gen_dir)? {
        let name = entry.at(gen_dir)?.file_name().to_string_lossy().into_owned();
        if name.starts_with("lifted_") && name.ends_with(".ll") && !keep.contains(&name) {
            fsx::remove_file(&gen_dir.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
