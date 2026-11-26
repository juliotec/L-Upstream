//! A release's files beside its full file (`longshot-builder dev release fetch-bases`, `deltas`, `files`): the earlier
//! releases' full files of its channel (a nightly's: and the latest stable's), a delta from each, and the manifest a
//! launcher reads (docs/dev/reference/releases.md).
//!
//! Per platform (linux-x64, windows-x64): the full program, [`NAME`] (`.exe` on Windows); a delta from each of the last
//! [`KEEP`] releases of its channel, and a nightly's from the latest stable too ([`also_of`]: a player who switches
//! from stable to nightly), `zstd --patch-from=<that release's full file> --long=LONG` of the full file, [`DELTA`]; and
//! the manifest, [`MANIFEST`]:
//!
//! ```text
//! {"v": 1, "version": <tag>, "channel": "nightly" | "stable", "platform": <p>,
//!  "full": {"name", "size", "sha256"},
//!  "deltas": [{"from": <older tag>, "from_sha256": <its full file's>, "name", "size", "sha256", "long": LONG}],
//!  "stamps": {...}}                         (`longshot-builder stamps`' answer, when given)
//! ```
//!
//! written with its keys sorted, one space a level, non-ASCII escaped (as the Python that wrote it before did). A
//! launcher with an earlier full file whose SHA-256 a delta names downloads that delta, checks it, runs `zstd -d
//! --long=LONG --patch-from=<its file> <delta>` (any zstd 1.4.5 or later, with a window of 2^LONG), and checks the
//! result against "full"'s SHA-256; else it downloads the full file.

use build_driver::tools::{archive, store};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The full file's name: version, platform (+ `.exe` on Windows).
pub const NAME: &str = "longshot-builder-{v}-{p}";
/// A delta's name: from, version, platform.
pub const DELTA: &str = "longshot-builder-{from}-to-{v}-{p}.zst-patch";
/// The manifest's name: version, platform.
pub const MANIFEST: &str = "longshot-builder-{v}-{p}.json";
/// How many earlier releases of the channel a release has deltas from.
pub const KEEP: usize = 5;
/// The delta's window log: 1 GB, past any full file (LongshotLauncher's LongshotBuildRelease.DeltaWindowLog).
pub const LONG: u32 = 30;
/// The platforms and their full file's extension.
pub const PLATFORMS: [(&str, &str); 2] = [("linux-x64", ""), ("windows-x64", ".exe")];
/// The release API's default server and repository.
pub const API: &str = "https://git.magicbean.dev/api/v1";
/// The repository whose releases are read.
pub const REPO: &str = "MagicBeanDev/Longshot";

fn ext(platform: &str) -> &'static str {
    PLATFORMS.iter().find(|(p, _)| *p == platform).map_or("", |(_, e)| e)
}

/// A release's full file's name for a platform.
pub fn full_name(version: &str, platform: &str) -> String {
    NAME.replace("{v}", version).replace("{p}", platform) + ext(platform)
}

/// A release's manifest's name for a platform.
pub fn manifest_name(version: &str, platform: &str) -> String {
    MANIFEST.replace("{v}", version).replace("{p}", platform)
}

fn delta_name(from: &str, version: &str, platform: &str) -> String {
    DELTA.replace("{from}", from).replace("{v}", version).replace("{p}", platform)
}

/// A tag's channel: `nightly-*` nightly, `v*` stable, anything else none.
pub fn channel_of(tag: &str) -> Option<&'static str> {
    if tag.starts_with("nightly-") {
        Some("nightly")
    } else if tag.starts_with('v') {
        Some("stable")
    } else {
        None
    }
}

/// A release's name a channel takes: `nightly-<anything>`, or `vX.Y.Z`.
pub fn on_a_channel(tag: &str) -> bool {
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match (tag.strip_prefix("nightly-"), tag.strip_prefix('v')) {
        (Some(rest), _) => !rest.is_empty(),
        (_, Some(v)) => {
            let parts: Vec<&str> = v.split('.').collect();
            parts.len() == 3 && parts.iter().all(|p| number(p))
        }
        _ => false,
    }
}

/// A file downloaded and checked: (url, to, sha256, size).
pub type GetFile<'a> = &'a dyn Fn(&str, &Path, &str, u64) -> Result<(), String>;

/// How the network is reached: a URL's JSON, and a file downloaded and checked. The tests' stand-ins; [`Net::curl`] for
/// a release.
pub struct Net<'a> {
    pub json: &'a dyn Fn(&str) -> Result<Value, String>,
    pub file: GetFile<'a>,
}

impl Net<'static> {
    /// curl's: build_driver::tools::store's, https only; an `http://` address (RELEASE_UPLOAD_API, the server on the
    /// runners' own network: [`Bases::mirror`]) by a curl of its own, which follows a redirect to https only.
    pub fn curl() -> Net<'static> {
        Net { json: &curl_json, file: &curl_file }
    }
}

fn plain_curl() -> Command {
    let mut c = build_core::child::command("curl");
    c.args(["-q", "--globoff", "--proto", "=http,https", "--proto-redir", "=https", "-fsSL"]);
    c.args(["--retry", "3", "--connect-timeout", "60", "-A", "longshot-tools"]);
    c
}

fn curl_json(url: &str) -> Result<Value, String> {
    if !url.starts_with("http://") {
        return store::json(url);
    }
    let o =
        build_core::child::output(plain_curl().args(["--max-time", "120", "-H", "Accept: application/json"]).arg(url))
            .map_err(|e| format!("curl did not start: {e}"))?;
    if !o.status.success() {
        return Err(format!("{url}: curl failed ({})", o.status));
    }
    serde_json::from_slice(&o.stdout).map_err(|e| format!("{url}: not JSON ({e})"))
}

fn curl_file(url: &str, to: &Path, sha256: &str, size: u64) -> Result<(), String> {
    if !url.starts_with("http://") {
        return store::download_checked(url, to, sha256, size);
    }
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let got = build_core::child::status(
        plain_curl()
            .args(["--speed-limit", "1024", "--speed-time", "60", "--max-filesize", &size.to_string(), "-o"])
            .arg(&part)
            .arg(url),
    )
    .map_err(|e| format!("curl did not start: {e}"))
    .and_then(|s| if s.success() { Ok(()) } else { Err(format!("{url}: curl failed ({s})")) })
    .and_then(|()| archive::sha256_file(&part))
    .and_then(|(sha, n)| {
        if sha == sha256 && n == size {
            std::fs::rename(&part, to).map_err(|e| format!("{}: {e}", to.display()))
        } else {
            Err(format!("{url}: {n} bytes, SHA-256 {sha}; {size} bytes, {sha256} wanted"))
        }
    });
    let _ = std::fs::remove_file(&part);
    got
}

/// The length of `url`'s `scheme://host[:port]`, None without one.
fn origin_len(url: &str) -> Option<usize> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(scheme.len() + 3 + host.len())
}

/// `url` on `server`'s scheme and host (`<server>/api/v1`, its path dropped as Tooling's release.py download() does),
/// when `url` is on `public`'s; else None.
pub fn through(url: &str, public: &str, server: &str) -> Option<String> {
    let (u, p, s) = (origin_len(url)?, origin_len(public)?, origin_len(server)?);
    (url[..u].eq_ignore_ascii_case(&public[..p])).then(|| format!("{}{}", &server[..s], &url[u..]))
}

/// What [`fetch_bases`] looks for.
pub struct Bases<'a> {
    pub platform: &'a str,
    pub channel: &'a str,
    /// This release's own tag, never a base.
    pub not: &'a str,
    pub out: &'a Path,
    pub api: &'a str,
    /// The same server by another address, FORGEJO_UPLOAD_API (`<server>/api/v1`: past Cloudflare, as publish's
    /// downloads and uploads go): each request on `api`'s server tried there first, then at its public address.
    pub mirror: Option<&'a str>,
    pub repo: &'a str,
    pub keep: usize,
    /// Another channel whose latest release with this platform's files is one more base ([`also_of`]: a nightly's
    /// `stable`).
    pub also: Option<&'a str>,
}

/// The channel whose latest release is one more base of a release of `channel`'s: a nightly's, the latest stable (a
/// player on a stable who switches to nightlies gets a delta); a stable's, none.
pub fn also_of(channel: &str) -> Option<&'static str> {
    (channel == "nightly").then_some("stable")
}

/// The last `keep` releases of the channel (but `not`) that published this platform's full file, and with `also` the
/// latest release of that channel that published it, into `out`, newest first (`also`'s last); each said by `say`. A
/// release of the channel whose manifest or full file cannot be had or checked is left out and the next one taken;
/// `also`'s latest left out is no base from `also` (an older one is not what its launchers have). A release list that
/// cannot be read leaves the bases found so far (none: the release has no deltas, and launchers take its full file).
/// Said as a CI warning (`::warning::`), launchers on the missing ones downloading the full file: fewer than `keep`
/// while more of the channel's releases have the platform's files, `also`'s latest left out, or a list that could not
/// be read. No release of `also` with the platform's files yet is no warning. Never fails but for `out` itself.
pub fn fetch_bases(b: &Bases<'_>, net: &Net<'_>, say: &mut dyn FnMut(String)) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(b.out).map_err(|e| format!("{}: {e}", b.out.display()))?;
    // (through the mirror first, then the public address; the mirror's failures counted, said once at the end)
    let missed = std::cell::Cell::new(0usize);
    let mirrored = |url: &str| b.mirror.and_then(|m| through(url, b.api, m));
    let json = |url: &str| {
        if let Some(m) = mirrored(url) {
            match (net.json)(&m) {
                Ok(v) => return Ok(v),
                Err(_) => missed.set(missed.get() + 1),
            }
        }
        (net.json)(url)
    };
    let file = |url: &str, to: &Path, sha: &str, size: u64| {
        if let Some(m) = mirrored(url) {
            match (net.file)(&m, to, sha, size) {
                Ok(()) => return Ok(()),
                Err(_) => missed.set(missed.get() + 1),
            }
        }
        (net.file)(url, to, sha, size)
    };
    // (`also` the channel itself: its latest is among the channel's own bases, not taken twice)
    let also = b.also.filter(|c| *c != b.channel);
    let mut got: Vec<String> = Vec::new();
    // (`also`'s latest with the platform's files: met (a base or left out), and its tag when a base)
    let (mut also_met, mut also_base) = (also.is_none(), None::<String>);
    let (mut candidates, mut unread) = (0usize, false);
    let mut page = 1;
    while (got.len() < b.keep || !also_met) && page < 20 {
        let url = format!("{}/repos/{}/releases?draft=false&limit=50&page={page}", b.api, b.repo);
        let releases = match json(&url) {
            Ok(Value::Array(r)) if !r.is_empty() => r,
            Ok(_) => break,
            Err(e) => {
                say(format!("the release list (page {page}) could not be read: {e}"));
                unread = true;
                break;
            }
        };
        for rel in &releases {
            if got.len() >= b.keep && also_met {
                break;
            }
            let tag = rel.get("tag_name").and_then(Value::as_str).unwrap_or("");
            let ours = channel_of(tag) == Some(b.channel) && got.len() < b.keep;
            let theirs = !also_met && channel_of(tag) == also;
            if tag == b.not || !(ours || theirs) {
                continue;
            }
            let asset = |name: &str| {
                rel.get("assets")?
                    .as_array()?
                    .iter()
                    .find(|a| a.get("name").and_then(Value::as_str) == Some(name))?
                    .get("browser_download_url")?
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .map(str::to_string)
            };
            let (full, manifest) = (full_name(tag, b.platform), manifest_name(tag, b.platform));
            let (Some(full_url), Some(manifest_url)) = (asset(&full), asset(&manifest)) else {
                continue; // (a release from before longshot-builder shipped alone)
            };
            candidates += usize::from(ours);
            let fetched = json(&manifest_url).and_then(|m| {
                let want = m.get("full").ok_or("its manifest has no \"full\"")?;
                let name = want.get("name").and_then(Value::as_str);
                if name != Some(full.as_str()) {
                    return Err(format!("its manifest names {}", name.unwrap_or("no full file")));
                }
                let sha = want.get("sha256").and_then(Value::as_str).ok_or("its manifest names no SHA-256")?;
                let size = want.get("size").and_then(Value::as_u64).ok_or("its manifest names no size")?;
                file(&full_url, &b.out.join(&full), sha, size)
            });
            match fetched {
                Err(e) => say(format!("{tag} left out: {e}")),
                Ok(()) if ours => {
                    say(format!("base {tag}"));
                    got.push(tag.to_string());
                }
                Ok(()) => {
                    say(format!("base {tag}, the latest {}", also.unwrap_or("")));
                    also_base = Some(tag.to_string());
                }
            }
            also_met |= theirs;
        }
        page += 1;
    }
    if missed.get() > 0 {
        say(format!(
            "{} requests through {} failed: their public address instead",
            missed.get(),
            b.mirror.unwrap_or("")
        ));
    }
    let ours = got.len();
    got.extend(also_base.clone());
    say(format!("{} bases: {}", got.len(), if got.is_empty() { "none".into() } else { got.join(", ") }));
    if ours < b.keep && (unread || candidates > ours) {
        let why = if unread {
            "the release list stopped answering".to_string()
        } else {
            format!("{} of the {} {} releases with its files left out", candidates - ours, candidates, b.channel)
        };
        say(format!(
            "::warning::{ours} bases for {}, not {}: {why} (launchers on the others download the full file)",
            b.platform, b.keep
        ));
    }
    if let Some(c) = also.filter(|_| also_base.is_none() && (also_met || unread)) {
        let why = if also_met { "it was left out" } else { "the release list stopped answering" };
        say(format!(
            "::warning::no base from the latest {c} for {}: {why} (launchers on it download the full file)",
            b.platform
        ));
    }
    Ok(got)
}

/// What [`deltas`] makes.
pub struct Deltas<'a> {
    /// The full file (copied into `out` under its release name when it has another).
    pub full: &'a Path,
    pub version: &'a str,
    pub channel: &'a str,
    pub platform: &'a str,
    /// A folder of earlier full files, named as their releases named them.
    pub bases: Option<&'a Path>,
    pub out: &'a Path,
    /// A file of `longshot-builder stamps --output json`'s answer.
    pub stamps: Option<&'a Path>,
}

fn sha(path: &Path) -> Result<(String, u64), String> {
    archive::sha256_file(path)
}

/// The stamps in `text`: one JSON object, or `longshot-builder stamps --output json`'s JSON Lines (the last event with
/// "stamps" in it).
pub fn read_stamps(text: &str) -> Result<Value, String> {
    let found: Vec<Value> = match serde_json::from_str::<Value>(text) {
        Ok(v) => vec![v],
        Err(_) => text
            .lines()
            .filter(|l| l.trim().starts_with('{'))
            .map(|l| serde_json::from_str(l).map_err(|e| format!("not JSON: {e}")))
            .collect::<Result<_, _>>()?,
    };
    let one = found.len() == 1;
    let stamps: Vec<&Value> = found
        .iter()
        .filter(|d| d.is_object() && (d.get("stamps").is_some() || one))
        .map(|d| d.get("stamps").unwrap_or(d))
        .collect();
    match stamps.last() {
        Some(v) if v.is_object() => Ok((*v).clone()),
        _ => Err("holds no stamps".into()),
    }
}

/// `v` with every object's keys in order (as Python's `sort_keys`), whichever map serde_json was built with.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut pairs: Vec<(String, Value)> = m.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().map(|(k, v)| (k, sorted(v))).collect::<Map<_, _>>())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        v => v,
    }
}

/// The manifest's text: keys sorted, one space a level, non-ASCII as `\uXXXX`, a newline at the end.
pub fn manifest_text(v: Value) -> String {
    use serde::Serialize;
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b" "));
    sorted(v).serialize(&mut ser).expect("a Value serializes");
    let mut text = String::new();
    for c in String::from_utf8(out).expect("JSON is UTF-8").chars() {
        if c.is_ascii() {
            text.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                text += &format!("\\u{u:04x}");
            }
        }
    }
    text + "\n"
}

fn zstd(args: &[&std::ffi::OsStr]) -> Result<bool, String> {
    let z = archive::zstd()?;
    build_core::child::status(build_core::child::command(&z).args(args))
        .map(|s| s.success())
        .map_err(|e| format!("zstd did not start: {e}"))
}

/// A delta from each base in `bases` to the full file, whatever its channel (a nightly's from the latest stable too:
/// [`fetch_bases`]), each checked to give the full file back, and the manifest, its deltas in their `from`'s order;
/// each said by `say`. Returns the manifest's path.
pub fn deltas(d: &Deltas<'_>, say: &mut dyn FnMut(String)) -> Result<PathBuf, String> {
    let mut full = std::path::absolute(d.full).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(d.out).map_err(|e| format!("{}: {e}", d.out.display()))?;
    let name = full_name(d.version, d.platform);
    if full.file_name().is_none_or(|n| n.to_string_lossy() != name) {
        let to = d.out.join(&name);
        std::fs::copy(&full, &to).map_err(|e| format!("{}: {e}", full.display()))?;
        full = to;
    }
    let (full_sha, full_size) = sha(&full)?;
    let suffix = format!("-{}{}", d.platform, ext(d.platform));
    let mut names: Vec<String> = match d.bases.filter(|b| b.is_dir()) {
        Some(b) => std::fs::read_dir(b)
            .map_err(|e| format!("{}: {e}", b.display()))?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        None => Vec::new(),
    };
    names.sort();
    let long = format!("--long={LONG}");
    let bases: Vec<(String, String)> = names
        .into_iter()
        .filter_map(|base| {
            let from = base.strip_prefix("longshot-builder-").and_then(|r| r.strip_suffix(&suffix))?.to_string();
            (!from.is_empty() && from != d.version).then_some((base, from))
        })
        .collect();
    // (each base's delta made and checked at once beside the others: zstd at -19 with --long is one core's work, and the
    // release runner has two; the manifest lists them in their `from`'s order whatever order they end in)
    let one = |base: &str, from: &str| -> Result<(Value, u64), String> {
        let path = d.bases.unwrap_or(Path::new("")).join(base);
        let delta = d.out.join(delta_name(from, d.version, d.platform));
        let patch_from = format!("--patch-from={}", path.display());
        let os = |s: &str| std::ffi::OsString::from(s);
        let made = zstd(&[
            &os("-q"),
            &os("-f"),
            &os("-19"),
            &os(&long),
            &os(&patch_from),
            full.as_os_str(),
            &os("-o"),
            delta.as_os_str(),
        ])?;
        if !made {
            return Err(format!("zstd --patch-from {base} failed"));
        }
        let mut back = delta.as_os_str().to_owned();
        back.push(".check");
        let back = PathBuf::from(back);
        let undone = zstd(&[
            &os("-q"),
            &os("-f"),
            &os("-d"),
            &os(&long),
            &os(&patch_from),
            delta.as_os_str(),
            &os("-o"),
            back.as_os_str(),
        ])?;
        if !undone || sha(&back)?.0 != full_sha {
            return Err(format!("the delta from {from} does not give the full file back"));
        }
        let _ = std::fs::remove_file(&back);
        let (delta_sha, delta_size) = sha(&delta)?;
        Ok((
            json!({
                "from": from,
                "from_sha256": sha(&path)?.0,
                "name": delta.file_name().map(|n| n.to_string_lossy().into_owned()),
                "size": delta_size,
                "sha256": delta_sha,
                "long": LONG,
            }),
            delta_size,
        ))
    };
    let made: Vec<Result<(Value, u64), String>> = std::thread::scope(|scope| {
        let running: Vec<_> = bases.iter().map(|(base, from)| scope.spawn(|| one(base, from))).collect();
        running.into_iter().map(|t| t.join().unwrap_or_else(|_| Err("a delta's thread panicked".into()))).collect()
    });
    let mut list = Vec::new();
    for ((_, from), result) in bases.iter().zip(made) {
        let (entry, size) = result?;
        say(format!("delta from {from}: {} KB", size >> 10));
        list.push(entry);
    }
    list.sort_by(|a, b| a["from"].as_str().cmp(&b["from"].as_str()));
    let stamps = match d.stamps {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            read_stamps(&text).map_err(|e| format!("{} {e}", p.display()))?
        }
        None => json!({}),
    };
    let count = list.len();
    let manifest = json!({
        "v": 1,
        "version": d.version,
        "channel": d.channel,
        "platform": d.platform,
        "full": {"name": name, "size": full_size, "sha256": full_sha},
        "deltas": list,
        "stamps": stamps,
    });
    let out = d.out.join(manifest_name(d.version, d.platform));
    std::fs::write(&out, manifest_text(manifest)).map_err(|e| format!("{}: {e}", out.display()))?;
    say(format!("{}: the full file and {count} deltas", out.display()));
    Ok(out)
}

/// What [`files`] makes a release's files from.
pub struct Files<'a> {
    /// The full file, named as the release names it (dist/longshot-builder-<v>-<p>[.exe]).
    pub full: &'a Path,
    pub version: &'a str,
    pub platform: &'a str,
    /// A folder for the bases and the stamps.
    pub work: &'a Path,
    /// What runs the full file (wine, for Windows' on Linux).
    pub runner: Option<&'a str>,
    pub api: &'a str,
    /// [`Bases::mirror`].
    pub mirror: Option<&'a str>,
    pub repo: &'a str,
}

/// A release's files beside its full file, for its channel (`nightly-*`: nightly; `vX.Y.Z`: stable; anything else is
/// on none, and makes none): the stamps (`longshot-builder stamps --output json`, run by `runner`), the deltas from the
/// channel's last [`KEEP`] releases, and a nightly's from the latest stable ([`fetch_bases`]), and the manifest
/// ([`deltas`]).
pub fn files(f: &Files<'_>, net: &Net<'_>, say: &mut dyn FnMut(String)) -> Result<(), String> {
    let channel = match channel_of(f.version) {
        Some(c) if on_a_channel(f.version) => c,
        _ => {
            say(format!("{} is on no channel: no deltas, no manifest", f.version));
            return Ok(());
        }
    };
    let full = std::path::absolute(f.full).map_err(|e| e.to_string())?;
    let out = full.parent().unwrap_or(Path::new(".")).to_path_buf();
    let work = std::path::absolute(f.work).map_err(|e| e.to_string())?;
    let bases = work.join("bases");
    let b = Bases {
        platform: f.platform,
        channel,
        not: f.version,
        out: &bases,
        api: f.api,
        mirror: f.mirror,
        repo: f.repo,
        keep: KEEP,
        also: also_of(channel),
    };
    fetch_bases(&b, net, say)?;
    let stamps = work.join("stamps.json");
    let file = std::fs::File::create(&stamps).map_err(|e| format!("{}: {e}", stamps.display()))?;
    let mut cmd = match f.runner {
        Some(r) => {
            let mut c = build_core::child::command(r);
            c.arg(&full);
            c
        }
        None => build_core::child::command(&full),
    };
    let ran = build_core::child::status(
        cmd.args(["stamps", "--output", "json", "--cache"]).arg(work.join("stamps-cache")).stdout(file),
    )
    .map_err(|e| format!("{} did not start: {e}", full.display()))?;
    if !ran.success() {
        return Err("longshot-builder stamps failed".into());
    }
    let d = Deltas {
        full: f.full,
        version: f.version,
        channel,
        platform: f.platform,
        bases: Some(&bases),
        out: &out,
        stamps: Some(&stamps),
    };
    deltas(&d, say).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const P: &str = "linux-x64";

    fn scratch(name: &str) -> PathBuf {
        // (a folder of each call's own: tests run side by side, some calling this with one name)
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("dev-release-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn release(tag: &str) -> Value {
        json!({"tag_name": tag, "assets": [
            {"name": full_name(tag, P), "browser_download_url": format!("full:{tag}")},
            {"name": manifest_name(tag, P), "browser_download_url": format!("manifest:{tag}")},
        ]})
    }

    /// fetch-bases over stand-in answers (`Err` for a URL that does not answer); the files it wrote and what it said.
    fn run_it(answers: &BTreeMap<String, Result<Value, String>>, failing: &[&str]) -> (Vec<String>, String) {
        run_via(answers, failing, None)
    }

    fn run_via(
        answers: &BTreeMap<String, Result<Value, String>>,
        failing: &[&str],
        mirror: Option<&str>,
    ) -> (Vec<String>, String) {
        run_as(answers, failing, mirror, "nightly", None)
    }

    fn run_as(
        answers: &BTreeMap<String, Result<Value, String>>,
        failing: &[&str],
        mirror: Option<&str>,
        channel: &str,
        also: Option<&str>,
    ) -> (Vec<String>, String) {
        let d = scratch("bases");
        let json = |url: &str| answers.get(url).cloned().unwrap_or_else(|| Err(format!("{url}: no answer")));
        let file = |url: &str, to: &Path, _: &str, _: u64| {
            if failing.contains(&url) {
                return Err("no answer".to_string());
            }
            std::fs::write(to, b"x").map_err(|e| e.to_string())
        };
        let net = Net { json: &json, file: &file };
        let mut said = String::new();
        let b = Bases { platform: P, channel, not: "nightly-9-a", out: &d, api: API, mirror, repo: "r", keep: 5, also };
        fetch_bases(&b, &net, &mut |s| said += &(s + "\n")).unwrap();
        let mut files: Vec<String> =
            std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        let _ = std::fs::remove_dir_all(&d);
        (files, said)
    }

    fn page(releases: Vec<Value>) -> BTreeMap<String, Result<Value, String>> {
        BTreeMap::from([
            (
                "https://git.magicbean.dev/api/v1/repos/r/releases?draft=false&limit=50&page=1".to_string(),
                Ok(Value::Array(releases)),
            ),
            (
                "https://git.magicbean.dev/api/v1/repos/r/releases?draft=false&limit=50&page=2".to_string(),
                Ok(json!([])),
            ),
        ])
    }

    #[test]
    fn a_bad_base_is_left_out_and_the_next_one_taken() {
        // (tests/test_longshot_build_bases.py's, which checked the Python this replaces)
        let tags: Vec<String> = (2..=8).rev().map(|n| format!("nightly-{n}-a")).collect();
        let mut all = vec![release("nightly-9-a"), release("v3.0.0")];
        all.extend(tags.iter().map(|t| release(t)));
        let mut answers = page(all);
        for t in &tags {
            let full = json!({"name": full_name(t, P), "sha256": "0".repeat(64), "size": 1});
            answers.insert(format!("manifest:{t}"), Ok(json!({ "full": full })));
        }
        answers.insert("manifest:nightly-8-a".into(), Err("no answer".into()));
        answers.insert("manifest:nightly-7-a".into(), Err("not JSON".into()));
        answers.insert("manifest:nightly-6-a".into(), Ok(json!({"version": "nightly-6-a"})));
        answers.insert("manifest:nightly-5-a".into(), Ok(json!({"full": {"name": "other", "sha256": "0", "size": 1}})));
        let (files, said) = run_it(&answers, &["full:nightly-4-a"]);
        assert_eq!(files, [full_name("nightly-2-a", P), full_name("nightly-3-a", P)], "{said}");
        for t in ["nightly-8-a", "nightly-7-a", "nightly-6-a", "nightly-5-a", "nightly-4-a"] {
            assert!(said.contains(&format!("{t} left out")), "{said}");
        }
        assert!(!said.contains("nightly-9-a") && !said.contains("v3.0.0"), "not this release, not another channel");
    }

    #[test]
    fn fewer_bases_than_releases_is_a_warning() {
        // (a release whose base could not be had: its launchers download the full file, which CI says)
        let tags = ["nightly-3-a", "nightly-2-a"];
        let mut answers = page(tags.iter().map(|t| release(t)).collect());
        for t in tags {
            let full = json!({"name": full_name(t, P), "sha256": "0".repeat(64), "size": 1});
            answers.insert(format!("manifest:{t}"), Ok(json!({ "full": full })));
        }
        let (files, said) = run_it(&answers, &[]);
        assert_eq!(files.len(), 2, "{said}");
        assert!(!said.contains("::warning::"), "every release a base, though fewer than KEEP: {said}");
        let (files, said) = run_it(&answers, &["full:nightly-3-a"]);
        assert_eq!(files, [full_name("nightly-2-a", P)], "{said}");
        assert!(said.contains("::warning::1 bases for linux-x64, not 5: 1 of the 2 nightly releases"), "{said}");
    }

    /// The release list over pages (an empty one after them), each release's manifest naming its full file.
    fn pages(lists: Vec<Vec<Value>>) -> BTreeMap<String, Result<Value, String>> {
        let mut answers = BTreeMap::new();
        for (i, list) in lists.into_iter().chain([vec![]]).enumerate() {
            for rel in &list {
                let t = rel["tag_name"].as_str().unwrap();
                let full = json!({"name": full_name(t, P), "sha256": "0".repeat(64), "size": 1});
                answers.insert(format!("manifest:{t}"), Ok(json!({ "full": full })));
            }
            let url = format!("https://git.magicbean.dev/api/v1/repos/r/releases?draft=false&limit=50&page={}", i + 1);
            answers.insert(url, Ok(Value::Array(list)));
        }
        answers
    }

    /// A release with only Windows' files.
    fn windows_only(tag: &str) -> Value {
        let p = "windows-x64";
        json!({"tag_name": tag, "assets": [
            {"name": full_name(tag, p), "browser_download_url": format!("full:{tag}")},
            {"name": manifest_name(tag, p), "browser_download_url": format!("manifest:{tag}")},
        ]})
    }

    #[test]
    fn a_nightly_also_has_the_latest_stable_with_its_files() {
        let mut nightlies: Vec<Value> = (2..=9).rev().map(|n| release(&format!("nightly-{n}-a"))).collect();
        nightlies.push(windows_only("v3.2.0")); // (no linux-x64 files: not this platform's latest)
        // (the stable on a later page than the channel's 5: the pages read on for it)
        let answers = pages(vec![nightlies, vec![release("v3.1.0"), release("v3.0.0")]]);
        let (files, said) = run_as(&answers, &[], None, "nightly", also_of("nightly"));
        let mut want: Vec<String> = (4..=8).map(|n| full_name(&format!("nightly-{n}-a"), P)).collect();
        want.push(full_name("v3.1.0", P));
        want.sort();
        assert_eq!(files, want, "{said}");
        assert!(said.contains("base v3.1.0, the latest stable"), "{said}");
        assert!(
            said.contains("6 bases: nightly-8-a, nightly-7-a, nightly-6-a, nightly-5-a, nightly-4-a, v3.1.0"),
            "{said}"
        );
        assert!(!said.contains("v3.0.0") && !said.contains("::warning::"), "only the latest stable: {said}");
    }

    #[test]
    fn the_latest_stable_left_out_is_a_warning_and_no_older_one_taken() {
        let mut answers = pages(vec![vec![release("nightly-3-a"), release("v3.1.0"), release("v3.0.0")]]);
        answers.insert("manifest:v3.1.0".into(), Err("no answer".into()));
        let (files, said) = run_as(&answers, &[], None, "nightly", also_of("nightly"));
        assert_eq!(files, [full_name("nightly-3-a", P)], "{said}");
        assert!(said.contains("v3.1.0 left out"), "{said}");
        assert!(said.contains("::warning::no base from the latest stable for linux-x64: it was left out"), "{said}");
        assert!(!said.contains("v3.0.0"), "an older stable is not what its launchers have: {said}");
    }

    #[test]
    fn no_stable_yet_is_no_warning() {
        let answers = pages(vec![vec![release("nightly-3-a"), windows_only("v3.0.0")]]);
        let (files, said) = run_as(&answers, &[], None, "nightly", also_of("nightly"));
        assert_eq!(files, [full_name("nightly-3-a", P)], "{said}");
        assert!(!said.contains("::warning::"), "{said}");
    }

    #[test]
    fn a_stable_has_its_last_5_stables_only() {
        assert_eq!((also_of("nightly"), also_of("stable")), (Some("stable"), None));
        let mut list: Vec<Value> = vec![release("nightly-8-a")];
        list.extend((0..=6).rev().map(|n| release(&format!("v3.{n}.0"))));
        let answers = pages(vec![list]);
        let want: Vec<String> = (2..=6).map(|n| full_name(&format!("v3.{n}.0"), P)).collect();
        for also in [also_of("stable"), Some("stable")] {
            // (the channel's own latest as `also` too: not taken twice, not a 6th)
            let (files, said) = run_as(&answers, &[], None, "stable", also);
            assert_eq!(files, want, "{said}");
            assert!(said.contains("5 bases: v3.6.0, v3.5.0, v3.4.0, v3.3.0, v3.2.0\n"), "{said}");
        }
    }

    #[test]
    fn the_bases_come_through_the_mirror_then_the_public_address() {
        let public = |t: &str| format!("https://git.magicbean.dev/x/{t}");
        let rel = |t: &str| {
            json!({"tag_name": t, "assets": [
                {"name": full_name(t, P), "browser_download_url": public(&format!("full-{t}"))},
                {"name": manifest_name(t, P), "browser_download_url": public(&format!("manifest-{t}"))},
            ]})
        };
        let manifest = |t: &str| Ok(json!({"full": {"name": full_name(t, P), "sha256": "0", "size": 1}}));
        let list = "repos/r/releases?draft=false&limit=50&page=";
        let mut answers = BTreeMap::from([
            (format!("http://10.0.0.2:3000/api/v1/{list}1"), Ok(json!([rel("nightly-3-a"), rel("nightly-2-a")]))),
            (format!("http://10.0.0.2:3000/api/v1/{list}2"), Ok(json!([]))),
            ("http://10.0.0.2:3000/x/manifest-nightly-3-a".to_string(), manifest("nightly-3-a")),
            (public("manifest-nightly-2-a"), manifest("nightly-2-a")),
        ]);
        let (files, said) = run_via(&answers, &[], Some("http://10.0.0.2:3000/api/v1"));
        assert_eq!(files, [full_name("nightly-2-a", P), full_name("nightly-3-a", P)], "{said}");
        assert!(said.contains("1 requests through http://10.0.0.2:3000/api/v1 failed"), "{said}");
        // (a mirror that does not answer at all: everything from the public address)
        answers.retain(|k, _| !k.starts_with("http://"));
        answers.extend(page(vec![rel("nightly-3-a")]));
        answers.insert(public("manifest-nightly-3-a"), manifest("nightly-3-a"));
        let (files, said) = run_via(&answers, &[], Some("http://10.0.0.2:3000/api/v1"));
        assert_eq!(files, [full_name("nightly-3-a", P)], "{said}");
    }

    #[test]
    fn a_url_through_the_mirror() {
        let (public, mirror) = (API, "http://10.0.0.2:3000/api/v1");
        assert_eq!(
            through("https://git.magicbean.dev/MagicBeanDev/Longshot/releases/download/v1/f", public, mirror)
                .as_deref(),
            Some("http://10.0.0.2:3000/MagicBeanDev/Longshot/releases/download/v1/f")
        );
        assert_eq!(through("https://elsewhere.example/f", public, mirror), None, "only the public server's");
        assert_eq!(through("not a url", public, mirror), None);
    }

    #[test]
    fn an_unreadable_release_list_leaves_no_bases() {
        let answers = BTreeMap::from([(
            "https://git.magicbean.dev/api/v1/repos/r/releases?draft=false&limit=50&page=1".to_string(),
            Err::<Value, String>("down".into()),
        )]);
        let (files, said) = run_it(&answers, &[]);
        assert!(files.is_empty());
        assert!(said.contains("0 bases: none"), "{said}");
        assert!(said.contains("::warning::0 bases for linux-x64, not 5: the release list stopped answering"), "{said}");
    }

    #[test]
    fn names_and_channels() {
        assert_eq!(full_name("v3.2.0", "windows-x64"), "longshot-builder-v3.2.0-windows-x64.exe");
        assert_eq!(full_name("nightly-7-abc", P), "longshot-builder-nightly-7-abc-linux-x64");
        assert_eq!(manifest_name("v3.2.0", "windows-x64"), "longshot-builder-v3.2.0-windows-x64.json");
        assert_eq!(delta_name("v3.1.0", "v3.2.0", P), "longshot-builder-v3.1.0-to-v3.2.0-linux-x64.zst-patch");
        assert_eq!(
            (channel_of("nightly-3-a"), channel_of("v1.2.3"), channel_of("x")),
            (Some("nightly"), Some("stable"), None)
        );
        assert!(on_a_channel("v1.2.3") && on_a_channel("nightly-3-abc"));
        assert!(!on_a_channel("v0.0.0-12-gabc") && !on_a_channel("v1.2") && !on_a_channel("nightly-"));
    }

    #[test]
    fn the_stamps_are_the_last_event_s() {
        let lines = "{\"v\":1,\"event\":\"start\"}\n{\"v\":1,\"event\":\"stamps\",\"stamps\":{\"compiler\":\"c\"}}\n";
        assert_eq!(read_stamps(lines).unwrap(), json!({"compiler": "c"}));
        assert_eq!(read_stamps("{\"compiler\": \"c\"}").unwrap(), json!({"compiler": "c"}), "one object: the stamps");
        assert!(read_stamps("{\"v\":1}\n{\"v\":2}\n").is_err());
        assert!(read_stamps("[]").is_err());
    }

    #[test]
    fn the_manifest_is_written_as_before() {
        // (as Python's json.dump(indent=1, sort_keys=True) wrote it, byte for byte)
        let m = json!({"v": 1, "version": "v1.0.0", "full": {"size": 3, "name": "n"}, "deltas": [], "stamps": {},
                       "x": "é"});
        let want = "{\n \"deltas\": [],\n \"full\": {\n  \"name\": \"n\",\n  \"size\": 3\n },\n \"stamps\": {},\n \
                    \"v\": 1,\n \"version\": \"v1.0.0\",\n \"x\": \"\\u00e9\"\n}\n";
        assert_eq!(manifest_text(m), want);
    }

    #[test]
    fn a_delta_gives_the_full_file_back() {
        if archive::zstd().is_err() {
            eprintln!("no zstd program: skipped");
            return;
        }
        let d = scratch("deltas");
        let bases = d.join("bases");
        std::fs::create_dir_all(&bases).unwrap();
        let old: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut new = old.clone();
        new[1000] ^= 1;
        std::fs::write(bases.join(full_name("v1.0.0", P)), &old).unwrap();
        std::fs::write(bases.join(full_name("v1.1.0", P)), &new).unwrap(); // (this release itself: no delta)
        std::fs::write(bases.join("longshot-builder-v0.9.0-windows-x64.exe"), &old).unwrap(); // (another platform's)
        std::fs::write(d.join("built"), &new).unwrap();
        std::fs::write(d.join("stamps.json"), "{\"event\":\"stamps\",\"stamps\":{\"compiler\":\"c\"}}\n").unwrap();
        let out = d.join("out");
        let dl = Deltas {
            full: &d.join("built"),
            version: "v1.1.0",
            channel: "stable",
            platform: P,
            bases: Some(&bases),
            out: &out,
            stamps: Some(&d.join("stamps.json")),
        };
        let path = deltas(&dl, &mut |_| {}).unwrap();
        let m: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(path, out.join("longshot-builder-v1.1.0-linux-x64.json"));
        assert_eq!(m["full"]["name"], full_name("v1.1.0", P));
        assert_eq!(std::fs::read(out.join(full_name("v1.1.0", P))).unwrap(), new, "copied under its release name");
        assert_eq!(m["deltas"].as_array().unwrap().len(), 1);
        assert_eq!(m["deltas"][0]["from"], "v1.0.0");
        assert_eq!(m["deltas"][0]["long"], LONG);
        assert_eq!(m["stamps"], json!({"compiler": "c"}));
        let keys: Vec<&String> = m.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["channel", "deltas", "full", "platform", "stamps", "v", "version"]);
        assert!(out.join(delta_name("v1.0.0", "v1.1.0", P)).is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_nightly_has_a_delta_from_a_stable() {
        if archive::zstd().is_err() {
            eprintln!("no zstd program: skipped");
            return;
        }
        let d = scratch("cross");
        let bases = d.join("bases");
        std::fs::create_dir_all(&bases).unwrap();
        let old: Vec<u8> = (0..100_000u32).map(|i| (i * 13 % 251) as u8).collect();
        let mut new = old.clone();
        new[500] ^= 1;
        std::fs::write(bases.join(full_name("nightly-4-a", P)), &old).unwrap();
        std::fs::write(bases.join(full_name("v3.0.0", P)), &old).unwrap();
        let full = d.join(full_name("nightly-5-a", P));
        std::fs::write(&full, &new).unwrap();
        let out = d.join("out");
        let dl = Deltas {
            full: &full,
            version: "nightly-5-a",
            channel: "nightly",
            platform: P,
            bases: Some(&bases),
            out: &out,
            stamps: None,
        };
        let m: Value =
            serde_json::from_str(&std::fs::read_to_string(deltas(&dl, &mut |_| {}).unwrap()).unwrap()).unwrap();
        let from: Vec<&str> = m["deltas"].as_array().unwrap().iter().map(|x| x["from"].as_str().unwrap()).collect();
        assert_eq!(from, ["nightly-4-a", "v3.0.0"]);
        assert_eq!(m["channel"], "nightly", "the release's own channel, whatever its bases'");
        assert_eq!(m["deltas"][1]["name"], delta_name("v3.0.0", "nightly-5-a", P));
        assert!(out.join(delta_name("v3.0.0", "nightly-5-a", P)).is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_release_on_no_channel_makes_nothing() {
        let net = Net { json: &|_| Err("no network in a test".into()), file: &|_, _, _, _| Err("none".into()) };
        let mut said = Vec::new();
        let f = Files {
            full: Path::new("dist/longshot-builder-v0.0.0-1-gabc-linux-x64"),
            version: "v0.0.0-1-gabc",
            platform: P,
            work: Path::new("nowhere"),
            runner: None,
            api: API,
            mirror: None,
            repo: REPO,
        };
        files(&f, &net, &mut |s| said.push(s)).unwrap();
        assert_eq!(said, ["v0.0.0-1-gabc is on no channel: no deltas, no manifest"]);
    }
}
