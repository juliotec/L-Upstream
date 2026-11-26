//! A version's program into a version folder (`longshot-builder fetch`; the Longshot Launcher's
//! Toolchains.InstallLongshotBuildAsync, ChainAsync and ApplyAsync, ported):
//!
//! 1. the forge's releases listed; the version asked (a tag, or a channel's newest) found;
//! 2. its `SHA256SUMS` checked against its signature (a trusted key's);
//! 3. its manifest for this platform read, checked by SHA256SUMS: none is `no_builder_in_release` (a make_game.py
//!    release);
//! 4. a delta from a kept program (`--from`: installed version folders) when the manifest lists one, else a chain of
//!    up to 5 deltas through the releases just before it ([`chain::find`], [`chain::look_back`]), else the full file;
//!    every intermediate file and the result checked against its release's manifest (size, SHA-256), each step
//!    deleted once used; any failure of a delta or a chain falls back to the full file;
//! 5. made a windowed program on Windows ([`crate::swap::set_subsystem`]), executable on Linux, then put in place
//!    atomically as `<out>/Longshot.exe` or `<out>/Longshot.x86_64`, with [`RECORD`] beside it.
//!
//! [`RECORD`] says which release the folder's program is, and its full file's SHA-256 (the one released, before it was
//! made windowed): what the next fetch's `--from` takes it as, with no hashing.

use crate::chain::{self, Files, Hop};
use crate::forge::{Forge, Listed, Release};
use crate::manifest::{MAX_MANIFEST, Manifest, manifest_name};
use crate::{Cancel, Channel, FetchError, sha256_file};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The version folder's record of its program.
pub const RECORD: &str = "longshot_release.json";
/// The folder the intermediate files go in, inside the version folder (deleted at the end, whatever happens).
pub const STAGING: &str = ".fetch";

/// What to fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
    /// This version.
    Tag(String),
    /// A channel's newest.
    Channel(Channel),
}

impl Want {
    /// `stable`, `nightly`, or a version's name.
    #[must_use]
    pub fn parse(s: &str) -> Want {
        Channel::parse(s).map_or_else(|| Want::Tag(s.to_string()), Want::Channel)
    }
}

/// A fetch.
#[derive(Clone, Debug)]
pub struct Request {
    /// What.
    pub want: Want,
    /// The version folder its program goes into.
    pub out: PathBuf,
    /// Installed version folders whose programs may be a delta's base.
    pub from: Vec<PathBuf>,
    /// The platform (`linux-x64`, `windows-x64`).
    pub platform: String,
}

/// How far a fetch is.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// `list`, `manifest`, `delta` (a delta downloaded), `patch` (applied), `full` (the full file downloaded),
    /// `check`, `place`.
    pub step: &'static str,
    /// The hop (1-based) of a chain of `hops`, 0 when not a delta's step.
    pub hop: usize,
    /// The chain's hops (1 for a single delta, 0 for the full file).
    pub hops: usize,
    /// Bytes so far of all the step's downloads.
    pub bytes: u64,
    /// Bytes in all (0 when not a download).
    pub total: u64,
}

/// How the program came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// The version folder had it already (its record and its bytes).
    Present,
    /// One delta.
    Delta,
    /// A chain of deltas.
    Chain,
    /// The full file.
    Full,
}

impl How {
    /// Its word.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            How::Present => "present",
            How::Delta => "delta",
            How::Chain => "chain",
            How::Full => "full",
        }
    }
}

/// A fetch done.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// The release.
    pub manifest: Manifest,
    /// The program put in place.
    pub program: PathBuf,
    /// How.
    pub how: How,
    /// The version its delta or chain started from.
    pub from: Option<String>,
    /// The releases a chain passed through, oldest first.
    pub through: Vec<String>,
    /// The bytes downloaded for it (the manifest, deltas, full file).
    pub downloaded: u64,
}

/// A kept program that may be a delta's base: its version when its record says it, the SHA-256 of the full file it
/// is (as released), its path, and whether it was made windowed.
#[derive(Clone, Debug)]
pub struct Base {
    /// Its version, from its record.
    pub version: Option<String>,
    /// The released full file's SHA-256 it is.
    pub sha256: String,
    /// The program.
    pub path: PathBuf,
}

/// A version folder's record: (version, platform, the full file's SHA-256 and size, the program's name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The release.
    pub version: String,
    /// Its channel.
    pub channel: String,
    /// The platform.
    pub platform: String,
    /// The released full file's SHA-256 (before it was made windowed).
    pub sha256: String,
    /// Its size.
    pub size: u64,
    /// The program's name in the folder.
    pub program: String,
    /// The manifest's stamps.
    pub stamps: std::collections::BTreeMap<String, String>,
}

impl Record {
    /// Version folder `dir`'s record, None when it has none that reads.
    #[must_use]
    pub fn read(dir: &Path) -> Option<Record> {
        let text = std::fs::read_to_string(dir.join(RECORD)).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let program = s("program").filter(|p| !p.contains(['/', '\\']) && !p.is_empty())?;
        Some(Record {
            version: s("version")?,
            channel: s("channel").unwrap_or_default(),
            platform: s("platform")?,
            sha256: v.get("full")?.get("sha256")?.as_str()?.to_string(),
            size: v.get("full")?.get("size")?.as_u64()?,
            program,
            stamps: v
                .get("stamps")
                .and_then(Value::as_object)
                .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
                .unwrap_or_default(),
        })
    }

    fn of(m: &Manifest, program: &str) -> Value {
        json!({"v": 1, "version": m.version, "channel": m.channel.name(), "platform": m.platform,
               "full": {"name": m.full.name, "size": m.full.size, "sha256": m.full.sha256},
               "program": program, "stamps": m.stamps})
    }
}

/// A program's bytes as the full file it was released as: a Windows program made windowed back to a console one
/// when that is what gives `sha256` (Linux's are the full file byte for byte).
fn as_released(path: &Path, sha256: &str) -> Result<Vec<u8>, String> {
    let mut bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if crate::sha256_hex(&bytes) == sha256 {
        return Ok(bytes);
    }
    if crate::swap::set_subsystem(&mut bytes, 3) == Ok(2) && crate::sha256_hex(&bytes) == sha256 {
        return Ok(bytes);
    }
    Err(format!("{} is not the file its record names", path.display()))
}

/// The SHA-256 of the full file the program at `path` is: its own, or, for a windowed Windows program, the console
/// one's it was made from.
fn released_sha(path: &Path) -> Option<String> {
    let mut bytes = std::fs::read(path).ok()?;
    if crate::swap::set_subsystem(&mut bytes, 3) == Ok(2) {
        return Some(crate::sha256_hex(&bytes));
    }
    Some(crate::sha256_hex(&bytes))
}

/// The bases the folders `from` keep: each one's program, by its record (no hashing), else its `Longshot.exe` or
/// `Longshot.x86_64` hashed (a version folder from before records, a standalone copy).
#[must_use]
pub fn bases(from: &[PathBuf], platform: &str) -> Vec<Base> {
    let mut out = Vec::new();
    for dir in from {
        if let Some(r) = Record::read(dir).filter(|r| r.platform == platform) {
            let path = dir.join(&r.program);
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                out.push(Base { version: Some(r.version), sha256: r.sha256, path });
                continue;
            }
        }
        // (a version the Longshot Launcher installed before it asked the builder: its toolchain.json's file and SHA-256)
        if let Some((tag, sha, path)) = launcher_record(dir) {
            out.push(Base { version: Some(tag), sha256: sha, path });
            continue;
        }
        let path = if dir.is_file() { dir.clone() } else { dir.join(crate::program_name(platform)) };
        if let Some(sha) = path.is_file().then(|| released_sha(&path)).flatten() {
            out.push(Base { version: None, sha256: sha, path });
        }
    }
    out
}

/// The Longshot Launcher's own record of a version it installed (`toolchain.json`: `tag`, `run`, `sha256`), when its
/// file is there.
fn launcher_record(dir: &Path) -> Option<(String, String, PathBuf)> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("toolchain.json")).ok()?).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let run = s("run").filter(|r| !r.is_empty() && !r.contains(['/', '\\']))?;
    let sha = s("sha256").filter(|x| x.len() == 64)?.to_ascii_lowercase();
    let path = dir.join(run);
    path.is_file().then(|| (s("tag").unwrap_or_default(), sha, path))
}

/// What a fetch is told as it goes: its progress, and lines for the log.
pub struct Tell<'a> {
    /// How far it is.
    pub progress: &'a mut dyn FnMut(Progress),
    /// A line for the log.
    pub say: &'a mut dyn FnMut(String),
}

/// How a delta is applied: [`crate::patch::apply`], or a test's.
pub type Patcher = dyn Fn(Vec<u8>, &Path, &Path, &Cancel) -> Result<(), String>;

/// The release `want` names: a channel's newest from `list`, or the tag (from the list, else asked by its tag).
fn find_release(forge: &Forge<'_>, list: &[Listed], want: &Want) -> Result<Listed, FetchError> {
    match want {
        Want::Channel(c) => crate::forge::on_channel(list, *c).first().map(|r| (*r).clone()).ok_or_else(|| {
            FetchError::new(
                "no_release_on_channel",
                format!("git.magicbean.dev has no {} release of Longshot", c.name()),
            )
        }),
        Want::Tag(t) => match list.iter().find(|r| &r.tag == t) {
            Some(r) => Ok(r.clone()),
            None => forge.by_tag(t),
        },
    }
}

/// A release's manifest for `platform`: None when it has none (a make_game.py release).
fn manifest_of(forge: &Forge<'_>, release: &Release, platform: &str) -> Result<Option<Manifest>, FetchError> {
    let tag = &release.listed.tag;
    let Some(file) = release.file(&manifest_name(tag, platform)) else { return Ok(None) };
    let bytes = forge.fetch_small(&file, MAX_MANIFEST)?;
    let text = String::from_utf8(bytes).map_err(|_| FetchError::new("release_refused", "the manifest is not text"))?;
    Manifest::parse(&text, tag, platform).map(Some).map_err(|e| FetchError::new("release_refused", e))
}

fn files_of(release: &Release) -> Files {
    release.listed.assets.iter().map(|a| (a.name.clone(), a.url.clone())).collect()
}

/// `url` downloaded into `to`, checked to be `size` bytes of SHA-256 `sha256`.
fn download_checked(
    forge: &Forge<'_>,
    url: &str,
    to: &Path,
    size: u64,
    sha256: &str,
    progress: &mut dyn FnMut(u64),
) -> Result<(), FetchError> {
    forge.net().download(url, to, size, progress).map_err(|e| {
        if e.cancelled {
            FetchError::cancelled()
        } else {
            FetchError::new("download_failed", format!("{url} could not be downloaded: {}", e.text))
        }
    })?;
    match sha256_file(to) {
        Ok((sha, n)) if n == size && sha == sha256 => Ok(()),
        Ok(_) => Err(FetchError::new(
            "download_failed",
            format!(
                "{} is not the file its release names (size or SHA-256)",
                to.file_name().unwrap_or_default().to_string_lossy()
            ),
        )),
        Err(e) => Err(FetchError::io(to, &e)),
    }
}

/// Fetches `req` from `forge`: the program in place and its record beside it, or why not (after the full file was
/// tried, when a delta or a chain failed).
pub fn fetch(
    forge: &Forge<'_>,
    req: &Request,
    patcher: &Patcher,
    cancel: &Cancel,
    tell: &mut Tell<'_>,
) -> Result<Fetched, FetchError> {
    let p = &req.platform;
    (tell.progress)(Progress { step: "list", hop: 0, hops: 0, bytes: 0, total: 0 });
    let list = forge.list()?;
    let listed = find_release(forge, &list, &req.want)?;
    let tag = listed.tag.clone();
    let release = forge.verified(&listed)?;
    (tell.progress)(Progress { step: "manifest", hop: 0, hops: 0, bytes: 0, total: 0 });
    let manifest = manifest_of(forge, &release, p)?.ok_or_else(|| {
        FetchError::new(
            "no_builder_in_release",
            format!(
                "The release {tag} has no longshot-builder for {p} (no {}): it is a release of the earlier kind, \
                 built with make_game.py, which this builder does not install.",
                manifest_name(&tag, p)
            ),
        )
    })?;
    let name = crate::program_name(p);
    let program = req.out.join(name);
    // (already here: its record says this release, and its bytes are it)
    if let Some(r) = Record::read(&req.out)
        && r.version == tag
        && r.platform == *p
        && r.sha256 == manifest.full.sha256
        && program.is_file()
        && as_released(&program, &manifest.full.sha256).is_ok()
    {
        return Ok(Fetched { manifest, program, how: How::Present, from: None, through: vec![], downloaded: 0 });
    }
    std::fs::create_dir_all(&req.out).map_err(|e| FetchError::io(&req.out, &e))?;
    let staging = req.out.join(STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| FetchError::io(&staging, &e))?;
    let r = (|| {
        let made = staging.join(format!("{name}.part"));
        let (how, from, through, downloaded) =
            make(forge, &list, &release, &manifest, req, &made, patcher, cancel, tell)?;
        place(&manifest, &made, &program, p)?;
        (tell.progress)(Progress { step: "place", hop: 0, hops: 0, bytes: 0, total: 0 });
        Ok(Fetched { manifest: manifest.clone(), program: program.clone(), how, from, through, downloaded })
    })();
    let _ = std::fs::remove_dir_all(&staging);
    r
}

type Made = (How, Option<String>, Vec<String>, u64);

/// The release's full file into `made`: by a delta or a chain when one fits and applies, else downloaded whole.
#[allow(clippy::too_many_arguments)]
fn make(
    forge: &Forge<'_>,
    list: &[Listed],
    release: &Release,
    manifest: &Manifest,
    req: &Request,
    made: &Path,
    patcher: &Patcher,
    cancel: &Cancel,
    tell: &mut Tell<'_>,
) -> Result<Made, FetchError> {
    let p = &req.platform;
    let tag = &manifest.version;
    let bases = bases(&req.from, p);
    let kept = |_: &str, sha: &str| bases.iter().any(|b| b.sha256 == sha);
    let files = files_of(release);
    let single = manifest.choose(&kept).and_then(|d| {
        let url = files.get(&d.file.name)?;
        Some(vec![Hop { into: manifest.clone(), delta: d.clone(), url: url.clone() }])
    });
    let chain = match single {
        Some(c) => Some(c),
        None if !bases.is_empty() => {
            let back = chain::look_back(list, tag);
            let mut earlier = |v: &str| -> Result<Option<(Manifest, Files)>, String> {
                if !back.iter().any(|b| b == v) {
                    return Ok(None);
                }
                let Some(l) = list.iter().find(|r| r.tag == v) else { return Ok(None) };
                let r = forge.verified(l).map_err(|e| e.text)?;
                Ok(manifest_of(forge, &r, p).map_err(|e| e.text)?.map(|m| (m, files_of(&r))))
            };
            chain::find(manifest, &files, &kept, &mut earlier, tell.say)
        }
        None => None,
    };
    if let Some(chain) = chain {
        let from = chain[0].delta.from.clone();
        match apply_chain(forge, &chain, &bases, made, patcher, cancel, tell) {
            Ok(downloaded) => {
                let through: Vec<String> = chain[..chain.len() - 1].iter().map(|h| h.into.version.clone()).collect();
                (tell.say)(if chain.len() == 1 {
                    format!("longshot-builder {tag}: made from {from}'s with a {} KB delta", downloaded >> 10)
                } else {
                    format!(
                        "longshot-builder {tag}: made from {from}'s through {} with {} deltas, {} KB",
                        through.join(", "),
                        chain.len(),
                        downloaded >> 10
                    )
                });
                let how = if chain.len() == 1 { How::Delta } else { How::Chain };
                return Ok((how, Some(from), through, downloaded));
            }
            Err(e) if e.code == "cancelled" => return Err(e),
            Err(e) => {
                (tell.say)(format!(
                    "longshot-builder {tag}: the {} from {from} did not apply ({}); the full file is downloaded",
                    if chain.len() == 1 { "delta" } else { "chain of deltas" },
                    e.text
                ));
                let _ = std::fs::remove_file(made);
            }
        }
    }
    let full = &manifest.full;
    let url = &release
        .listed_file(&full.name)
        .ok_or_else(|| FetchError::new("release_refused", format!("{tag} has no {}", full.name)))?
        .url;
    let total = full.size;
    (tell.progress)(Progress { step: "full", hop: 0, hops: 0, bytes: 0, total });
    download_checked(forge, url, made, full.size, &full.sha256, &mut |b| {
        (tell.progress)(Progress { step: "full", hop: 0, hops: 0, bytes: b, total })
    })?;
    (tell.say)(format!("longshot-builder {tag}: downloaded whole ({} MB)", full.size >> 20));
    Ok((How::Full, None, vec![], full.size))
}

/// A chain applied in order into `exe`: each delta downloaded and applied to the file before it (first the kept
/// base, checked by its bytes), each result checked by its release's manifest; every step deleted once the next is
/// made. The bytes downloaded.
fn apply_chain(
    forge: &Forge<'_>,
    chain: &[Hop],
    bases: &[Base],
    exe: &Path,
    patcher: &Patcher,
    cancel: &Cancel,
    tell: &mut Tell<'_>,
) -> Result<u64, FetchError> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let first = &chain[0].delta;
    let base = bases
        .iter()
        .find(|b| b.sha256 == first.from_sha256)
        .ok_or_else(|| FetchError::new("io", format!("{}'s program is not there any more", first.from)))?;
    // (the base's bytes, not only its record: a file changed since is no base)
    let mut basis = as_released(&base.path, &first.from_sha256).map_err(|e| FetchError::new("io", e))?;
    let total: u64 = chain.iter().map(|h| h.delta.file.size).sum();
    let hops = chain.len();
    let mut done = 0u64;
    let mut step: Option<PathBuf> = None;
    for (i, hop) in chain.iter().enumerate() {
        if cancel.is_set() {
            return Err(FetchError::cancelled());
        }
        let part = dir.join(format!("{}.zst.part", hop.delta.file.sha256));
        let before = done;
        (tell.progress)(Progress { step: "delta", hop: i + 1, hops, bytes: before, total });
        download_checked(forge, &hop.url, &part, hop.delta.file.size, &hop.delta.file.sha256, &mut |b| {
            (tell.progress)(Progress { step: "delta", hop: i + 1, hops, bytes: before + b, total })
        })?;
        done += hop.delta.file.size;
        let last = i + 1 == hops;
        let output = if last { exe.to_path_buf() } else { dir.join(format!("{}.step", hop.into.full.sha256)) };
        (tell.progress)(Progress { step: "patch", hop: i + 1, hops, bytes: done, total });
        patcher(std::mem::take(&mut basis), &part, &output, cancel)
            .map_err(|e| if cancel.is_set() { FetchError::cancelled() } else { FetchError::new("bad_delta", e) })?;
        let _ = std::fs::remove_file(&part);
        (tell.progress)(Progress { step: "check", hop: i + 1, hops, bytes: done, total });
        let want = &hop.into.full;
        match sha256_file(&output) {
            Ok((sha, n)) if n == want.size && sha == want.sha256 => {}
            _ => {
                return Err(FetchError::new(
                    "bad_delta",
                    format!("{} made from the delta is not the file the release names (size or SHA-256)", want.name),
                ));
            }
        }
        if let Some(s) = step.take() {
            let _ = std::fs::remove_file(s);
        }
        if !last {
            basis = std::fs::read(&output).map_err(|e| FetchError::io(&output, &e))?;
            step = Some(output);
        }
    }
    Ok(done)
}

/// The full file at `made` put in place as `program`: windowed on Windows, executable on Linux, renamed over what was
/// there, then its record written.
fn place(manifest: &Manifest, made: &Path, program: &Path, platform: &str) -> Result<(), FetchError> {
    if platform == "windows-x64" {
        let mut bytes = std::fs::read(made).map_err(|e| FetchError::io(made, &e))?;
        crate::swap::set_subsystem(&mut bytes, 2).map_err(|e| FetchError::new("release_refused", e))?;
        std::fs::write(made, bytes).map_err(|e| FetchError::io(made, &e))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(made, std::fs::Permissions::from_mode(0o755)).map_err(|e| FetchError::io(made, &e))?;
    }
    let name = program.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let dir = program.parent().unwrap_or(Path::new("."));
    // (the record says what it is only once it is there: no record while the program moves; then the new one)
    let _ = std::fs::remove_file(dir.join(RECORD));
    crate::swap::while_held(|| std::fs::rename(made, program), &std::thread::sleep)
        .map_err(|e| FetchError::io(program, &e))?;
    let text = serde_json::to_string_pretty(&Record::of(manifest, &name)).unwrap_or_default() + "\n";
    let part = dir.join(format!("{RECORD}.part"));
    std::fs::write(&part, text)
        .and_then(|()| std::fs::rename(&part, dir.join(RECORD)))
        .map_err(|e| FetchError::io(&dir.join(RECORD), &e))
}

#[cfg(test)]
pub(crate) mod tests;
