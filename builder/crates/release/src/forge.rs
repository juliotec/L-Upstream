//! The forge's releases (docs/dev/reference/releases.md; the Longshot Launcher's Core/Releases.cs, ReleaseManifest.cs
//! and CdnFetcher.cs, ported): the published releases listed, a channel's newest, and a release's files each with the
//! SHA-256 its signed `SHA256SUMS` gives.
//!
//! The addresses, all anonymous `GET`s over https (no token is ever sent, so no redirect can carry one):
//!
//! ```text
//! https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/releases?limit=50&page=<n>   the list, 50 a page
//! https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/releases/tags/<tag>         one release
//! <the asset's browser_download_url>   https://git.magicbean.dev/MagicBeanDev/Longshot/releases/download/<tag>/<file>
//! ```
//!
//! Each is asked of Cloudflare's edge first, `https://git-cdn.magicbean.dev` with the same path (a list kept a minute
//! there; a file served from the forge's R2 store), then of git.magicbean.dev itself when the edge fails. A download
//! asked of git.magicbean.dev carries `cdn=0`, or the forge's own redirect rule would send it back to the edge that
//! just failed; it then redirects to its storage's signed link, which curl follows (https only). Cloudflare's 100 MB
//! limit is on uploads alone: downloads of any size go through the edge. `LONGSHOT_NO_CDN=1` (or the launcher's
//! `LONGSHOT_LAUNCHER_NO_CDN=1`) asks git.magicbean.dev alone.

use crate::trust::{self, Key};
use crate::{Cancel, FetchError};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// The forge.
pub const ORIGIN: &str = "https://git.magicbean.dev";
/// Cloudflare's edge in front of its release files and lists.
pub const CDN: &str = "https://git-cdn.magicbean.dev";
/// The repository whose releases these are.
pub const REPO: &str = "MagicBeanDev/Longshot";
/// The query that keeps git.magicbean.dev from redirecting a download to the edge.
pub const NO_REDIRECT: &str = "cdn=0";
/// The variables that turn the edge off.
pub const NO_CDN_VARIABLES: [&str; 2] = ["LONGSHOT_NO_CDN", "LONGSHOT_LAUNCHER_NO_CDN"];
/// The release list's pages read at most (50 a page).
pub const MAX_PAGES: u32 = 20;
/// The largest answer of the API read.
pub const MAX_TEXT: u64 = 16 << 20;
/// The largest file a release may have.
pub const MAX_FILE: u64 = 16 << 30;
/// `SHA256SUMS` and its signature.
pub const SUMS: &str = "SHA256SUMS";
/// `SHA256SUMS.sig`.
pub const SIG: &str = "SHA256SUMS.sig";

/// Why the network did not give what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetError {
    /// Why, in words.
    pub text: String,
    /// It was cancelled.
    pub cancelled: bool,
}

impl NetError {
    /// A failure.
    #[must_use]
    pub fn new(text: impl Into<String>) -> NetError {
        NetError { text: text.into(), cancelled: false }
    }
}

/// The network as the forge is asked: curl's ([`crate::curl::Curl`]), or a test's fake server.
pub trait Net {
    /// `url`'s answer as text, at most `max` bytes.
    fn text(&self, url: &str, max: u64) -> Result<String, NetError>;
    /// `url` into the file `to` (made, or emptied first), at most `max` bytes; `progress` told the bytes so far.
    fn download(&self, url: &str, to: &Path, max: u64, progress: &mut dyn FnMut(u64)) -> Result<(), NetError>;
}

/// A release file's or package's link on git.magicbean.dev, on the edge; None for any other link.
#[must_use]
pub fn cdn_of(url: &str) -> Option<String> {
    let path = url.strip_prefix(ORIGIN)?;
    if !path.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = path.split(['?', '#']).next().unwrap_or("").split('/').collect();
    let release =
        parts.len() > 4 && !parts[1].is_empty() && !parts[2].is_empty() && parts[3..5] == ["releases", "download"];
    (release || path.starts_with("/api/packages/")).then(|| format!("{CDN}{path}"))
}

/// A release list's (any page) or a release's (by its tag) link on git.magicbean.dev's API, on the edge; None for any
/// other.
#[must_use]
pub fn list_cdn_of(url: &str) -> Option<String> {
    let path = url.strip_prefix(ORIGIN)?;
    let parts: Vec<&str> = path.split('?').next().unwrap_or("").split('/').collect();
    let ok = |p: &[&str]| p.iter().all(|s| !s.is_empty());
    let list = match parts.as_slice() {
        ["", "api", "v1", "repos", o, r, "releases"] => ok(&[o, r]),
        ["", "api", "v1", "repos", o, r, "releases", "tags", t] => ok(&[o, r, t]),
        _ => false,
    };
    list.then(|| format!("{CDN}{path}"))
}

/// A release file's link as git.magicbean.dev answers it itself, not redirected to the edge: [`NO_REDIRECT`] in its
/// query. Any other link as it is.
#[must_use]
pub fn direct(url: &str) -> String {
    if cdn_of(url).is_none() {
        return url.to_string();
    }
    let (link, fragment) = url.split_at(url.find('#').unwrap_or(url.len()));
    let query = link.split_once('?').map_or("", |(_, q)| q);
    if query.split('&').any(|p| p == NO_REDIRECT) {
        return url.to_string();
    }
    let sep = if link.ends_with('?') {
        ""
    } else if link.contains('?') {
        "&"
    } else {
        "?"
    };
    format!("{link}{sep}{NO_REDIRECT}{fragment}")
}

/// A [`Net`] that asks the edge first, then git.magicbean.dev ([`cdn_of`], [`list_cdn_of`], [`direct`]); with the
/// edge off, git.magicbean.dev alone. `said`: each fall back, for the log.
pub struct Edge<'a> {
    /// The network asked.
    pub inner: &'a dyn Net,
    /// Whether the edge is asked first.
    pub enabled: bool,
    /// Told each time the edge failed and the forge is asked instead.
    pub said: &'a dyn Fn(String),
}

impl Edge<'_> {
    /// Whether the edge is on (`get`: the environment).
    #[must_use]
    pub fn enabled_by(get: &dyn Fn(&str) -> Option<String>) -> bool {
        !NO_CDN_VARIABLES.iter().any(|v| get(v).is_some_and(|x| x.trim() == "1"))
    }
}

impl Net for Edge<'_> {
    fn text(&self, url: &str, max: u64) -> Result<String, NetError> {
        if self.enabled
            && let Some(cdn) = list_cdn_of(url)
        {
            match self.inner.text(&cdn, max) {
                Ok(t) => return Ok(t),
                Err(e) if e.cancelled => return Err(e),
                Err(e) => (self.said)(format!("git-cdn.magicbean.dev failed ({}): trying git.magicbean.dev", e.text)),
            }
        }
        self.inner.text(url, max)
    }

    fn download(&self, url: &str, to: &Path, max: u64, progress: &mut dyn FnMut(u64)) -> Result<(), NetError> {
        if self.enabled
            && let Some(cdn) = cdn_of(url)
        {
            match self.inner.download(&cdn, to, max, progress) {
                Ok(()) => return Ok(()),
                Err(e) if e.cancelled => return Err(e),
                Err(e) => (self.said)(format!("git-cdn.magicbean.dev failed ({}): trying git.magicbean.dev", e.text)),
            }
        }
        self.inner.download(&direct(url), to, max, progress)
    }
}

/// A file of a release, as the API lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// Its name.
    pub name: String,
    /// Its download link (https).
    pub url: String,
    /// Its size, as listed.
    pub size: u64,
}

/// A release as the API lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Its tag: the version.
    pub tag: String,
    /// A draft: CI still making it, or pulled back.
    pub draft: bool,
    /// A pre-release: on no stable channel.
    pub prerelease: bool,
    /// Its files.
    pub assets: Vec<Asset>,
}

impl Listed {
    /// A release of the API's JSON.
    fn of(v: &Value) -> Listed {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let assets = v
            .get("assets")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|a| {
                        let name = a.get("name")?.as_str()?.to_string();
                        let url = a.get("browser_download_url")?.as_str()?;
                        let size = a.get("size")?.as_u64()?;
                        url.starts_with("https://").then(|| Asset { name, url: url.to_string(), size })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Listed { tag: s("tag_name"), draft: flag("draft"), prerelease: flag("prerelease"), assets }
    }

    /// Its file of this name.
    #[must_use]
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The releases of a list's JSON (an array of the API's releases).
pub fn parse_list(json: &str) -> Result<Vec<Listed>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("the release list can't be read: {e}"))?;
    let a = v.as_array().ok_or("the release list is not a list")?;
    Ok(a.iter().map(Listed::of).collect())
}

/// The published releases a channel offers from `list`, newest first, each tag once.
#[must_use]
pub fn on_channel(list: &[Listed], channel: crate::Channel) -> Vec<&Listed> {
    let mut on: Vec<&Listed> = list
        .iter()
        .filter(|r| !r.draft && crate::Channel::of(&r.tag) == Some(channel))
        .filter(|r| channel == crate::Channel::Nightly || !r.prerelease)
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    on.retain(|r| seen.insert(r.tag.clone()));
    // (stable: ties keep the server's order)
    on.sort_by(|a, b| {
        if crate::versions::newer(&a.tag, &b.tag) {
            std::cmp::Ordering::Less
        } else if crate::versions::newer(&b.tag, &a.tag) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    on
}

/// A file of a release, with the SHA-256 its signed `SHA256SUMS` gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRef {
    /// Its name.
    pub name: String,
    /// Its link.
    pub url: String,
    /// Its SHA-256 (hex), SHA256SUMS's.
    pub sha256: String,
    /// Its size, as listed.
    pub size: u64,
}

/// A release whose `SHA256SUMS` a trusted key signed.
#[derive(Clone, Debug)]
pub struct Release {
    /// It as listed.
    pub listed: Listed,
    /// SHA256SUMS: each file's SHA-256.
    pub sums: BTreeMap<String, String>,
}

impl Release {
    /// Its file of this name, when it is both listed and in SHA256SUMS.
    #[must_use]
    pub fn file(&self, name: &str) -> Option<FileRef> {
        let a = self.listed.asset(name)?;
        let sha = self.sums.get(name)?;
        Some(FileRef { name: name.to_string(), url: a.url.clone(), sha256: sha.clone(), size: a.size })
    }

    /// A listed file's link and size (a delta: its SHA-256 the manifest's), when the release lists it.
    #[must_use]
    pub fn listed_file(&self, name: &str) -> Option<&Asset> {
        self.listed.asset(name)
    }
}

/// The forge, asked through a [`Net`], its releases' signatures checked with `keys`.
pub struct Forge<'a> {
    net: &'a dyn Net,
    api: String,
    keys: Vec<Key>,
    cancel: &'a Cancel,
    scratch: std::path::PathBuf,
}

impl<'a> Forge<'a> {
    /// The forge's releases of [`REPO`] through `net`; `scratch`: a folder for small downloads (SHA256SUMS, manifests).
    #[must_use]
    pub fn new(net: &'a dyn Net, keys: Vec<Key>, cancel: &'a Cancel, scratch: &Path) -> Forge<'a> {
        Forge { net, api: format!("{ORIGIN}/api/v1/repos/{REPO}"), keys, cancel, scratch: scratch.to_path_buf() }
    }

    fn checked(&self) -> Result<(), FetchError> {
        if self.cancel.is_set() { Err(FetchError::cancelled()) } else { Ok(()) }
    }

    fn unreachable(e: NetError) -> FetchError {
        if e.cancelled {
            FetchError::cancelled()
        } else {
            FetchError::new("release_unreachable", format!("git.magicbean.dev did not answer: {}", e.text))
        }
    }

    /// The published releases, every page (drafts left out).
    pub fn list(&self) -> Result<Vec<Listed>, FetchError> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            self.checked()?;
            let url = format!("{}/releases?limit=50&page={page}", self.api);
            let text = match self.net.text(&url, MAX_TEXT) {
                Ok(t) => t,
                Err(e) if page > 1 && !e.cancelled => break,
                Err(e) => return Err(Self::unreachable(e)),
            };
            let got = parse_list(&text).map_err(|e| FetchError::new("release_refused", e))?;
            let n = got.len();
            all.extend(got.into_iter().filter(|r| !r.draft));
            if n < 50 {
                break;
            }
        }
        Ok(all)
    }

    /// A release by its tag, as the API gives it (one the list may not show).
    pub fn by_tag(&self, tag: &str) -> Result<Listed, FetchError> {
        self.checked()?;
        if !crate::versions::is_tag_name(tag) {
            return Err(FetchError::new("no_such_release", format!("{tag:?} is not a version's name")));
        }
        let text = self.net.text(&format!("{}/releases/tags/{tag}", self.api), MAX_TEXT).map_err(|e| {
            if e.cancelled {
                FetchError::cancelled()
            } else {
                FetchError::new(
                    "no_such_release",
                    format!("git.magicbean.dev has no Longshot release {tag}, or did not answer ({})", e.text),
                )
            }
        })?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| FetchError::new("release_refused", format!("the release {tag} can't be read: {e}")))?;
        Ok(Listed::of(&v))
    }

    /// A small file into memory, at most `max` bytes.
    fn small(&self, url: &str, max: u64, name: &str) -> Result<Vec<u8>, FetchError> {
        self.checked()?;
        std::fs::create_dir_all(&self.scratch).map_err(|e| FetchError::io(&self.scratch, &e))?;
        let to = self.scratch.join(format!("{name}.part"));
        let got = self.net.download(url, &to, max, &mut |_| {}).map_err(Self::unreachable);
        let bytes = got.and_then(|()| std::fs::read(&to).map_err(|e| FetchError::io(&to, &e)));
        let _ = std::fs::remove_file(&to);
        let bytes = bytes?;
        if bytes.len() as u64 > max {
            return Err(FetchError::new("release_refused", format!("{name} is larger than expected")));
        }
        Ok(bytes)
    }

    /// A release's files checked: its `SHA256SUMS` signed by a trusted key.
    pub fn verified(&self, listed: &Listed) -> Result<Release, FetchError> {
        let tag = &listed.tag;
        let refused = |why: String| FetchError::new("release_refused", format!("The release {tag} is refused: {why}."));
        if listed.draft {
            return Err(refused("it is not published".into()));
        }
        if crate::Channel::of(tag).is_none() {
            return Err(refused("it is no version of Longshot".into()));
        }
        let (Some(sums), Some(sig)) = (listed.asset(SUMS), listed.asset(SIG)) else {
            return Err(refused("it has no signed SHA256SUMS".into()));
        };
        let sums_bytes = self.small(&sums.url, trust::MAX_SUMS, SUMS)?;
        let sig_bytes = self.small(&sig.url, trust::MAX_SIG, SIG)?;
        let sig = String::from_utf8_lossy(&sig_bytes);
        let sums = trust::open(&sums_bytes, &sig, &self.keys).map_err(refused)?;
        Ok(Release { listed: listed.clone(), sums })
    }

    /// A small file of a release (a manifest) into memory, checked for its size and SHA-256.
    pub fn fetch_small(&self, file: &FileRef, max: u64) -> Result<Vec<u8>, FetchError> {
        if file.size > max {
            return Err(FetchError::new("release_refused", format!("{} is larger than expected", file.name)));
        }
        let bytes = self.small(&file.url, file.size, &file.name)?;
        if bytes.len() as u64 != file.size || crate::sha256_hex(&bytes) != file.sha256 {
            return Err(FetchError::new(
                "release_refused",
                format!("{} is not the file its release names (size or SHA-256)", file.name),
            ));
        }
        Ok(bytes)
    }

    /// The network this forge asks.
    #[must_use]
    pub fn net(&self) -> &dyn Net {
        self.net
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edge_s_links_and_the_forge_s_own() {
        let file = "https://git.magicbean.dev/MagicBeanDev/Longshot/releases/download/v3.0.0/a.json";
        assert_eq!(
            cdn_of(file).as_deref(),
            Some("https://git-cdn.magicbean.dev/MagicBeanDev/Longshot/releases/download/v3.0.0/a.json")
        );
        assert_eq!(direct(file), format!("{file}?cdn=0"));
        assert_eq!(direct(&format!("{file}?x=1")), format!("{file}?x=1&cdn=0"));
        assert_eq!(direct(&format!("{file}?cdn=0")), format!("{file}?cdn=0"));
        assert_eq!(cdn_of("https://example.com/a/b/releases/download/x/y"), None);
        assert_eq!(direct("https://example.com/f"), "https://example.com/f");
        let list = "https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/releases?limit=50&page=2";
        assert_eq!(
            list_cdn_of(list).as_deref(),
            Some("https://git-cdn.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/releases?limit=50&page=2")
        );
        assert!(
            list_cdn_of("https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/releases/tags/v3.0.0").is_some()
        );
        assert_eq!(list_cdn_of("https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/pulls"), None);
        assert!(!Edge::enabled_by(&|k| (k == "LONGSHOT_LAUNCHER_NO_CDN").then(|| "1".into())));
        assert!(Edge::enabled_by(&|_| None));
    }

    #[test]
    fn a_channel_s_releases_newest_first() {
        let r = |tag: &str, draft, pre| Listed { tag: tag.into(), draft, prerelease: pre, assets: vec![] };
        let list = vec![
            r("v3.0.0", false, false),
            r("nightly-10-aaaaaaaa", false, false),
            r("v3.1.0", false, false),
            r("v3.2.0", true, false),
            r("v3.3.0", false, true),
            r("nightly-12-bbbbbbbb", false, false),
            r("v2.0.0", false, false),
        ];
        let tags = |c| on_channel(&list, c).iter().map(|r| r.tag.clone()).collect::<Vec<_>>();
        assert_eq!(tags(crate::Channel::Stable), ["v3.1.0", "v3.0.0"]);
        assert_eq!(tags(crate::Channel::Nightly), ["nightly-12-bbbbbbbb", "nightly-10-aaaaaaaa"]);
    }
}
