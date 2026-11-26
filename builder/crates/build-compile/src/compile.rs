//! One unit: its key worked out, its object found in the store, or compiled.

use crate::key::{self, Anchors};
use crate::options::IrSource;
use crate::plan::{Kind, Unit};
use crate::prebuilt::Prebuilt;
use crate::state::State;
use crate::store::{self, Entry, Store};
use build_core::memory::Ticket;
use build_core::{Error, IoContext, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The prerequisites of a make-style depfile (clang's `-MD`): the target and its `:` left out, line continuations
/// joined, `\ ` a space in a name, `$$` a `$`, `\#` a `#`. Clang writes `/` on Windows too.
pub fn depfile_paths(text: &str) -> Vec<String> {
    let text = text.replace("\\\r\n", " ").replace("\\\n", " ");
    let bytes = text.as_bytes();
    // (the target's colon: one followed by white space or the end, so a Windows drive's C: is not it)
    let Some(colon) =
        (0..bytes.len()).find(|&i| bytes[i] == b':' && bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace()))
    else {
        return Vec::new();
    };
    let body: Vec<char> = text[colon + 1..].chars().collect();
    let (mut out, mut cur, mut i) = (Vec::new(), String::new(), 0);
    while i < body.len() {
        let c = body[i];
        if c == '\\' && matches!(body.get(i + 1), Some(' ' | '#')) {
            cur.push(body[i + 1]);
            i += 2;
        } else if c == '$' && body.get(i + 1) == Some(&'$') {
            cur.push('$');
            i += 2;
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            i += 1;
        } else {
            cur.push(c);
            i += 1;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// How a unit's compile came out.
#[derive(Debug)]
pub enum Made {
    /// Compiled, its object stored by this key.
    Compiled(String),
    /// Compiled, but its key could not be worked out (a header gone meanwhile): the object is used, not stored.
    Unstored,
    /// Its text made, its object found in the store by it (a lifted unit whose key changed but not its text).
    Stored(String),
    /// It did not compile: what the compiler said, and whether the system stopped it (out of memory, most likely).
    Failed {
        /// The compiler's last words.
        text: String,
        /// Stopped by the system, not by an error in the source: worth a try alone.
        killed: bool,
        /// The content key of the text it was given, when that was made now (a lifted unit's, from the lift's
        /// session): a try again makes it again, and a text that comes out other than this one was changed by the PC.
        made: Option<String>,
    },
}

/// What a compile needs to find and store objects: shared by the workers.
pub struct Keys<'a> {
    /// The folders keys name.
    pub anchors: Anchors,
    /// The last build's record.
    pub state: &'a State,
    /// The store.
    pub store: &'a Store,
    /// The release's prebuilt set, if one is used.
    pub prebuilt: Option<&'a Prebuilt>,
    /// Each compiler's identity.
    pub compilers: HashMap<PathBuf, String>,
    /// Where a lifted unit's text comes from when gen/ does not hold its file.
    pub ir: Option<IrSource>,
    /// How long one compile may run before it is stopped ([`UNIT_LIMIT`]).
    pub limit: Duration,
    headers: Mutex<HashMap<String, Option<String>>>,
}

impl<'a> Keys<'a> {
    /// The keys' context.
    pub fn new(
        anchors: Anchors,
        state: &'a State,
        store: &'a Store,
        prebuilt: Option<&'a Prebuilt>,
        compilers: HashMap<PathBuf, String>,
    ) -> Keys<'a> {
        Keys {
            anchors,
            state,
            store,
            prebuilt,
            compilers,
            ir: None,
            limit: UNIT_LIMIT,
            headers: Mutex::new(HashMap::new()),
        }
    }

    /// A unit's base key; `None` for a lifted unit whose text is not made yet (neither its file nor its content key
    /// listed).
    pub fn base(&self, u: &Unit) -> Result<Option<String>> {
        // (a lifted unit's hash is the lift's, while its file is the size the lift listed, or not there at all: the
        // lift keeps no texts by default. 5.7 GB not read again)
        let sha1 = match &u.listed {
            Some(l) if l.pending() && !u.source.is_file() => return Ok(None),
            Some(l)
                if !l.pending()
                    && std::fs::metadata(&u.source).ok().is_none_or(|m| m.len() == l.bytes)
                    && (u.source.is_file() || self.ir.is_some()) =>
            {
                l.sha1.clone()
            }
            _ => self.state.sha1(&u.source)?,
        };
        Ok(Some(self.base_of(u, &sha1)))
    }

    /// A unit's base key, its source's content key given.
    pub fn base_of(&self, u: &Unit, sha1: &str) -> String {
        let cc = self.compilers.get(&u.compiler).map_or("?", String::as_str);
        key::base(cc, &key::normal_flags(&u.flags, &self.anchors), sha1, &u.extra)
    }

    /// A named header's hash now (`None`: gone), once a build.
    pub fn header(&self, named: &str) -> Option<String> {
        if let Some(h) = self.headers.lock().unwrap_or_else(|p| p.into_inner()).get(named) {
            return h.clone();
        }
        let h = self.state.sha1_if_there(&self.anchors.resolve(named));
        self.headers.lock().unwrap_or_else(|p| p.into_inner()).insert(named.to_string(), h.clone());
        h
    }

    /// The object key a base has now in the store, or in the prebuilt set (then copied into the store, checked).
    pub fn find(&self, base: &str, third_party: bool) -> Option<String> {
        if let Some(k) = store::lookup(&self.store.entries(base), |h| self.header(h))
            && self.store.has(&k)
        {
            return Some(k);
        }
        let set = self.prebuilt.filter(|_| third_party)?;
        set.take(base, self.store, |h| self.header(h))
    }
}

/// `<obj>`'s stamp: the key its object was made or taken by.
pub fn stamp_of(obj: &Path) -> PathBuf {
    let mut n = obj.as_os_str().to_os_string();
    n.push(".key");
    PathBuf::from(n)
}

/// Whether `<obj>` is the object of `key` already.
pub fn up_to_date(obj: &Path, key: &str) -> bool {
    obj.is_file() && std::fs::read_to_string(stamp_of(obj)).is_ok_and(|s| s == key)
}

/// The object of `key` placed at the unit's object from the store, stamped.
pub fn take(u: &Unit, key: &str, store: &Store) -> Result<bool> {
    stamped(&u.object, Some(key), || store.fetch(key, &u.object))
}

/// An object put at `obj` by `place` (whether it did), then stamped with `key` (`None`: left unstamped). The old stamp
/// goes before the object is replaced and the new one is written after it is in: a run stopped between the two leaves
/// an object with no stamp (compiled or taken again next time), never one stamped with another object's key.
fn stamped(obj: &Path, key: Option<&str>, place: impl FnOnce() -> Result<bool>) -> Result<bool> {
    build_core::fsx::remove_file(&stamp_of(obj))?;
    if !place()? {
        return Ok(false);
    }
    if let Some(k) = key {
        build_core::fsx::write_atomic(&stamp_of(obj), k.as_bytes())?;
    }
    Ok(true)
}

/// A zero byte in a source: the lift and the tree never write one into C, so the drive or the memory changed the file
/// after it was written (a player's 26 MB chunk had one where a "(" belonged, and the compiler said only "extraneous
/// ')' before ';'"). Said in one line with "error:", where the compiler's message would be.
fn zero_byte(src: &Path) -> Option<String> {
    let data = std::fs::read(src).ok()?;
    let at = data.iter().position(|&b| b == 0)?;
    let line = data[..at].iter().filter(|&&b| b == b'\n').count() + 1;
    let col = at - data[..at].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1) + 1;
    Some(format!(
        "{}:{line}:{col}: error: this file has a zero byte, which the build never writes: the drive or the PC's memory \
         changed it after it was written. Build again: the file is made again. If this keeps happening, check the drive \
         for errors and test the memory",
        src.display()
    ))
}

/// The command a unit compiles with (without its depfile's flags: what `compile_commands.json` names).
pub fn command(u: &Unit) -> Vec<String> {
    let mut c = vec![u.compiler.display().to_string()];
    c.extend(u.flags.iter().cloned());
    if u.depfile {
        c.push("-MD".into());
    }
    c.extend(["-c".into(), u.source.display().to_string(), "-o".into(), u.object.display().to_string()]);
    c
}

/// The compiler's first `error:` line in `text`, trimmed: what it stopped on, often hundreds of lines before its last
/// words (a note, a count of errors).
pub fn first_error(text: &str) -> Option<&str> {
    text.lines().find(|l| l.contains("error:")).map(str::trim)
}

/// About `n` bytes of a failure's text: all of it when it fits, else its first `error:` line (or its first line), a
/// mark of what is left out, and its last bytes. The tail alone lost the error the compile stopped on.
fn clip(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let head = first_error(text).or_else(|| text.lines().next()).unwrap_or("");
    let head = &head[..floor(head, n / 2)];
    let end = tail(text, n.saturating_sub(head.len()));
    format!("{head}\n[... {} bytes left out ...]\n{end}", text.len() - head.len() - end.len())
}

/// The highest character boundary in `s` at or below `n`.
fn floor(s: &str, n: usize) -> usize {
    let mut i = n.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The last `n` bytes of `text`, cut at a character.
fn tail(text: &str, n: usize) -> String {
    let mut start = text.len().saturating_sub(n);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(unix)]
fn signal_of(s: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(s)
}

#[cfg(not(unix))]
fn signal_of(s: &std::process::ExitStatus) -> Option<i32> {
    // (Windows: STATUS_NO_MEMORY, STATUS_COMMITMENT_LIMIT; a process ended from outside exits 1, which says nothing)
    s.code().filter(|&c| c as u32 == 0xC000_0017 || c as u32 == 0xC000_012D)
}

/// The longest a unit's compile may take before it is stopped: the slowest units take minutes, and a slow CPU or a PC
/// paging hard some times that; one past an hour is a compiler that hangs, which a build would otherwise wait on for
/// ever. Not tried again alone (`killed: false`): the build stops naming the unit.
pub const UNIT_LIMIT: Duration = Duration::from_secs(60 * 60);

/// What a compile stopped at [`UNIT_LIMIT`] says, naming its unit (the compiler's last words after it).
fn over_limit(unit: &str, limit: Duration, said: &str) -> String {
    let mut text = format!(
        "error: the compiler ran {} minutes on {unit} without finishing and was stopped (a compiler that hangs, or a PC \
         far too short of memory for this file)",
        limit.as_secs().div_ceil(60)
    );
    if !said.trim().is_empty() {
        text.push('\n');
        text.push_str(&tail(said, 2000));
    }
    text
}

/// Compiles a unit (its base given, or `None`: a lifted unit whose text is made now), inside the gate's `ticket`:
/// the object stored and placed, or why not. `peak` gets the most memory the compiler was seen to hold (MB).
///
/// A lifted unit whose file gen/ does not hold has its text made ([`Keys::ir`]) and given to the compiler on its
/// standard input; its key is its text's, as a file's would be (the text names its file inside, so the object is the
/// same byte for byte), and an object the store has for that text is taken instead.
pub fn compile(u: &Unit, base: Option<&str>, keys: &Keys, ticket: Option<&Ticket<'_>>, peak: &mut u64) -> Result<Made> {
    if u.source.extension().is_some_and(|e| e == "c" || e == "cpp" || e == "h")
        && let Some(text) = zero_byte(&u.source)
    {
        return Ok(Made::Failed { text, killed: false, made: None });
    }
    let made_text = match &keys.ir {
        Some(ir) if u.listed.is_some() && !u.source.is_file() => {
            let name = u.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Some(ir.0.text(&name)?)
        }
        _ => None,
    };
    let owned_base;
    let text_key = made_text.as_ref().map(|t| build_core::key::sha1_hex(t.as_bytes()));
    let base = match (&text_key, base) {
        (Some(text_key), _) => {
            owned_base = keys.base_of(u, text_key);
            if let Some(k) = keys.find(&owned_base, false)
                && take(u, &k, keys.store)?
            {
                return Ok(Made::Stored(k));
            }
            owned_base.as_str()
        }
        (None, Some(b)) => b,
        (None, None) => {
            return Err(Error::format(u.name.clone(), "neither its file nor a way to make its text".to_string()));
        }
    };
    if let Some(dir) = u.object.parent() {
        std::fs::create_dir_all(dir).at(dir)?;
    }
    // (the compiler writes beside the object, under a name of its own; the object takes its place whole)
    let tmp = u.object.with_extension(format!("o.{}.out", std::process::id()));
    let dep = u.object.with_extension("d");
    let _ = std::fs::remove_file(&dep);
    // (what the compile read must not have changed while it ran, or the object is not stored under their keys)
    let started = std::time::SystemTime::now();
    let mut cmd = crate::toolchain::clang(&u.compiler);
    cmd.args(&u.flags);
    if u.depfile {
        cmd.arg("-MD").arg("-MF").arg(&dep);
    }
    match &made_text {
        Some(_) => cmd.args(["-x", "ir", "-c", "-"]).stdin(Stdio::piped()),
        None => cmd.arg("-c").arg(&u.source).stdin(Stdio::null()),
    };
    cmd.arg("-o").arg(&tmp).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child =
        build_core::child::spawn(&mut cmd).map_err(|e| Error::tool(u.compiler.display().to_string(), e.to_string()))?;
    if let Some(t) = ticket {
        t.started(child.id());
    }
    // (the text written on a thread of its own while the compiler's output is read: neither waits on the other)
    let feed = made_text.zip(child.stdin.take()).map(|(text, mut stdin)| {
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        })
    });
    let pipe = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut s);
            }
            String::from_utf8_lossy(&s).into_owned()
        })
    };
    let out = pipe(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let pid = child.id();
    let t = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| Error::tool(u.compiler.display().to_string(), e.to_string()))? {
            break s;
        }
        if t.elapsed() > keys.limit {
            // (killed, its pipes close and the threads reading them end)
            let _ = child.kill();
            let _ = child.wait();
            let (_, said) = (out.join(), err.join().unwrap_or_default());
            if let Some(f) = feed {
                let _ = f.join();
            }
            let _ = std::fs::remove_file(&tmp);
            return Ok(Made::Failed { text: over_limit(&u.name, keys.limit, &said), killed: false, made: text_key });
        }
        *peak = (*peak).max(build_core::memory::tree_mb(pid).unwrap_or(0));
        std::thread::sleep(Duration::from_millis(200));
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if let Some(f) = feed {
        let _ = f.join();
    }
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        let said = if !err.is_empty() { err } else { out };
        let killed = signal_of(&status);
        let mut text = if said.is_empty() {
            format!("Compiler exited with status {} without diagnostics.", status.code().unwrap_or(-1))
        } else {
            clip(&said, 4000)
        };
        if let Some(sig) = killed {
            text.push_str(&format!(
                "\n(the compiler was stopped by the system, signal or status {sig}: out of memory?)"
            ));
        }
        return Ok(Made::Failed { text, killed: killed.is_some(), made: text_key });
    }
    let headers = if changed_since(&u.source, started) {
        None
    } else if u.depfile {
        depfile_headers(&dep, &u.source, keys, started)
    } else {
        Some(Vec::new())
    };
    let made = match headers {
        Some(headers) => {
            let k = key::object(base, &headers);
            keys.store.put(&k, &tmp)?;
            keys.store.add_entry(base, &Entry { key: k.clone(), headers })?;
            Made::Compiled(k)
        }
        None => Made::Unstored,
    };
    let key = match &made {
        Made::Compiled(k) => Some(k.as_str()),
        _ => None,
    };
    stamped(&u.object, key, || {
        build_core::fsx::rename(&tmp, &u.object).map(|()| true).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::io(&u.object, e)
        })
    })?;
    Ok(made)
}

/// Whether the file at `p` was written at or after `t` (its time; a file not there has not).
///
/// What this can miss, by the clocks it compares: `t` is this PC's clock when the compiler started, a file's time is
/// the one its writer's file system set. A write within the file system's time resolution before `t` (2 s on FAT,
/// 100 ns on NTFS, 1 ns on ext4 and btrfs, coarser where the kernel's tick is), a file written by another PC whose
/// clock is behind (a network drive), or an editor that sets a file's time back (a `touch -d`, a restored backup) is
/// not seen as changed: its object is stored under the hash read after the compile. The next build's key of that
/// header is then of its bytes as they are, so a wrong object is used only while the header stays exactly as it was
/// after the write it missed.
fn changed_since(p: &Path, t: std::time::SystemTime) -> bool {
    std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|m| m >= t)
}

/// The headers a compile read, named and hashed, from its depfile (the source itself left out, each once); `None` when
/// the depfile is missing, a header cannot be read now, or one was written since the compile `started` (its hash now
/// is not what the compiler read).
fn depfile_headers(
    dep: &Path,
    source: &Path,
    keys: &Keys,
    started: std::time::SystemTime,
) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(dep).ok()?;
    let src = key::path_text(&std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf()));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in depfile_paths(&text) {
        let abs = std::path::absolute(&p).unwrap_or_else(|_| PathBuf::from(&p));
        if key::path_text(&abs) == src {
            continue;
        }
        let named = keys.anchors.name(&abs.to_string_lossy());
        if changed_since(&abs, started) {
            return None;
        }
        if seen.insert(named.clone()) {
            let h = keys.header(&named)?;
            out.push((named, h));
        }
    }
    Some(out)
}

/// The memory a unit's compile takes at its peak, in MB, a little over: what it was seen to take last time, else an
/// estimate measured on a cold build (clang 23, x86-64): a lifted unit 0.4 GB + 16 MB per MB of IR (a hot one 0.4 GB
/// more: -O2 inlines; a cold one at -O0 0.3 GB + 8 MB per MB); C++ 1 GB; other C 0.4 GB.
pub fn mem_need(u: &Unit, size: u64, measured: Option<u64>) -> u64 {
    if let Some(m) = measured {
        return m + m / 8 + 64;
    }
    let mb = |b: u64| b as f64 / (1u64 << 20) as f64;
    match u.kind {
        Kind::LiftedIr => (400.0 + 16.0 * mb(size)) as u64,
        Kind::LiftedHotIr => (800.0 + 16.0 * mb(size)) as u64,
        Kind::LiftedColdIr => (300.0 + 8.0 * mb(size)) as u64,
        _ if u.source.extension().is_some_and(|e| e == "cpp") => 1000,
        _ => 400,
    }
}

/// A unit's expected compile time, in seconds: what it took last time, else the rates a first build on the
/// development PC showed at 23 jobs (what orders a first build, longest first, so no long unit starts last): a lifted
/// unit 0.45 s per MB of its compile cost (`units.txt`'s, which grows with its largest functions), a hot one 0.65 s,
/// a cold one 0.1 s; C++ 12 s per MB of source, other C 2.5 s, the lift's tables 0.35 s.
pub fn seconds(u: &Unit, size: u64, measured: Option<f64>) -> f64 {
    if let Some(s) = measured {
        return s.max(0.05);
    }
    let mb = |b: u64| b as f64 / (1u64 << 20) as f64;
    let cost = mb(u.listed.as_ref().map_or(size, |l| l.cost.max(size)));
    match u.kind {
        Kind::LiftedIr => 0.45 * cost + 0.5,
        Kind::LiftedHotIr => 0.65 * cost + 0.5,
        Kind::LiftedColdIr => 0.1 * cost + 0.3,
        // (the function table and the import stubs: big arrays, quick)
        Kind::LiftC => 0.35 * mb(size) + 0.2,
        _ if u.source.extension().is_some_and(|e| e == "cpp") => 12.0 * mb(size) + 0.5,
        _ => 2.5 * mb(size) + 0.2,
    }
}

/// About `n` bytes of a failure's text, for the log: its first error and its end (see `clip`).
pub fn short(text: &str, n: usize) -> String {
    clip(text, n)
}

/// How long since `t`, in seconds.
pub fn since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_depfile_s_prerequisites() {
        let d = "obj/a.o: src/a.c \\\n  /usr/include/stdio.h my\\ file.h \\\r\n  cost$$.h C:/x/y.h\n";
        assert_eq!(depfile_paths(d), ["src/a.c", "/usr/include/stdio.h", "my file.h", "cost$.h", "C:/x/y.h"]);
        assert_eq!(depfile_paths("C:/obj/a.o: C:/src/a.c\n"), ["C:/src/a.c"]);
        assert!(depfile_paths("no colon").is_empty());
    }

    #[test]
    fn a_stop_between_an_object_and_its_stamp_leaves_no_stamp_of_another_key() {
        let d = crate::testdir("stamp");
        let obj = d.join("a.o");
        std::fs::write(&obj, b"A").unwrap();
        std::fs::write(stamp_of(&obj), b"key-a").unwrap();
        // (the object B put in, then the run stopped before its stamp)
        let stopped = stamped(&obj, Some("key-b"), || {
            std::fs::write(&obj, b"B").unwrap();
            Err(Error::format("test", "stopped"))
        });
        assert!(stopped.is_err());
        assert!(!up_to_date(&obj, "key-a"), "object B taken for key A's");
        assert!(stamped(&obj, Some("key-b"), || Ok(true)).unwrap());
        assert!(up_to_date(&obj, "key-b"));
    }

    #[test]
    fn a_header_written_during_the_compile_keeps_its_object_out_of_the_store() {
        let d = crate::testdir("moved-header");
        let (h, dep) = (d.join("a.h"), d.join("a.d"));
        std::fs::write(&h, "#define A 1\n").unwrap();
        std::fs::write(&dep, format!("a.o: {} {}\n", d.join("a.c").display(), h.display())).unwrap();
        let (state, store) = (crate::state::State::load(&d), Store::open(&d.join("store")));
        let keys = Keys::new(crate::key::Anchors::new(), &state, &store, None, HashMap::new());
        std::thread::sleep(Duration::from_millis(20));
        let started = std::time::SystemTime::now();
        assert!(depfile_headers(&dep, &d.join("a.c"), &keys, started).is_some(), "unchanged: stored");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&h, "#define A 2\n").unwrap();
        assert!(depfile_headers(&dep, &d.join("a.c"), &keys, started).is_none(), "written meanwhile: not stored");
    }

    #[cfg(unix)]
    #[test]
    fn a_compile_past_its_time_limit_is_stopped_and_names_its_unit() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::testdir("unit-limit");
        let cc = d.join("cc");
        std::fs::write(&cc, "#!/bin/sh\necho 'still at it' >&2\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&cc, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(d.join("a.c"), "int a;\n").unwrap();
        let u = Unit {
            name: "a.c".into(),
            module: crate::plan::Module::Game,
            kind: Kind::Tree,
            compiler: cc,
            source: d.join("a.c"),
            object: d.join("obj/a.o"),
            flags: vec![],
            extra: vec![],
            depfile: false,
            third_party: false,
            listed: None,
        };
        let (state, store) = (State::load(&d), Store::open(&d.join("store")));
        let mut keys = Keys::new(Anchors::new(), &state, &store, None, HashMap::new());
        keys.limit = Duration::from_millis(300);
        let t = Instant::now();
        let made = compile(&u, Some("base"), &keys, None, &mut 0).unwrap();
        assert!(t.elapsed() < Duration::from_secs(20), "waited for the compiler");
        let Made::Failed { text, killed, .. } = made else { panic!("{made:?}") };
        assert!(!killed, "not tried again alone");
        assert!(text.contains("on a.c without finishing") && text.contains("still at it"), "{text}");
        assert!(!d.join("obj/a.o").exists());
    }

    #[test]
    fn a_zero_byte_is_named_where_it_is() {
        let d = crate::testdir("zero");
        let f = d.join("a.c");
        std::fs::write(&f, b"int a;\nint \0b;\n").unwrap();
        assert!(zero_byte(&f).unwrap().contains("a.c:2:5: error: this file has a zero byte"));
        std::fs::write(&f, b"int a;\n").unwrap();
        assert!(zero_byte(&f).is_none());
    }
}
