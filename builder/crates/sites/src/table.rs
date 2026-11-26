//! The site tables (docs/dev/reference/hooks.md, the format): each site's definition (`game/sites/sites.toml`: what it
//! is and its callback's contract) joined with where it is in a build (`game/sites/<build id>/locations.toml`), and
//! every rule of the format checked before anything is placed. A fault stops with the file, the site and the rule,
//! in the words nx2/hooks.py uses.

use crate::signature::Signature;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use toml::{Table, Value};

/// A table that does not follow the format: the file, the site (when one is at fault) and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableError {
    /// The file (a repository path, as `game/sites/sites.toml`).
    pub place: String,
    /// The site: its name, or `#<n>` for one with none.
    pub site: Option<String>,
    /// What is wrong.
    pub what: String,
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.site {
            Some(site) => write!(f, "{}: site {site}: {}", self.place, self.what),
            None => write!(f, "{}: {}", self.place, self.what),
        }
    }
}

impl std::error::Error for TableError {}

fn whole_file(place: &str, what: impl Into<String>) -> TableError {
    TableError { place: place.into(), site: None, what: what.into() }
}

/// The format of the definitions and a build's locations (two files).
pub const FORMAT: i64 = 2;
/// The format of a table with both in one file (as the checks write them).
pub const ONE_FILE: i64 = 1;
/// The most places a fingerprint's most specific part may be found in.
pub const MOST_PLACES: usize = 256;

const DEF_KEYS: &[&str] = &[
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
const LOC_KEYS: &[&str] = &[
    "name",
    "function",
    "offset",
    "signature",
    "signature_offset",
    "guards",
    "to",
    "in",
    "point",
    "fingerprint",
    "args",
];
const VSLOT_LOC_KEYS: &[&str] = &["name", "function", "vtable", "slot", "args"];
const ANCHOR_KEYS: &[&str] = &["words", "call", "vcall", "string", "at", "from", "to", "ordinal", "of", "shift"];
const FINGERPRINT_KEYS: &[&str] = &["calls", "strings", "words", "size", "blocks"];
const MODULES: &[&str] = &["main"];

fn code_keys() -> BTreeSet<&'static str> {
    DEF_KEYS.iter().chain(LOC_KEYS).copied().collect()
}

fn vslot_keys() -> BTreeSet<&'static str> {
    DEF_KEYS
        .iter()
        .filter(|k| !matches!(**k, "at" | "can_return" | "rule" | "value"))
        .chain(VSLOT_LOC_KEYS)
        .copied()
        .collect()
}

/// A site's name: dot-separated words, each a lower-case letter then letters and digits (`text.letterStep`).
pub fn is_name(s: &str) -> bool {
    s.split('.').all(|w| {
        let mut c = w.chars();
        c.next().is_some_and(|c| c.is_ascii_lowercase()) && c.all(|c| c.is_ascii_alphanumeric())
    })
}

/// A site's C identifier: its name with each dot an underscore (`hos_site_on_<ident>`).
pub fn ident(name: &str) -> String {
    name.replace('.', "_")
}

/// A function's name in a build, `f_` and 8 lower-case hex digits, as its address.
pub fn function_address(s: &str) -> Option<u64> {
    let hex = s.strip_prefix("f_")?;
    (hex.len() == 8 && hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
        .then(|| u64::from_str_radix(hex, 16).ok())
        .flatten()
}

fn is_build(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// What a site's callback is: it tells (event), it may change the outcome (filter), or it is data a rule applies.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Event,
    Filter,
    Data,
}

/// Where a site is reached: on the game's serial thread, or a worker.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thread {
    Serial,
    Worker,
}

/// A site's form (`at`): where its hook goes and what its answer may do (docs/dev/reference/hooks.md, the forms).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Form {
    Entry,
    Return,
    After,
    Before,
    Replace,
    Branch,
    Allow,
    Jump,
}

impl Form {
    const ALL: [Form; 8] =
        [Form::Entry, Form::Return, Form::After, Form::Before, Form::Replace, Form::Branch, Form::Allow, Form::Jump];

    /// Its name in a table.
    pub fn name(self) -> &'static str {
        match self {
            Form::Entry => "entry",
            Form::Return => "return",
            Form::After => "after",
            Form::Before => "before",
            Form::Replace => "replace",
            Form::Branch => "branch",
            Form::Allow => "allow",
            Form::Jump => "jump",
        }
    }

    fn parse(s: &str) -> Option<Form> {
        Form::ALL.into_iter().find(|f| f.name() == s)
    }

    /// Whether it is at an instruction inside the function (an offset or a point), not at its entry or return.
    pub fn at_offset(self) -> bool {
        !matches!(self, Form::Entry | Form::Return)
    }

    /// Whether it changes the game's outcome (where control goes, or what an instruction does).
    pub fn changes_outcome(self) -> bool {
        matches!(self, Form::Branch | Form::Allow | Form::Jump | Form::Replace)
    }

    fn is_rule_form(self) -> bool {
        matches!(self, Form::Entry | Form::Return | Form::After | Form::Before)
    }
}

/// A register's class by its letter: an X or W general register, an S or D float.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegClass {
    X,
    W,
    S,
    D,
}

impl RegClass {
    /// Its letter.
    pub fn letter(self) -> char {
        match self {
            RegClass::X => 'x',
            RegClass::W => 'w',
            RegClass::S => 's',
            RegClass::D => 'd',
        }
    }

    fn of(c: char) -> Option<RegClass> {
        Some(match c {
            'x' => RegClass::X,
            'w' => RegClass::W,
            's' => RegClass::S,
            'd' => RegClass::D,
            _ => return None,
        })
    }

    /// The types a value of its width can be.
    pub fn types(self) -> &'static [ArgType] {
        match self {
            RegClass::X => &[ArgType::Ptr, ArgType::U64, ArgType::I64],
            RegClass::W => {
                &[ArgType::U32, ArgType::I32, ArgType::U16, ArgType::I16, ArgType::U8, ArgType::I8, ArgType::Bool]
            }
            RegClass::S => &[ArgType::F32],
            RegClass::D => &[ArgType::F64],
        }
    }

    fn highest(self) -> u8 {
        if matches!(self, RegClass::X | RegClass::W) { 30 } else { 31 }
    }
}

/// An argument's type, as a table names it.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgType {
    Ptr,
    U64,
    I64,
    U32,
    I32,
    U16,
    I16,
    U8,
    I8,
    Bool,
    F32,
    F64,
}

impl ArgType {
    const ALL: [ArgType; 12] = [
        ArgType::Ptr,
        ArgType::U64,
        ArgType::I64,
        ArgType::U32,
        ArgType::I32,
        ArgType::U16,
        ArgType::I16,
        ArgType::U8,
        ArgType::I8,
        ArgType::Bool,
        ArgType::F32,
        ArgType::F64,
    ];

    /// Its name in a table.
    pub fn name(self) -> &'static str {
        match self {
            ArgType::Ptr => "ptr",
            ArgType::U64 => "u64",
            ArgType::I64 => "i64",
            ArgType::U32 => "u32",
            ArgType::I32 => "i32",
            ArgType::U16 => "u16",
            ArgType::I16 => "i16",
            ArgType::U8 => "u8",
            ArgType::I8 => "i8",
            ArgType::Bool => "bool",
            ArgType::F32 => "f32",
            ArgType::F64 => "f64",
        }
    }

    fn parse(s: &str) -> Option<ArgType> {
        ArgType::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// One register a site's callback reads: `x19:ptr:anim` (its class and number, its type, perhaps its name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arg {
    /// The register's class.
    pub class: RegClass,
    /// Its number.
    pub reg: u8,
    /// The value's type.
    pub ty: ArgType,
    /// Its name, if it has one (a rule reads its arguments by name).
    pub name: Option<String>,
}

impl Arg {
    /// The argument `text` writes (`<register>:<type>[:<name>]`), or what is wrong with it.
    pub fn parse(text: &str) -> Result<Arg, String> {
        let bad = || format!("{text:?} is not an argument: <register>:<type>[:<name>], as x19:ptr:anim");
        let mut parts = text.split(':');
        let (reg, ty, name) = (parts.next().ok_or_else(bad)?, parts.next().ok_or_else(bad)?, parts.next());
        if parts.next().is_some() {
            return Err(bad());
        }
        let mut chars = reg.chars();
        let class = chars.next().and_then(RegClass::of).ok_or_else(bad)?;
        let digits = chars.as_str();
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        if ty.is_empty() || !ty.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(bad());
        }
        if let Some(n) = name {
            let mut c = n.chars();
            if !(c.next().is_some_and(|c| c.is_ascii_lowercase()) && c.all(|c| c.is_ascii_alphanumeric())) {
                return Err(bad());
            }
        }
        let n: u32 = digits.parse().map_err(|_| bad())?;
        if n > u32::from(class.highest()) {
            return Err(format!("{text:?}: no register {}{n}", class.letter()));
        }
        let ty = ArgType::parse(ty).filter(|t| class.types().contains(t)).ok_or_else(|| {
            let names: Vec<&str> = class.types().iter().map(|t| t.name()).collect();
            format!("{text:?}: a {} register holds {}", class.letter(), names.join(", "))
        })?;
        Ok(Arg { class, reg: n as u8, ty, name: name.map(String::from) })
    }
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}:{}", self.class.letter(), self.reg, self.ty.name())?;
        match &self.name {
            Some(n) => write!(f, ":{n}"),
            None => Ok(()),
        }
    }
}

/// What a site's callback returns: nothing, or a value of a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Returns {
    /// Nothing.
    Void,
    /// A value.
    Value(ArgType),
}

/// An inline data rule: the lifter's own code at the site, reading a host value (game/sites.def's HOS_VALUE).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// out = in * the value (a float's bits); 0 leaves the instruction's own result.
    MulF32,
    /// out *= the value (a float's bits); 0 or 1.0 is off.
    ScaleF32,
    /// the value = a register: the game tells the host something, as data.
    NoteU64,
}

impl Rule {
    const ALL: [Rule; 3] = [Rule::MulF32, Rule::NoteU64, Rule::ScaleF32];

    /// Its name in a table.
    pub fn name(self) -> &'static str {
        match self {
            Rule::MulF32 => "mul_f32",
            Rule::ScaleF32 => "scale_f32",
            Rule::NoteU64 => "note_u64",
        }
    }

    /// The C type of the host value it reads.
    pub fn host_type(self) -> &'static str {
        match self {
            Rule::MulF32 | Rule::ScaleF32 => "uint32_t",
            Rule::NoteU64 => "uint64_t",
        }
    }

    /// The arguments it reads, by name, and the class of register each must be.
    pub fn needs(self) -> &'static [(&'static str, RegClass)] {
        match self {
            Rule::MulF32 => &[("in", RegClass::S), ("out", RegClass::S)],
            Rule::ScaleF32 => &[("out", RegClass::S)],
            Rule::NoteU64 => &[("what", RegClass::X)],
        }
    }

    fn parse(s: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.name() == s)
    }
}

/// Code elsewhere in a site's function that its callback relies on, checked where the site finds the function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guard {
    /// Bytes from the function's entry.
    pub offset: u64,
    /// The words there.
    pub signature: Signature,
}

/// What an anchor matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnchorKind {
    /// Instruction words in a row; the point is word `at` of them.
    Words {
        /// The words.
        words: Signature,
        /// Which of them is the point.
        at: usize,
    },
    /// A bl to a site's function, a function `f_<address>` or an exported symbol (past PLT stubs).
    Call(String),
    /// A blr through a vtable's slot: `ldr xN, [xM, #slot]` the nearest load of xN in the four instructions before.
    Vcall(u64),
    /// The add of an adrp + add pair that makes the address of this string.
    String(String),
}

/// An end of an anchor's slice: another site's point, or an anchor of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// Another site's point.
    Site(String),
    /// An anchor found in the whole function.
    Anchor(Box<Anchor>),
}

/// A point found inside its function by what is there (docs/dev/reference/hooks.md#anchors): its matches after `start`
/// and before `end`, the `ordinal`-th in address order (else the only one; with `of`, of exactly that many), moved by
/// `shift` instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// What it matches.
    pub kind: AnchorKind,
    /// Only matches after this.
    pub start: Option<End>,
    /// Only matches before this.
    pub end: Option<End>,
    /// Which match, from 0.
    pub ordinal: Option<usize>,
    /// With `ordinal`: how many matches the slice must hold (another count stops the lift).
    pub of: Option<usize>,
    /// Whole instructions after (negative: before) the match.
    pub shift: i64,
}

impl Anchor {
    /// What it is, in words (the lift's messages).
    pub fn describe(&self) -> String {
        let mut what = match &self.kind {
            AnchorKind::Words { words, at } => {
                format!("the words {words}{}", if *at > 0 { format!(" (word {at})") } else { String::new() })
            }
            AnchorKind::Call(t) => format!("the call to {t}"),
            AnchorKind::Vcall(slot) => format!("the call through vtable slot {slot:#x}"),
            AnchorKind::String(s) => format!("the reference to the string {}", py_str(s)),
        };
        if let Some(n) = self.ordinal {
            let of = self.of.map(|m| format!("{m} of ")).unwrap_or_default();
            what = format!("number {n} of {of}{what}");
        }
        for (word, end) in [("after", &self.start), ("before", &self.end)] {
            match end {
                Some(End::Site(name)) => what += &format!(" {word} {name}"),
                Some(End::Anchor(a)) => what += &format!(" {word} {}", a.describe()),
                None => {}
            }
        }
        if self.shift != 0 {
            what += &format!(", {:+} instruction{}", self.shift, if self.shift.abs() > 1 { "s" } else { "" });
        }
        what
    }
}

/// A function found by what it does: it calls each of `calls`, makes the address of each of `strings`, holds `words`,
/// and has `size` instructions and `blocks` blocks (each low and high).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fingerprint {
    /// Sites' functions, functions `f_<address>` or exported symbols it calls.
    pub calls: Vec<String>,
    /// Strings it makes the address of.
    pub strings: Vec<String>,
    /// Instruction words it holds in a row.
    pub words: Option<Signature>,
    /// How many instructions it has.
    pub size: Option<(u64, u64)>,
    /// How many blocks it has.
    pub blocks: Option<(u64, u64)>,
}

/// One test of a fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// It calls this.
    Call(String),
    /// It makes this string's address.
    String(String),
    /// It holds these words.
    Words(Signature),
    /// Its instructions, from and to.
    Size(u64, u64),
    /// Its blocks, from and to.
    Blocks(u64, u64),
}

impl Part {
    /// The test in words (the lift's messages).
    pub fn describe(&self) -> String {
        match self {
            Part::Call(t) => format!("calls {t}"),
            Part::String(s) => format!("makes the string {}", py_str(s)),
            Part::Words(w) => format!("holds {w}"),
            Part::Size(lo, hi) => format!("has {lo} to {hi} instructions"),
            Part::Blocks(lo, hi) => format!("has {lo} to {hi} blocks"),
        }
    }
}

impl Fingerprint {
    /// Its tests, each a function must pass, in the order the table gives them kinds.
    pub fn parts(&self) -> Vec<Part> {
        let mut out: Vec<Part> = self.calls.iter().map(|t| Part::Call(t.clone())).collect();
        out.extend(self.strings.iter().map(|s| Part::String(s.clone())));
        out.extend(self.words.iter().map(|w| Part::Words(w.clone())));
        out.extend(self.size.map(|(lo, hi)| Part::Size(lo, hi)));
        out.extend(self.blocks.map(|(lo, hi)| Part::Blocks(lo, hi)));
        out
    }
}

/// A rule's use at a site: which rule, and the host value it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleUse {
    /// The rule.
    pub rule: Rule,
    /// The host value's name (game/sites.def's HOS_VALUE).
    pub value: String,
}

/// A site in the game's code: where its function is found and where in it the hook goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSite {
    /// The function's address in this build (`function = "f_<address>"`).
    pub function: u64,
    /// The form.
    pub form: Form,
    /// Bytes from the entry to the site's instruction (0 for entry and return); `None` when a point finds it.
    pub offset: Option<u64>,
    /// The point, found by an anchor.
    pub point: Option<Anchor>,
    /// A jump's target: bytes from the entry.
    pub to: Option<u64>,
    /// An entry site whose callback may return from the function.
    pub can_return: bool,
    /// An inline rule, in place of a callback.
    pub rule: Option<RuleUse>,
    /// The words that find the function.
    pub signature: Option<Signature>,
    /// Bytes from the function's entry to the signature's first word.
    pub signature_offset: i64,
    /// Another site whose function this one is in.
    pub inside: Option<String>,
    /// What the function does, which finds it (or checks what the signature found).
    pub fingerprint: Option<Fingerprint>,
    /// Code elsewhere in the function the callback relies on.
    pub guards: Vec<Guard>,
}

impl CodeSite {
    /// Whether the site is found only with the lift's functions (an anchor, or a fingerprint alone): a signature, or
    /// another site's function and an offset, is found in the code alone.
    pub fn needs_lift(&self) -> bool {
        self.point.is_some() || (self.fingerprint.is_some() && self.signature.is_none())
    }
}

/// What kind of site it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A place in the game's code.
    Code(Box<CodeSite>),
    /// A virtual method's slot in a vtable (part of the format; not placed yet).
    Vslot {
        /// The function's name in this build, if the table gives it.
        function: Option<u64>,
        /// The vtable's guest address.
        vtable: u64,
        /// The method's index in it.
        slot: u64,
    },
}

/// One checked site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Its name (`text.letterStep`).
    pub name: String,
    /// One line saying what it is.
    pub doc: String,
    /// What its callback is.
    pub shape: Shape,
    /// Where it is reached.
    pub thread: Thread,
    /// The registers its callback reads.
    pub args: Vec<Arg>,
    /// What its callback returns.
    pub returns: Returns,
    /// The module it is in (`main`).
    pub module: String,
    /// Whether every build must locate it.
    pub require: bool,
    /// Its kind and where it is.
    pub kind: Kind,
}

impl Site {
    /// Its C identifier.
    pub fn ident(&self) -> String {
        ident(&self.name)
    }

    /// Its place in the code, for a code site.
    pub fn code(&self) -> Option<&CodeSite> {
        match &self.kind {
            Kind::Code(c) => Some(c),
            Kind::Vslot { .. } => None,
        }
    }

    /// Whether it changes the game's outcome on a worker thread with a callback (a code site of an outcome form, or
    /// `can_return`, with no inline rule): the host's `HOS_SITE_OUTCOME` lines list exactly these, and the host refuses
    /// them a callback set at run time (docs/dev/reference/hooks.md, who may change the outcome).
    pub fn decides_on_worker(&self) -> bool {
        self.thread == Thread::Worker
            && self.code().is_some_and(|c| c.rule.is_none() && (c.form.changes_outcome() || c.can_return))
    }
}

// ---- reading TOML values the way the checks need them

/// A value as Python's repr writes it (the messages say what the table holds as nx2/hooks.py's do).
fn repr(v: Option<&Value>) -> String {
    match v {
        None => "None".into(),
        Some(Value::String(s)) => py_str(s),
        Some(Value::Integer(i)) => i.to_string(),
        Some(Value::Boolean(b)) => if *b { "True" } else { "False" }.into(),
        Some(Value::Float(x)) => x.to_string(),
        Some(other) => other.to_string(),
    }
}

/// A string as Python's repr writes it.
fn py_str(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') { format!("\"{s}\"") } else { format!("'{}'", s.replace('\'', "\\'")) }
}

/// A whole number (not a boolean) from `least`, a multiple of `multiple`.
fn whole(v: Option<&Value>, what: &str, multiple: i64, least: i64) -> Result<i64, String> {
    match v {
        Some(Value::Integer(n)) if *n >= least && n % multiple == 0 => Ok(*n),
        _ => Err(format!(
            "{what} must be a whole number{}{}, not {}",
            if least != 0 { format!(" from {least}") } else { String::new() },
            if multiple > 1 { format!(", a multiple of {multiple}") } else { String::new() },
            repr(v)
        )),
    }
}

fn plural(n: usize) -> &'static str {
    if n > 1 { "s" } else { "" }
}

fn unknown_keys<'a>(raw: &'a Table, known: &BTreeSet<&str>) -> Vec<&'a str> {
    raw.keys().map(String::as_str).filter(|k| !known.contains(k)).collect()
}

fn str_of<'a>(raw: &'a Table, key: &str) -> Option<&'a str> {
    raw.get(key).and_then(Value::as_str)
}

fn one_of(v: Option<&Value>, default: &str, choices: &[&str], what: &str) -> Result<String, String> {
    match v {
        None if choices.contains(&default) => Ok(default.into()),
        Some(Value::String(s)) if choices.contains(&s.as_str()) => Ok(s.clone()),
        _ => Err(format!("{what} {}: one of {}", repr(v), choices.join(", "))),
    }
}

fn flag(raw: &Table, key: &str, default: bool) -> Result<bool, String> {
    match raw.get(key) {
        None => Ok(default),
        Some(Value::Boolean(b)) => Ok(*b),
        Some(_) => Err(format!("{key} is true or false")),
    }
}

// ---- one site

/// One `[[site]]` table checked (definition and location joined): a [`Site`], or what is wrong. `engine`: the names of
/// the sites the engine's own features own (game/sites.def's HOS_SITE_ENGINE lines), for the rule on who may change
/// the outcome on a worker thread; `None` leaves that rule out.
pub fn check_site(raw: &Table, engine: Option<&BTreeSet<String>>) -> Result<Site, String> {
    let kind = match raw.get("kind") {
        None => "code",
        Some(Value::String(s)) if s == "code" || s == "vslot" => s.as_str(),
        other => return Err(format!("kind {}: one of code, vslot", repr(other))),
    };
    let known = if kind == "code" { code_keys() } else { vslot_keys() };
    let unknown = unknown_keys(raw, &known);
    if !unknown.is_empty() {
        let mut unknown = unknown;
        unknown.sort_unstable();
        return Err(format!("unknown key{} {}", plural(unknown.len()), unknown.join(", ")));
    }
    let name = match str_of(raw, "name") {
        Some(n) if is_name(n) => n.to_string(),
        _ => {
            return Err(format!(
                "name {}: dot-separated words, each a lower-case letter then letters and digits (text.letterStep)",
                repr(raw.get("name"))
            ));
        }
    };
    let doc = match str_of(raw, "doc") {
        Some(d) if !d.trim().is_empty() => d.to_string(),
        _ => return Err("doc: one line saying what the site is".into()),
    };
    let shape = match one_of(raw.get("shape"), "", &["event", "filter", "data"], "shape")?.as_str() {
        "event" => Shape::Event,
        "filter" => Shape::Filter,
        _ => Shape::Data,
    };
    let thread = if one_of(raw.get("thread"), "", &["serial", "worker"], "thread")? == "serial" {
        Thread::Serial
    } else {
        Thread::Worker
    };
    if shape == Shape::Filter && thread != Thread::Serial {
        return Err("a filter runs only at a site reached on the serial thread (thread = \"serial\")".into());
    }
    let args: Vec<Arg> = match raw.get("args") {
        None => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|a| match a {
                Value::String(s) => Arg::parse(s),
                other => Err(format!(
                    "{} is not an argument: <register>:<type>[:<name>], as x19:ptr:anim",
                    repr(Some(other))
                )),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("args is a list".into()),
    };
    let names: Vec<&str> = args.iter().filter_map(|a| a.name.as_deref()).collect();
    if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
        return Err("two arguments share a name".into());
    }
    let returns = match raw.get("returns") {
        None => Returns::Void,
        Some(Value::String(s)) if s == "void" => Returns::Void,
        Some(Value::String(s)) if ArgType::parse(s).is_some() => {
            Returns::Value(ArgType::parse(s).unwrap_or(ArgType::U64))
        }
        other => {
            let all: Vec<&str> = ArgType::ALL.iter().map(|t| t.name()).collect();
            return Err(format!("returns {}: void or one of {}", repr(other), all.join(", ")));
        }
    };
    let module = one_of(raw.get("module"), "main", MODULES, "module")?;
    let function = match raw.get("function") {
        None => None,
        Some(Value::String(s)) if function_address(s).is_some() => function_address(s),
        other => {
            return Err(format!("function {}: the function's name in this build, f_ and 8 hex digits", repr(other)));
        }
    };
    let require = flag(raw, "require", true).map_err(|_| "require is true or false".to_string())?;
    let site = |kind| Site {
        name: name.clone(),
        doc: doc.clone(),
        shape,
        thread,
        args: args.clone(),
        returns,
        module: module.clone(),
        require,
        kind,
    };
    if kind == "vslot" {
        if shape == Shape::Filter {
            return Err("a vslot site is an event or a data rule (it runs on the caller's thread)".into());
        }
        let vtable = whole(raw.get("vtable"), "vtable (the vtable's guest address)", 8, 1)? as u64;
        let slot = whole(raw.get("slot"), "slot (the method's index in the vtable)", 1, 0)? as u64;
        return Ok(site(Kind::Vslot { function, vtable, slot }));
    }
    let function = function.ok_or("function: the function's name in this build (f_80b1e140)")?;
    let forms: Vec<&str> = Form::ALL.iter().map(|f| f.name()).collect();
    let form = match raw.get("at") {
        None => Form::Entry,
        Some(Value::String(s)) if Form::parse(s).is_some() => Form::parse(s).unwrap_or(Form::Entry),
        other => {
            return Err(format!("at {}: one of {} (docs/dev/reference/hooks.md)", repr(other), forms.join(", ")));
        }
    };
    let at_offset_forms =
        Form::ALL.iter().filter(|f| f.at_offset()).map(|f| format!("\"{}\"", f.name())).collect::<Vec<_>>();
    let (offset, point) = if !form.at_offset() {
        if raw.contains_key("offset") || raw.contains_key("point") {
            let key = if raw.contains_key("offset") { "offset" } else { "point" };
            return Err(format!("{key} is for at = {}", at_offset_forms.join(" / ")));
        }
        (Some(0), None)
    } else if let Some(p) = raw.get("point") {
        if raw.contains_key("offset") {
            return Err(
                "offset or point: the instruction of the site by its place, or by what is there, not both".into()
            );
        }
        (None, Some(anchor(p, &name, false).map_err(|e| format!("point: {e}"))?))
    } else {
        let off = whole(
            raw.get("offset"),
            "offset (from the function's entry to the instruction of the site, or point = {...}: \
             docs/dev/reference/hooks.md#anchors)",
            4,
            0,
        )?;
        (Some(off as u64), None)
    };
    let to = if form == Form::Jump {
        Some(whole(raw.get("to"), "to (from the function's entry to the instruction the jump goes to)", 4, 0)? as u64)
    } else if raw.contains_key("to") {
        return Err("to is for at = \"jump\"".into());
    } else {
        None
    };
    let can_return = flag(raw, "can_return", false)?;
    if can_return && form != Form::Entry {
        return Err(
            "can_return is for at = \"entry\" (a branch site takes its branch, a jump site jumps; the others go on)"
                .into(),
        );
    }
    let rule = match raw.get("rule") {
        None => {
            if raw.contains_key("value") {
                return Err("value is for a rule".into());
            }
            None
        }
        Some(v) => {
            let rule = v.as_str().and_then(Rule::parse).ok_or_else(|| {
                format!("rule {}: one of {}", repr(Some(v)), ["mul_f32", "note_u64", "scale_f32"].join(", "))
            })?;
            if shape != Shape::Data {
                return Err("a rule is a data rule (shape = \"data\")".into());
            }
            if !form.is_rule_form() {
                return Err(format!(
                    "a rule is inlined at = \"entry\" / \"return\" / \"after\" / \"before\", not {}",
                    py_str(form.name())
                ));
            }
            if can_return {
                return Err("a rule cannot return from the function".into());
            }
            let value = match str_of(raw, "value") {
                Some(v) if is_name(v) => v.to_string(),
                _ => {
                    return Err(
                        "value: the name of the host value the rule reads (game/sites.def's HOS_VALUE lines)".into()
                    );
                }
            };
            let mut needs: Vec<_> = rule.needs().to_vec();
            needs.sort_by_key(|(n, _)| *n);
            for (want, class) in needs {
                if !args.iter().any(|a| a.name.as_deref() == Some(want) && a.class == class) {
                    let l = class.letter();
                    return Err(format!(
                        "rule {} needs an argument named {want} in a {l} register (args: \"{l}<n>:...:{want}\")",
                        rule.name()
                    ));
                }
            }
            Some(RuleUse { rule, value })
        }
    };
    let inside = raw.get("in");
    let has_signature = raw.contains_key("signature");
    let has_fingerprint = raw.contains_key("fingerprint");
    let inside = match inside {
        Some(v) => {
            if has_signature || has_fingerprint {
                return Err("in names the function: no signature or fingerprint beside it".into());
            }
            match v.as_str() {
                Some(s) if is_name(s) && s != name => Some(s.to_string()),
                _ => return Err("in: another site's name, whose function this site is in".into()),
            }
        }
        None if !has_signature && !has_fingerprint => {
            return Err("signature, in or fingerprint: how the site's function is found".into());
        }
        None => None,
    };
    let signature = match raw.get("signature") {
        None => None,
        Some(v) => Some(signature(v).map_err(|e| format!("signature: {e}"))?),
    };
    let fingerprint = match raw.get("fingerprint") {
        None => None,
        Some(v) => Some(fingerprint(v).map_err(|e| format!("fingerprint: {e}"))?),
    };
    if signature.is_none() && raw.contains_key("signature_offset") {
        return Err("signature_offset is for a signature".into());
    }
    let signature_offset = match raw.get("signature_offset") {
        None => 0,
        Some(Value::Integer(n)) if n % 4 == 0 => *n,
        other => {
            return Err(format!(
                "signature_offset (from the function's entry to the signature's first word) must be a multiple of 4, not {}",
                repr(other)
            ));
        }
    };
    let guards = guards(raw.get("guards"))?;
    let changes = form.changes_outcome() || can_return;
    if changes && thread == Thread::Worker && rule.is_none() && engine.is_some_and(|e| !e.contains(&name)) {
        return Err(format!(
            "{} on a worker thread changes the game's outcome: such a site is an engine feature's (a HOS_SITE_ENGINE \
             line in game/sites.def) or an inline data rule, never a callback set at run time (a HOS_SITE line); \
             on the serial thread any site may (docs/dev/reference/hooks.md, who may change the outcome)",
            if can_return { "can_return".to_string() } else { format!("at = \"{}\"", form.name()) }
        ));
    }
    Ok(site(Kind::Code(Box::new(CodeSite {
        function,
        form,
        offset,
        point,
        to,
        can_return,
        rule,
        signature,
        signature_offset,
        inside,
        fingerprint,
        guards,
    }))))
}

fn signature(v: &Value) -> Result<Signature, String> {
    match v {
        Value::String(s) => Signature::parse(s),
        _ => Err("a signature is a string of instruction words".into()),
    }
}

fn guards(v: Option<&Value>) -> Result<Vec<Guard>, String> {
    let list = match v {
        None => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err("guards: a list of {offset = <from the entry>, signature = \"<words>\"}".into()),
    };
    list.iter()
        .map(|g| {
            let g = g
                .as_table()
                .filter(|t| t.len() == 2 && t.contains_key("offset") && t.contains_key("signature"))
                .ok_or("guards: each is {offset = <from the entry>, signature = \"<words>\"}, nothing else")?;
            let offset = match g.get("offset") {
                Some(Value::Integer(n)) if *n >= 0 && n % 4 == 0 => *n as u64,
                other => {
                    return Err(format!(
                        "guards: offset (from the function's entry) must be a multiple of 4, not {}",
                        repr(other)
                    ));
                }
            };
            let signature = signature(g.get("signature").unwrap_or(&Value::Boolean(false)))
                .map_err(|e| format!("guards: signature: {e}"))?;
            Ok(Guard { offset, signature })
        })
        .collect()
}

fn anchor(v: &Value, name: &str, inner: bool) -> Result<Anchor, String> {
    let raw = v.as_table().ok_or(
        "a table: {words = \"...\"} or {call = \"...\"}, {vcall = <slot>}, {string = \"...\"}, with from, to, ordinal, of, shift",
    )?;
    let known: BTreeSet<&str> = ANCHOR_KEYS.iter().copied().collect();
    let mut unknown = unknown_keys(raw, &known);
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(format!("unknown key{} {}", plural(unknown.len()), unknown.join(", ")));
    }
    let kinds: Vec<&str> = ["words", "call", "vcall", "string"].into_iter().filter(|k| raw.contains_key(*k)).collect();
    let [kind] = kinds[..] else {
        return Err("one of words, call, vcall, string: what the point is".into());
    };
    let v = &raw[kind];
    let kind = match kind {
        "words" => {
            let words = signature(v)?;
            let at = match raw.get("at") {
                None => 0,
                Some(Value::Integer(n)) if *n >= 0 && (*n as usize) < words.len() => *n as usize,
                Some(_) => return Err(format!("at: which of the words is the point, 0 to {}", words.len() - 1)),
            };
            AnchorKind::Words { words, at }
        }
        _ if raw.contains_key("at") => return Err("at is for words".into()),
        "call" => match v.as_str() {
            Some(t) if !t.is_empty() && !t.chars().any(char::is_whitespace) => AnchorKind::Call(t.to_string()),
            _ => return Err("call: a site's name, a function's (f_<address>) or an exported symbol's".into()),
        },
        "vcall" => match v {
            Value::Integer(n) if *n >= 0 && n % 8 == 0 && *n <= 0x7ff8 => AnchorKind::Vcall(*n as u64),
            _ => return Err("vcall: the slot's offset in its vtable, a multiple of 8 up to 0x7ff8".into()),
        },
        _ => match v.as_str() {
            Some(s) if !s.is_empty() => AnchorKind::String(s.to_string()),
            _ => return Err("string: the text the code makes the address of".into()),
        },
    };
    let mut ends = [None, None];
    for (i, key) in ["from", "to"].into_iter().enumerate() {
        let Some(end) = raw.get(key) else { continue };
        if inner {
            return Err(format!("{key}: a slice's end is found in the whole function (no from or to of its own)"));
        }
        ends[i] = Some(match end {
            Value::String(s) => {
                if !is_name(s) || s == name {
                    return Err(format!("{key}: another site's name (its point), or an anchor"));
                }
                End::Site(s.clone())
            }
            other => End::Anchor(Box::new(anchor(other, name, true)?)),
        });
    }
    let [start, end] = ends;
    let ordinal = match raw.get("ordinal") {
        None => None,
        v => Some(whole(v, "ordinal (which match, from 0, in address order)", 1, 0)? as usize),
    };
    let of = match raw.get("of") {
        None => None,
        v => {
            let Some(n) = ordinal else { return Err("of is for an ordinal: how many matches it counts from".into()) };
            let m = whole(v, "of: how many matches the ordinal counts from", 1, 1)? as usize;
            if m <= n {
                return Err(format!("of: how many matches the ordinal counts from, more than the ordinal ({n})"));
            }
            Some(m)
        }
    };
    let shift = match raw.get("shift") {
        None => 0,
        Some(Value::Integer(n)) => *n,
        Some(_) => return Err("shift: whole instructions after (negative: before) the match".into()),
    };
    Ok(Anchor { kind, start, end, ordinal, of, shift })
}

fn fingerprint(v: &Value) -> Result<Fingerprint, String> {
    let raw = v.as_table().ok_or(
        "a table: {calls = [...], strings = [...], words = \"...\", size = [low, high], blocks = [low, high]}",
    )?;
    let known: BTreeSet<&str> = FINGERPRINT_KEYS.iter().copied().collect();
    let mut unknown = unknown_keys(raw, &known);
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(format!("unknown key{} {}", plural(unknown.len()), unknown.join(", ")));
    }
    let mut f = Fingerprint::default();
    for key in ["calls", "strings"] {
        let names = match raw.get(key) {
            None => Vec::new(),
            Some(Value::Array(list)) => list
                .iter()
                .map(|t| match t.as_str() {
                    Some(s) if !s.is_empty() && (key == "strings" || !s.chars().any(char::is_whitespace)) => {
                        Ok(s.to_string())
                    }
                    _ => Err(()),
                })
                .collect::<Result<Vec<_>, ()>>()
                .map_err(|()| list_error(key))?,
            Some(_) => return Err(list_error(key)),
        };
        if key == "calls" {
            f.calls = names;
        } else {
            f.strings = names;
        }
    }
    if let Some(w) = raw.get("words") {
        f.words = Some(signature(w)?);
    }
    for key in ["size", "blocks"] {
        let Some(v) = raw.get(key) else { continue };
        let pair = match v.as_array().map(Vec::as_slice) {
            Some([Value::Integer(lo), Value::Integer(hi)]) if *lo > 0 && *hi > 0 && lo <= hi => {
                (*lo as u64, *hi as u64)
            }
            _ => return Err(format!("{key}: [low, high], whole numbers from 1")),
        };
        if key == "size" {
            f.size = Some(pair);
        } else {
            f.blocks = Some(pair);
        }
    }
    if f.calls.is_empty() && f.strings.is_empty() && f.words.is_none() {
        return Err("calls, strings or words: what the function does (size and blocks only narrow it)".into());
    }
    Ok(f)
}

fn list_error(key: &str) -> String {
    format!("{key}: a list of {}", if key == "calls" { "names (sites, f_<address>, symbols)" } else { "texts" })
}

// ---- whole tables

fn parse_toml(text: &str, place: &str) -> Result<Table, TableError> {
    text.parse::<Table>().map_err(|e| whole_file(place, format!("not TOML: {}", e.message())))
}

/// A table's top level checked: its `[[site]]` tables.
fn top(doc: &Table, place: &str, keys: &[&str], format: i64, build_id: Option<&str>) -> Result<Vec<Value>, TableError> {
    let mut unknown: Vec<&str> = doc.keys().map(String::as_str).filter(|k| !keys.contains(k)).collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(whole_file(place, format!("unknown key{} {}", plural(unknown.len()), unknown.join(", "))));
    }
    if doc.get("format").and_then(Value::as_integer) != Some(format) {
        return Err(whole_file(place, format!("format = {format} expected, found {}", repr(doc.get("format")))));
    }
    if keys.contains(&"build") {
        let build = match doc.get("build").and_then(Value::as_str) {
            Some(b) if is_build(b) => b,
            _ => return Err(whole_file(place, "build: the game build's id, 40 lower-case hex digits")),
        };
        if let Some(id) = build_id
            && build != id.to_ascii_lowercase()
        {
            return Err(whole_file(place, format!("build {build}, the folder's is {}", id.to_ascii_lowercase())));
        }
    }
    match doc.get("site") {
        None => Ok(Vec::new()),
        Some(Value::Array(list)) => Ok(list.clone()),
        Some(_) => Err(whole_file(place, "site is an array of tables ([[site]])")),
    }
}

fn label(raw: &Value, i: usize) -> String {
    raw.as_table()
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("#{}", i + 1), String::from)
}

/// Each joined site checked, in order: the first fault names its file (`at`, else `place`) and the site.
fn sites(
    raws: &[(Value, Option<String>)],
    place: &str,
    engine: Option<&BTreeSet<String>>,
) -> Result<Vec<Site>, TableError> {
    let mut out: Vec<Site> = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (i, (raw, at)) in raws.iter().enumerate() {
        let place_of = at.as_deref().unwrap_or(place);
        let err = |what: String| TableError { place: place_of.into(), site: Some(label(raw, i)), what };
        let table = raw.as_table().ok_or_else(|| err("a site is a table".into()))?;
        let s = check_site(table, engine).map_err(err)?;
        if let Some(by) = seen.get(&s.ident()) {
            return Err(TableError {
                place: place_of.into(),
                site: Some(s.name.clone()),
                what: format!("its name is taken (by {by})"),
            });
        }
        seen.insert(s.ident(), s.name.clone());
        out.push(s);
    }
    let code: BTreeSet<&str> = out.iter().filter(|s| s.code().is_some()).map(|s| s.name.as_str()).collect();
    for s in &out {
        let Some(c) = s.code() else { continue };
        let mut refs: Vec<&str> = c.inside.iter().map(String::as_str).collect();
        if let Some(p) = &c.point {
            for end in [&p.start, &p.end].into_iter().flatten() {
                if let End::Site(n) = end {
                    refs.push(n);
                }
            }
        }
        if let Some(r) = refs.into_iter().find(|r| !code.contains(r)) {
            return Err(TableError {
                place: place.into(),
                site: Some(s.name.clone()),
                what: format!("it names the site {r}, which the table has not (or not as code)"),
            });
        }
    }
    Ok(out)
}

/// A table with the definitions and the locations in one file (format = 1, as the checks write them), checked whole.
pub fn parse_table(
    text: &str,
    place: &str,
    build_id: Option<&str>,
    engine: Option<&BTreeSet<String>>,
) -> Result<Vec<Site>, TableError> {
    let raw = top(&parse_toml(text, place)?, place, &["format", "build", "site"], ONE_FILE, build_id)?;
    sites(&raw.into_iter().map(|r| (r, None)).collect::<Vec<_>>(), place, engine)
}

/// The sites' definitions (format = 2, no build): each a table with only a definition's keys, a name each, no name twice.
pub fn parse_definitions(text: &str, place: &str) -> Result<Vec<Table>, TableError> {
    let mut out: Vec<Table> = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, raw) in top(&parse_toml(text, place)?, place, &["format", "site"], FORMAT, None)?.into_iter().enumerate() {
        let label = label(&raw, i);
        let err = |what: String| TableError { place: place.into(), site: Some(label.clone()), what };
        let t = match raw {
            Value::Table(t) if t.get("name").is_some_and(Value::is_str) => t,
            _ => return Err(err("a site is a table with a name".into())),
        };
        let defs: BTreeSet<&str> = DEF_KEYS.iter().copied().collect();
        let mut unknown = unknown_keys(&t, &defs);
        if !unknown.is_empty() {
            unknown.sort_unstable();
            let all_locations = unknown.iter().all(|k| LOC_KEYS.contains(k) || VSLOT_LOC_KEYS.contains(k));
            return Err(err(format!(
                "{} {}",
                unknown.join(", "),
                if all_locations { "is where the site is in a build: its locations.toml" } else { "unknown" }
            )));
        }
        let name = str_of(&t, "name").unwrap_or_default().to_string();
        if !seen.insert(name.clone()) {
            return Err(err("defined twice".into()));
        }
        out.push(t);
    }
    Ok(out)
}

/// A build's locations (format = 2, its build): each name with its location's table, in the file's order.
pub fn parse_locations(text: &str, place: &str, build_id: Option<&str>) -> Result<Vec<(String, Table)>, TableError> {
    let mut out: Vec<(String, Table)> = Vec::new();
    for (i, raw) in
        top(&parse_toml(text, place)?, place, &["format", "build", "site"], FORMAT, build_id)?.into_iter().enumerate()
    {
        let label = label(&raw, i);
        let err = |what: String| TableError { place: place.into(), site: Some(label.clone()), what };
        let t = match raw {
            Value::Table(t) if t.get("name").is_some_and(Value::is_str) => t,
            _ => return Err(err("a location is a table with the site's name".into())),
        };
        let locs: BTreeSet<&str> = LOC_KEYS.iter().chain(VSLOT_LOC_KEYS).copied().collect();
        let mut unknown = unknown_keys(&t, &locs);
        if !unknown.is_empty() {
            unknown.sort_unstable();
            let all_definitions = unknown.iter().all(|k| DEF_KEYS.contains(k));
            return Err(err(format!(
                "{} {}",
                unknown.join(", "),
                if all_definitions { "is the site's definition: game/sites/sites.toml" } else { "unknown" }
            )));
        }
        let name = str_of(&t, "name").unwrap_or_default().to_string();
        if out.iter().any(|(n, _)| *n == name) {
            return Err(err("located twice".into()));
        }
        out.push((name, t));
    }
    Ok(out)
}

/// A location's arguments: the definition's, each the same type and name, perhaps in other registers.
fn same_args(defined: &Value, located: &Value) -> bool {
    let (Some(d), Some(l)) = (defined.as_array(), located.as_array()) else { return false };
    d.len() == l.len()
        && d.iter().zip(l).all(|(d, l)| match (d.as_str().map(Arg::parse), l.as_str().map(Arg::parse)) {
            (Some(Ok(d)), Some(Ok(l))) => d.ty == l.ty && d.name == l.name,
            _ => false,
        })
}

/// The build's sites: each definition with its location, checked whole, in the definitions' order (the order of
/// several hooks at one instruction). A required site the build does not locate, or a location of no defined site,
/// stops naming both files.
pub fn join(
    defs: &[Table],
    locs: &[(String, Table)],
    defs_place: &str,
    locs_place: &str,
    engine: Option<&BTreeSet<String>>,
) -> Result<Vec<Site>, TableError> {
    let defined: BTreeSet<&str> = defs.iter().filter_map(|d| str_of(d, "name")).collect();
    if let Some((name, _)) = locs.iter().filter(|(n, _)| !defined.contains(n.as_str())).min_by(|a, b| a.0.cmp(&b.0)) {
        return Err(TableError {
            place: locs_place.into(),
            site: Some(name.clone()),
            what: format!("located, but {defs_place} does not define it"),
        });
    }
    let mut raws = Vec::new();
    for d in defs {
        let name = str_of(d, "name").unwrap_or_default();
        let Some((_, loc)) = locs.iter().find(|(n, _)| n == name) else {
            if d.get("require").and_then(Value::as_bool) != Some(false) {
                return Err(TableError {
                    place: locs_place.into(),
                    site: Some(name.into()),
                    what: format!(
                        "not located in this build, and {defs_place} requires it (require = false lets a build leave it out)"
                    ),
                });
            }
            continue;
        };
        let mut merged = d.clone();
        for (k, v) in loc {
            if k == "args"
                && let Some(da) = d.get("args")
                && !same_args(da, v)
            {
                let names: Vec<&str> =
                    da.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                return Err(TableError {
                    place: locs_place.into(),
                    site: Some(name.into()),
                    what: format!(
                        "args: the definition's arguments ({}), each the same type and name, in this build's registers",
                        names.join(", ")
                    ),
                });
            }
            merged.insert(k.clone(), v.clone());
        }
        raws.push((Value::Table(merged), Some(locs_place.to_string())));
    }
    sites(&raws, &format!("{defs_place} with {locs_place}"), engine)
}

#[cfg(test)]
pub(crate) mod tests;
