//! The builder's cache under one budget: everything in it can be made again (docs/dev/reference/build.md, "The
//! cache's budget"), so one least-recently-used rule keeps it to a size, whatever kind each part is.
//!
//! ```text
//! objects    <store>/compile/v1/objects/: the object store, an object a file (build_compile::store)
//! lifts      <store>/lifts/<key>/: whole lifts (lift::whole::store)
//! shaders    <cache>/shaders/<translator stamp>/<driver>/: a profile's game folders' shader caches (crate::shaders)
//! runtimes   <cache>/runtimes/: the upscalers' vendor files (crate::runtimes)
//! tools      <cache>/tools/<name>-<hash>/: the carried build tools, unpacked (crate::sources)
//! sources    <cache>/sources/<hash>/: the carried trees, unpacked (crate::sources)
//! rust       <cache>/rust/<version>-<key>/: the Rust toolchains `from-source` installs
//! work       <cache>/work/<name>-<hash>/: the game folders' work folders (crate::layout)
//! mods       <cache>/mods/: the Mod API's store, each folder in it an entry ([`register`] puts its own rule)
//! ```
//!
//! Each kind lists its entries ([`Kind::entries`]): a path, the bytes it holds (a file hard-linked elsewhere counts
//! its share: the work folders' objects are links of the store's), when it was last used (each store marks that as it
//! did before: an object's time, a lift's `complete`, a folder's `.used`, a work folder's log) and whether something
//! holds it now. [`prune`] removes the least recently used first while the whole is over the budget
//! ([`budget`]: `LONGSHOT_CACHE_MB`, else [`DEFAULT_MB`]), never one used within [`MIN_AGE`] nor one held (a running
//! build's work folder, a lift a build holds, a game's shader caches, and while any build runs its tools, sources and
//! runtimes), nor the most recently used of each kind. What a stopped write left (a `.part`, a `.tmp`) a day old goes
//! whatever the budget. Builds and play prune at their end; `longshot-builder storage` reports and prunes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// The budget's size when `LONGSHOT_CACHE_MB` names none, in MB: 12 GB. The stores' own caps before it came to 9 GB
/// (the object store's 8 GB, the lifts' 1 GB), and what had none comes to about 3 GB more for one game folder (its
/// work folder's 1.7 GB, the unpacked sources and tools' 0.6 GB, the vendors' files): one release's whole cache with
/// room for a second (a version switched back to, a profile's code mods), its shader caches among it.
pub const DEFAULT_MB: u64 = 12 * 1024;
/// The variable that sets the budget, in MB (`0`: only what is held, used within [`MIN_AGE`] or the newest of its
/// kind is kept).
pub const BUDGET_VAR: &str = "LONGSHOT_CACHE_MB";
/// How long an entry is kept whatever the budget once used: a build that starts reads what a build before it made.
pub const MIN_AGE: Duration = Duration::from_secs(3600);
/// How old what a stopped write left (`.part`, `.tmp`) must be to go whatever the budget: another build may be
/// writing it.
pub const STALE_AFTER: Duration = Duration::from_secs(86_400);

/// The budget in bytes: `LONGSHOT_CACHE_MB` (`get`, the environment), else [`DEFAULT_MB`].
pub fn budget(get: impl Fn(&str) -> Option<String>) -> u64 {
    get(BUDGET_VAR).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_MB).saturating_mul(1 << 20)
}

/// Where the kinds are: the builder's cache and the stores' folder (`--store`, `LONGSHOT_STORE`: the cache's `store/`
/// unless named apart, a developer's shared one).
#[derive(Clone, Debug)]
pub struct Places {
    /// The builder's cache (crate::layout::cache_root).
    pub cache: PathBuf,
    /// The stores' folder (crate::layout::store_root).
    pub store: PathBuf,
}

/// One part of the cache the budget can let go of whole.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Its file or folder.
    pub path: PathBuf,
    /// The bytes it holds (a file with other links counts its share).
    pub bytes: u64,
    /// When it was last used.
    pub used: SystemTime,
    /// Something holds it now: never let go.
    pub held: bool,
    /// What a stopped write left: let go once [`STALE_AFTER`] old, whatever the budget.
    pub stale: bool,
}

impl Entry {
    /// An entry of `path`, its bytes and last use as given, not held.
    pub fn new(path: PathBuf, bytes: u64, used: SystemTime) -> Entry {
        Entry { path, bytes, used, held: false, stale: false }
    }
}

/// A kind of what the cache holds: its entries, and how one is let go.
pub trait Kind: Send + Sync {
    /// Its name (`objects`, `lifts`, ..., `mods`): in `storage`'s report.
    fn name(&self) -> &'static str;
    /// Its entries at `at`.
    fn entries(&self, at: &Places) -> Vec<Entry>;
    /// Whether `e` came into use after the survey ([`Kind::entries`]) took it: asked again just before it is removed,
    /// so a build or a game that started meanwhile keeps it. (Lifts and work folders check under their own locks in
    /// [`Kind::remove`].)
    fn in_use_now(&self, at: &Places, e: &Entry) -> bool {
        let _ = (at, e);
        false
    }
    /// `e` let go (removed): `Ok(false)` when it could not be now (taken meanwhile), nothing then removed.
    fn remove(&self, at: &Places, e: &Entry) -> std::io::Result<bool> {
        let _ = at;
        remove_any(&e.path).map(|()| true)
    }
    /// After a prune that let `removed` of its entries go (what goes with them: the object store's manifests).
    fn after(&self, at: &Places, removed: usize) {
        let _ = (at, removed);
    }
}

/// The kinds registered besides the builder's own ([`register`]).
static REGISTERED: Mutex<Vec<Arc<dyn Kind>>> = Mutex::new(Vec::new());

/// A kind of another part of Longshot (the Mod API's store of `<cache>/mods/`) put in the budget, with its own entries
/// and its own answer to what is in use: in place of the builder's own of that name.
pub fn register(kind: Arc<dyn Kind>) {
    let mut r = REGISTERED.lock().unwrap_or_else(|p| p.into_inner());
    r.retain(|k| k.name() != kind.name());
    r.push(kind);
}

/// Every kind, in the report's order: the builder's own, each replaced by one registered of its name.
pub fn kinds() -> Vec<Arc<dyn Kind>> {
    let registered = REGISTERED.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let own: [Arc<dyn Kind>; 9] = [
        Arc::new(Objects),
        Arc::new(Lifts),
        Arc::new(Shaders),
        Arc::new(Runtimes),
        Arc::new(Folders { name: "tools", dir: "tools" }),
        Arc::new(Folders { name: "sources", dir: "sources" }),
        Arc::new(Folders { name: "rust", dir: "rust" }),
        Arc::new(Works),
        Arc::new(Mods),
    ];
    let mut all: Vec<Arc<dyn Kind>> =
        own.into_iter().map(|k| registered.iter().find(|r| r.name() == k.name()).cloned().unwrap_or(k)).collect();
    let more: Vec<Arc<dyn Kind>> =
        registered.into_iter().filter(|r| !all.iter().any(|k| k.name() == r.name())).collect();
    all.extend(more);
    all
}

/// One kind's part of the cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// The kind's name.
    pub kind: String,
    /// Its bytes.
    pub bytes: u64,
    /// Its entries.
    pub entries: usize,
}

/// What [`prune`] found and did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The budget, in bytes.
    pub budget: u64,
    /// Each kind's part, after the prune.
    pub kinds: Vec<Usage>,
    /// The entries let go: (kind, path, bytes).
    pub removed: Vec<(String, PathBuf, u64)>,
}

impl Report {
    /// The bytes the cache holds, after the prune.
    pub fn total(&self) -> u64 {
        self.kinds.iter().map(|k| k.bytes).sum()
    }

    /// The bytes let go.
    pub fn freed(&self) -> u64 {
        self.removed.iter().map(|r| r.2).sum()
    }
}

/// The cache at `at` measured, kind by kind (nothing removed).
pub fn survey(at: &Places, budget: u64) -> Report {
    prune_with(at, budget, SystemTime::now(), &kinds(), false)
}

/// The cache at `at` pruned to `budget` bytes (the module's rule), as at `now`.
pub fn prune(at: &Places, budget: u64) -> Report {
    prune_with(at, budget, SystemTime::now(), &kinds(), true)
}

/// [`prune`] (or with `apply` false, [`survey`]) of `kinds` as at `now`.
pub fn prune_with(at: &Places, budget: u64, now: SystemTime, kinds: &[Arc<dyn Kind>], apply: bool) -> Report {
    let age = |e: &Entry| now.duration_since(e.used).unwrap_or_default();
    // (every entry, its kind's index beside it, the least recently used first)
    let mut all: Vec<(usize, Entry)> = Vec::new();
    for (i, k) in kinds.iter().enumerate() {
        all.extend(k.entries(at).into_iter().map(|e| (i, e)));
    }
    all.sort_by(|a, b| a.1.used.cmp(&b.1.used).then_with(|| a.1.path.cmp(&b.1.path)));
    // (the newest of each kind: kept)
    let mut newest: Vec<Option<usize>> = vec![None; kinds.len()];
    for (n, (i, e)) in all.iter().enumerate() {
        if !e.stale {
            newest[*i] = Some(n);
        }
    }
    let mut total: u64 = all.iter().map(|(_, e)| e.bytes).sum();
    let mut gone = vec![false; all.len()];
    let mut removed = Vec::new();
    if apply {
        for (n, (i, e)) in all.iter().enumerate() {
            let stale = e.stale && age(e) >= STALE_AFTER;
            let over = total > budget && age(e) >= MIN_AGE && newest[*i] != Some(n);
            if e.held || !(stale || over) {
                continue;
            }
            if kinds[*i].in_use_now(at, e) {
                continue;
            }
            if let Ok(true) = kinds[*i].remove(at, e) {
                total = total.saturating_sub(e.bytes);
                gone[n] = true;
                removed.push((kinds[*i].name().to_string(), e.path.clone(), e.bytes));
            }
        }
    }
    for (i, k) in kinds.iter().enumerate() {
        let n = all.iter().zip(&gone).filter(|((j, _), g)| *j == i && **g).count();
        if n > 0 {
            k.after(at, n);
        }
    }
    let mut usage: Vec<Usage> =
        kinds.iter().map(|k| Usage { kind: k.name().to_string(), ..Usage::default() }).collect();
    for (n, (i, e)) in all.iter().enumerate() {
        if !gone[n] {
            usage[*i].bytes += e.bytes;
            usage[*i].entries += 1;
        }
    }
    Report { budget, kinds: usage, removed }
}

/// [`prune`] of this process's cache and stores (`store`: the build's, else crate::layout's), said in the log when it
/// let anything go. What builds and play call at their end.
pub fn prune_here(store: Option<&Path>, r: &build_core::report::Reporter) -> Report {
    let cache = crate::layout::cache_root(crate::env_os);
    let store = store.map_or_else(|| crate::layout::store_root(None, &cache, crate::env), Path::to_path_buf);
    let report = prune(&Places { cache, store }, budget(crate::env));
    if !report.removed.is_empty() {
        r.log(format!(
            "the cache: {} MB of {} MB, within its budget of {} MB: {} least recently used let go ({} MB: {})",
            report.total() >> 20,
            (report.total() + report.freed()) >> 20,
            report.budget >> 20,
            report.removed.len(),
            report.freed() >> 20,
            summary(&report.removed)
        ));
    }
    report
}

/// The file whose write time is the cache's last prune by play ([`prune_if_due`]).
pub const PRUNED: &str = ".pruned";

/// Whether a prune by play is due now, with `last` its last one's time: none yet, over [`MIN_AGE`] ago, or a time ahead
/// of `now` (a clock set back).
fn prune_due(last: Option<SystemTime>, now: SystemTime) -> bool {
    last.is_none_or(|t| now.duration_since(t).map_or(true, |a| a >= MIN_AGE))
}

/// [`prune_here`] at play's end, at most once in [`MIN_AGE`] (measuring the whole cache is slow on a hard drive, and play
/// adds little to it: a build's end prunes every time). None when it was not due.
pub fn prune_if_due(r: &build_core::report::Reporter) -> Option<Report> {
    let cache = crate::layout::cache_root(crate::env_os);
    let mark = cache.join(PRUNED);
    if !prune_due(time_of(&mark), SystemTime::now()) {
        return None;
    }
    let _ = std::fs::create_dir_all(&cache);
    let _ = std::fs::write(&mark, b"");
    Some(prune_here(None, r))
}

/// The kinds of what was let go, with how many of each.
fn summary(removed: &[(String, PathBuf, u64)]) -> String {
    let mut by: Vec<(String, usize)> = Vec::new();
    for (k, _, _) in removed {
        match by.iter_mut().find(|b| &b.0 == k) {
            Some(b) => b.1 += 1,
            None => by.push((k.clone(), 1)),
        }
    }
    by.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")
}

// ---- measuring

/// A file's share of its bytes: the whole for a file of one link, else its part of the links (the rest are another
/// entry's: a work folder's object and the store's).
fn share(p: &Path, m: &std::fs::Metadata) -> u64 {
    m.len() / build_core::fsx::link_count(p, m)
}

/// The bytes under `p` (a file's, or a folder's files'), each by its [`share`]; links are not followed.
pub fn bytes_of(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return share(p, &m);
    }
    std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| bytes_of(&e.path())).sum()
}

/// A file's write time, if it has one.
fn time_of(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The last use of folder `d`: the newest of its marks `marks` that are there, else the folder's own time.
fn used_of(d: &Path, marks: &[&str]) -> SystemTime {
    marks.iter().filter_map(|m| time_of(&d.join(m))).max().or_else(|| time_of(d)).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// `path`'s use marked now: its [`crate::sources::USED`] mark when it is a folder, its own time when a file (best
/// effort: an unmarked entry is only older).
pub fn mark_used(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::write(path.join(crate::sources::USED), b"");
    } else if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Whether the lock file at `path` is held by another process now (its lock can't be shared). No file: not held.
pub fn locked(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(|f| matches!(f.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock)))
}

/// A file or a folder's tree removed; nothing there, nothing to do.
fn remove_any(p: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => build_core::fsx::remove_tree(p).map_err(|e| std::io::Error::other(e.to_string())),
        Ok(_) => std::fs::remove_file(p),
        Err(_) => Ok(()),
    }
}

/// The folders (or files) in `dir`.
fn children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    v.sort();
    v
}

/// Whether a build holds any work folder of the cache now: then the tools, sources and runtimes it reads are held.
fn building(cache: &Path) -> bool {
    children(&cache.join("work")).iter().any(|w| locked(&w.join(build_core::lock::FILE)))
}

/// Whether `name` is what a stopped write left (an unpack's `.part-`, a save's `.part`, a download's `.tmp`).
fn is_partial(name: &str) -> bool {
    name.contains(".part") || name.ends_with(".tmp")
}

// ---- the builder's kinds

/// The object store's objects, each one an entry (its time: its last use, build_compile::store).
struct Objects;

impl Kind for Objects {
    fn name(&self) -> &'static str {
        "objects"
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        let objects = at.store.join("compile").join(build_compile::store::LAYOUT).join("objects");
        let mut out = Vec::new();
        for sub in children(&objects) {
            for f in children(&sub) {
                let Ok(m) = std::fs::symlink_metadata(&f) else { continue };
                let mut e = Entry::new(f.clone(), share(&f, &m), m.modified().unwrap_or(SystemTime::UNIX_EPOCH));
                e.stale = f.to_string_lossy().ends_with(".tmp");
                out.push(e);
            }
        }
        out
    }

    fn after(&self, at: &Places, _: usize) {
        // (a manifest whose every object is gone goes with them)
        build_compile::store::Store::open(&at.store.join("compile")).drop_orphan_manifests();
    }
}

/// The lifts' store's lifts (lift::whole::store), each one an entry: held while a build uses it (`<key>.use`).
struct Lifts;

impl Kind for Lifts {
    fn name(&self) -> &'static str {
        "lifts"
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        let store = at.store.join("lifts");
        let Ok(listed) = lift::whole::store::listed(&store) else { return Vec::new() };
        let mut out: Vec<Entry> = listed
            .into_iter()
            .map(|k| Entry { held: lift::whole::store::used_now(&store, &k.key), ..Entry::new(k.dir, k.bytes, k.used) })
            .collect();
        // (a save that stopped part way)
        for p in children(&store).into_iter().filter(|p| p.to_string_lossy().ends_with(".part")) {
            let mut e = Entry::new(p.clone(), bytes_of(&p), time_of(&p).unwrap_or(SystemTime::UNIX_EPOCH));
            e.stale = true;
            out.push(e);
        }
        out
    }

    fn remove(&self, at: &Places, e: &Entry) -> std::io::Result<bool> {
        let store = at.store.join("lifts");
        let key = e.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        lift::whole::store::let_go(&store, &key).map_err(|err| std::io::Error::other(err.to_string()))
    }
}

/// The shader cache folders (crate::shaders::cache_dir), each `<translator stamp>/<driver>/` an entry: held while a
/// game keeps its program cache's lock there (gpu/vk/vk_progcache.cpp: `longshot_programs.bin.lock`).
struct Shaders;

impl Kind for Shaders {
    fn name(&self) -> &'static str {
        "shaders"
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        let mut out = Vec::new();
        for t in children(&at.cache.join(crate::shaders::SHADERS)).into_iter().filter(|t| t.is_dir()) {
            for d in children(&t).into_iter().filter(|d| d.is_dir()) {
                let used = used_of(&d, &[crate::sources::USED]);
                out.push(Entry { held: shaders_held(&d), ..Entry::new(d.clone(), bytes_of(&d), used) });
            }
        }
        out
    }

    fn in_use_now(&self, _: &Places, e: &Entry) -> bool {
        shaders_held(&e.path) || used_lately(used_of(&e.path, &[crate::sources::USED]))
    }

    fn remove(&self, _: &Places, e: &Entry) -> std::io::Result<bool> {
        remove_any(&e.path)?;
        // (its translator's folder too, once empty)
        if let Some(t) = e.path.parent() {
            let _ = std::fs::remove_dir(t);
        }
        Ok(true)
    }
}

/// Whether a game holds the shader folder `d`'s program cache lock (gpu/vk/vk_progcache.cpp).
fn shaders_held(d: &Path) -> bool {
    ["longshot_programs.bin.lock", "longshot_programs.zst.bin.lock"].iter().any(|l| locked(&d.join(l)))
}

/// Whether a last use `t` is within [`MIN_AGE`] of now (a play's start marks what it uses).
fn used_lately(t: SystemTime) -> bool {
    SystemTime::now().duration_since(t).is_ok_and(|a| a < MIN_AGE) || t > SystemTime::now()
}

/// The upscalers' vendor files (crate::runtimes), each file or SDK folder an entry (its time, or its `.used`): held
/// while a build runs.
struct Runtimes;

impl Kind for Runtimes {
    fn name(&self) -> &'static str {
        "runtimes"
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        let held = building(&at.cache);
        children(&at.cache.join("runtimes"))
            .into_iter()
            .map(|p| {
                let used = if p.is_dir() { Some(used_of(&p, &[crate::sources::USED])) } else { time_of(&p) };
                let mut e = Entry::new(p.clone(), bytes_of(&p), used.unwrap_or(SystemTime::UNIX_EPOCH));
                e.stale = is_partial(&p.file_name().unwrap_or_default().to_string_lossy());
                e.held = held && !e.stale;
                e
            })
            .collect()
    }

    fn in_use_now(&self, at: &Places, e: &Entry) -> bool {
        !e.stale && building(&at.cache)
    }
}

/// The carried trees or tools, unpacked (crate::sources): each folder an entry (its `.used`, else its `.complete`),
/// held while a build runs.
struct Folders {
    name: &'static str,
    dir: &'static str,
}

impl Kind for Folders {
    fn name(&self) -> &'static str {
        self.name
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        let held = building(&at.cache);
        children(&at.cache.join(self.dir))
            .into_iter()
            .filter(|p| p.is_dir())
            .map(|p| {
                let used = used_of(&p, &[crate::sources::USED, crate::sources::COMPLETE]);
                let mut e = Entry::new(p.clone(), bytes_of(&p), used);
                e.stale = is_partial(&p.file_name().unwrap_or_default().to_string_lossy());
                e.held = held && !e.stale;
                e
            })
            .collect()
    }

    fn in_use_now(&self, at: &Places, e: &Entry) -> bool {
        !e.stale
            && (building(&at.cache) || used_lately(used_of(&e.path, &[crate::sources::USED, crate::sources::COMPLETE])))
    }
}

/// The work folders (crate::layout::default_work), each an entry (its log's, times' or note's time): held while a
/// build holds its lock.
struct Works;

impl Kind for Works {
    fn name(&self) -> &'static str {
        "work"
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        children(&at.cache.join("work"))
            .into_iter()
            .filter(|p| p.is_dir())
            .map(|p| {
                let used = used_of(&p, &["build.log", "build_times.json", crate::layout::GAME_NOTE]);
                let mut e = Entry::new(p.clone(), bytes_of(&p), used);
                e.stale = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(crate::layout::GONE));
                e.held = !e.stale && locked(&p.join(build_core::lock::FILE));
                e
            })
            .collect()
    }

    fn remove(&self, at: &Places, e: &Entry) -> std::io::Result<bool> {
        Ok(crate::layout::remove_work(&at.cache.join("work"), &e.path))
    }
}

/// The Mod API's store, `<cache>/mods/`: each folder or file in it an entry (its newest time), until the Mod API
/// registers its own kind of this name ([`register`]) with its own answer to what is in use.
struct Mods;

impl Kind for Mods {
    fn name(&self) -> &'static str {
        "mods"
    }

    fn entries(&self, at: &Places) -> Vec<Entry> {
        children(&at.cache.join("mods"))
            .into_iter()
            .map(|p| {
                let used = used_of(&p, &[crate::sources::USED]);
                Entry::new(p.clone(), bytes_of(&p), used)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kind of the test's own: its entries as given, its removals noted.
    struct Fixed {
        name: &'static str,
        entries: Vec<Entry>,
        removed: Mutex<Vec<PathBuf>>,
    }

    impl Kind for Fixed {
        fn name(&self) -> &'static str {
            self.name
        }

        fn entries(&self, _: &Places) -> Vec<Entry> {
            let removed = self.removed.lock().unwrap();
            self.entries.iter().filter(|e| !removed.contains(&e.path)).cloned().collect()
        }

        fn remove(&self, _: &Places, e: &Entry) -> std::io::Result<bool> {
            self.removed.lock().unwrap().push(e.path.clone());
            Ok(true)
        }
    }

    fn at() -> Places {
        Places { cache: "/c".into(), store: "/c/store".into() }
    }

    #[test]
    fn the_least_recently_used_go_first_while_over_the_budget_and_never_the_recent_the_held_or_the_newest() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        let ago = |h: u64| now - Duration::from_secs(h * 3600);
        let e = |name: &str, mb: u64, hours: u64| Entry::new(PathBuf::from(name), mb << 20, ago(hours));
        let objects = Arc::new(Fixed {
            name: "objects",
            entries: vec![e("o-old", 4, 100), e("o-mid", 4, 50), e("o-new", 4, 2)],
            removed: Mutex::default(),
        });
        let shaders = Arc::new(Fixed {
            name: "shaders",
            entries: vec![
                Entry { held: true, ..e("s-held", 3, 200) },
                e("s-recent", 3, 0),
                Entry { stale: true, ..e("s.part", 1, 30) },
            ],
            removed: Mutex::default(),
        });
        let kinds: Vec<Arc<dyn Kind>> = vec![objects.clone(), shaders.clone()];
        // (measured only: nothing goes)
        let survey = prune_with(&at(), 0, now, &kinds, false);
        assert_eq!(survey.total(), 19 << 20);
        assert!(survey.removed.is_empty());
        // (within the budget: only what a stopped write left a day ago)
        let r = prune_with(&at(), 100 << 20, now, &kinds, true);
        assert_eq!(r.removed.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), [PathBuf::from("s.part")]);
        // (over it: the oldest first, until within; never the held, the last hour's or each kind's newest)
        let r = prune_with(&at(), 15 << 20, now, &kinds, true);
        assert_eq!(r.removed.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), [PathBuf::from("o-old")]);
        assert_eq!(r.total(), 14 << 20);
        let r = prune_with(&at(), 0, now, &kinds, true);
        assert_eq!(r.removed.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), [PathBuf::from("o-mid")]);
        assert_eq!(
            r.kinds,
            [
                Usage { kind: "objects".into(), bytes: 4 << 20, entries: 1 },
                Usage { kind: "shaders".into(), bytes: 6 << 20, entries: 2 }
            ]
        );
    }

    #[test]
    fn play_prunes_at_most_once_an_hour() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        assert!(prune_due(None, now), "never pruned yet");
        assert!(!prune_due(Some(now - Duration::from_secs(59 * 60)), now));
        assert!(prune_due(Some(now - Duration::from_secs(61 * 60)), now));
        assert!(prune_due(Some(now + Duration::from_secs(3600)), now), "a clock set back");
    }

    #[test]
    fn what_came_into_use_after_the_survey_is_kept() {
        /// [`Fixed`] whose entries named `taken-*` came into use after the survey.
        struct Taken(Fixed);
        impl Kind for Taken {
            fn name(&self) -> &'static str {
                self.0.name()
            }
            fn entries(&self, at: &Places) -> Vec<Entry> {
                self.0.entries(at)
            }
            fn in_use_now(&self, _: &Places, e: &Entry) -> bool {
                e.path.to_string_lossy().starts_with("taken-")
            }
            fn remove(&self, at: &Places, e: &Entry) -> std::io::Result<bool> {
                self.0.remove(at, e)
            }
        }
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        let ago = |h: u64| now - Duration::from_secs(h * 3600);
        let e = |name: &str, hours: u64| Entry::new(PathBuf::from(name), 4 << 20, ago(hours));
        let kind = Arc::new(Taken(Fixed {
            name: "tools",
            entries: vec![e("taken-old", 100), e("free-old", 90), e("newest", 2)],
            removed: Mutex::default(),
        }));
        let kinds: Vec<Arc<dyn Kind>> = vec![kind.clone()];
        let r = prune_with(&at(), 0, now, &kinds, true);
        assert_eq!(r.removed.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), [PathBuf::from("free-old")]);
    }

    #[test]
    fn a_kind_registered_takes_the_place_of_the_builder_s_own() {
        struct Theirs;
        impl Kind for Theirs {
            fn name(&self) -> &'static str {
                "mods"
            }
            fn entries(&self, _: &Places) -> Vec<Entry> {
                vec![Entry { held: true, ..Entry::new("/c/mods/x".into(), 5, SystemTime::UNIX_EPOCH) }]
            }
        }
        register(Arc::new(Theirs));
        let all = kinds();
        let names: Vec<&str> = all.iter().map(|k| k.name()).collect();
        assert_eq!(names, ["objects", "lifts", "shaders", "runtimes", "tools", "sources", "rust", "work", "mods"]);
        assert!(all[8].entries(&at())[0].held, "the Mod API's own answer");
        REGISTERED.lock().unwrap().clear();
    }

    #[test]
    fn the_budget_is_the_variable_s_else_twelve_gb() {
        assert_eq!(budget(|_| None), 12 << 30);
        assert_eq!(budget(|k| (k == BUDGET_VAR).then(|| "100".into())), 100 << 20);
        assert_eq!(budget(|k| (k == BUDGET_VAR).then(|| "lots".into())), 12 << 30);
    }

    #[test]
    fn the_builder_s_kinds_measure_the_cache_and_a_held_work_folder_is_kept() {
        let d = crate::scratch("storage-kinds");
        let at = Places { cache: d.join("cache"), store: d.join("cache/store") };
        let put = |rel: &str, n: usize| {
            let p = d.join("cache").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; n]).unwrap();
        };
        put("store/compile/v1/objects/ab/abcd.o", 100);
        put("shaders/t1/0123456789abcdef/longshot_programs.bin", 300);
        put("runtimes/00ff-x.dll", 50);
        put("tools/clang-1234/bin/clang", 70);
        put("sources/5678/.complete", 0);
        put("work/game-1/build.log", 40);
        put("work/game-2/build.log", 60);
        put("mods/m1/a.zip", 10);
        let lock = build_core::lock::WorkLock::take(&d.join("cache/work/game-1")).unwrap();
        let r = prune_with(&at, 0, SystemTime::now() + Duration::from_secs(7200), &kinds(), true);
        let of = |k: &str| r.kinds.iter().find(|u| u.kind == k).unwrap().clone();
        assert_eq!((of("objects").bytes, of("shaders").bytes, of("mods").bytes), (100, 300, 10));
        assert_eq!(of("tools").bytes, 70);
        // (a build holds game-1: its work folder, the tools and the runtimes stay; game-2, older or newer, is one of two)
        assert!(d.join("cache/work/game-1").is_dir());
        assert!(d.join("cache/tools/clang-1234").is_dir() && d.join("cache/runtimes/00ff-x.dll").is_file());
        drop(lock);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
