//! Longshot's store, the server's generic package registry (docs/dev/reference/releases.md, "tools.lock"): a tool's
//! archive downloaded, asked after and uploaded, with curl (as the builder's other downloads: crate::runtimes). Also the
//! server's release API, read the same way ([`json`], [`download_checked`]: `longshot-builder dev release fetch-bases`).
//! Anyone downloads with no account; an upload takes a token with write access to the organization's packages
//! (`RELEASE_TOKEN`), and goes to `FORGEJO_UPLOAD_API` (`<server>/api/v1`, past Cloudflare's 100 MB limit) when set.

use super::lock::{OWNER, SERVER, Tool};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn curl() -> Result<PathBuf, String> {
    crate::runtimes::downloader("curl", std::env::var_os("PATH"), &[]).ok_or_else(|| "no curl on this PC".into())
}

/// `url` into `to`, refused past `most` bytes (the network's [`super::archive::Getter`]).
pub fn download(url: &str, to: &Path, most: u64) -> Result<(), String> {
    let s = build_core::child::status(
        build_core::child::command(curl()?)
            .args(crate::runtimes::CURL)
            .args(["-A", "longshot-tools", "--max-filesize", &most.to_string(), "-o"])
            .arg(to)
            .arg(url),
    )
    .map_err(|e| format!("curl did not start: {e}"))?;
    if !s.success() {
        return Err(format!("{url}: curl failed ({s})"));
    }
    // (older curls hold --max-filesize to the Content-Length alone: an answer with none is measured here)
    if std::fs::metadata(to).map_or(0, |m| m.len()) > most {
        let _ = std::fs::remove_file(to);
        return Err(format!("{url}: the answer is over {most} bytes"));
    }
    Ok(())
}

/// How many times an ask of the server is made in all, and how long before each try again (10 s, then 20 s).
const TRIES: u64 = 3;

fn wait_before(n: u64) {
    if n > 0 {
        std::thread::sleep(std::time::Duration::from_secs(10 * n));
    }
}

/// `url`'s JSON (anonymous, https only), asked again up to three times in all, 10 s then 20 s apart, when the answer
/// fails or is not JSON; the last try's error.
pub fn json(url: &str) -> Result<serde_json::Value, String> {
    let mut last = String::new();
    for n in 0..TRIES {
        wait_before(n);
        let out = build_core::child::output(
            build_core::child::command(curl()?)
                .args(crate::runtimes::CURL_SAFE)
                .args(["-fsSL", "--max-time", "120", "-A", "longshot-tools", "-H", "Accept: application/json"])
                .arg(url),
        );
        last = match out {
            Ok(o) if o.status.success() => match serde_json::from_slice(&o.stdout) {
                Ok(v) => return Ok(v),
                Err(e) => format!("{url}: not JSON ({e})"),
            },
            Ok(o) => format!("{url}: curl failed ({})", o.status),
            Err(e) => format!("curl did not start: {e}"),
        };
    }
    Err(last)
}

/// `url` into `to` (through `to.part`, removed whatever happens), checked to be `size` bytes with this SHA-256; nothing
/// downloaded when `to` has those bytes already.
pub fn download_checked(url: &str, to: &Path, sha256: &str, size: u64) -> Result<(), String> {
    if super::archive::sha256_file(to).is_ok_and(|(sha, n)| sha == sha256 && n == size) {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let got = download(url, &part, size).and_then(|()| super::archive::sha256_file(&part)).and_then(|(sha, n)| {
        if sha == sha256 && n == size {
            std::fs::rename(&part, to).map_err(|e| format!("{}: {e}", to.display()))
        } else {
            Err(format!("{url}: {n} bytes, SHA-256 {sha}; {size} bytes, {sha256} wanted"))
        }
    });
    let _ = std::fs::remove_file(&part);
    got
}

/// Whether a tool's copy is in the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InStore {
    /// There, with the size and SHA-256 the package API lists for it.
    There {
        /// Its size, if listed.
        size: Option<u64>,
        /// Its SHA-256, if listed.
        sha256: Option<String>,
    },
    /// Not there: the server answered, and its version's list has no such file (or there is no such version).
    Missing,
    /// The server could not be asked (no network, an error of its own, an answer that is not the list), every try:
    /// why, the last time. Not the same as missing: nothing is known.
    Unreached(String),
}

/// Whether a tool's copy is in the store, from the package API's list of its version's files (anonymous; a HEAD
/// through Cloudflare gives no size), asked up to three times in all as [`json`] is when it can't be reached.
pub fn exists(t: &Tool) -> InStore {
    let api = format!("{SERVER}/api/v1/packages/{OWNER}/generic/{}-{}/{}/files", t.name, t.system, t.version);
    let file = format!("{}-{}{}", t.sha256, t.name, super::lock::EXT);
    let Ok(c) = curl() else { return InStore::Unreached("no curl on this PC".into()) };
    let mut last = String::new();
    for n in 0..TRIES {
        wait_before(n);
        // (no -f: a 404, no such version, is an answer, not a failure)
        let out = build_core::child::output(
            build_core::child::command(&c)
                .args(crate::runtimes::CURL_SAFE)
                .args(["-sSL", "--max-time", "60", "-A", "longshot-tools", "-H", "Accept: application/json"])
                .args(["-w", "\n%{http_code}"])
                .arg(&api),
        );
        let o = match out {
            Ok(o) => o,
            Err(e) => return InStore::Unreached(format!("curl did not start: {e}")),
        };
        match answer(&o.stdout, &file) {
            Ok(found) => return found,
            Err(why) => last = format!("{api}: {why}"),
        }
    }
    InStore::Unreached(last)
}

/// What the package API's answer (`<body>\n<HTTP status>`) says of `file`; `Err` when it says nothing (no answer, the
/// server's own error, a body that is not the list).
fn answer(out: &[u8], file: &str) -> Result<InStore, String> {
    let text = String::from_utf8_lossy(out);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    match code.trim() {
        "404" => Ok(InStore::Missing),
        "200" => {
            let Ok(serde_json::Value::Array(files)) = serde_json::from_str(body) else {
                return Err("an answer that is not the list of files".into());
            };
            Ok(files.iter().find(|f| f["name"].as_str() == Some(file)).map_or(InStore::Missing, |f| InStore::There {
                size: f.get("Size").or_else(|| f.get("size")).and_then(serde_json::Value::as_u64),
                sha256: f["sha256"].as_str().map(str::to_string),
            }))
        }
        "000" | "" => Err("not reached".into()),
        c => Err(format!("HTTP {c}")),
    }
}

/// Where an upload goes: `FORGEJO_UPLOAD_API`'s server when it names one (`<server>/api/v1`), else the store's.
pub fn upload_server(upload_api: Option<&str>) -> String {
    let base = upload_api.unwrap_or("").trim_end_matches('/');
    base.strip_suffix("/api/v1").unwrap_or(SERVER).to_string()
}

/// The curl options an upload to `to` starts with: [`CURL_SAFE`](crate::runtimes::CURL_SAFE), HTTPS alone, for the
/// store; plain HTTP too for an `http://` `FORGEJO_UPLOAD_API` (the server's own address, inside its network), its
/// redirects still HTTPS alone, as `dev release`'s own asks of it.
fn put_protocols(to: &str) -> &'static [&'static str] {
    if to.starts_with("http://") {
        &["-q", "--globoff", "--proto", "=http,https", "--proto-redir", "=https"]
    } else {
        crate::runtimes::CURL_SAFE
    }
}

/// The archive at `path` put in the store as `t`'s copy (PUT). A copy there already (409) is kept when it is the same:
/// its bytes cannot change. Returns the copy's URL.
pub fn put(path: &Path, t: &Tool) -> Result<String, String> {
    let url = t.store_url(SERVER);
    let to = t.store_url(&upload_server(crate::env("FORGEJO_UPLOAD_API").as_deref()));
    let token = crate::env("RELEASE_TOKEN")
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("RELEASE_TOKEN is needed (a token with write access to {OWNER}'s packages)"))?;
    // (the token by stdin, never on the command line: `-H @-`)
    let mut put = build_core::child::command(curl()?);
    put.args(put_protocols(&to))
        .args(["-sS", "-X", "PUT", "--max-time", "1800", "-A", "longshot-tools"])
        .args(["-H", "@-", "-H", "Content-Type: application/zstd", "--upload-file"])
        .arg(path)
        .args(["-w", "\n%{http_code}"])
        .arg(&to)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = build_core::child::spawn(&mut put).map_err(|e| format!("curl did not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(format!("Authorization: token {token}\n").as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    match code.trim() {
        "200" | "201" => println!("uploaded {url}"),
        "409" => {
            let (size, sha) = match exists(t) {
                InStore::There { size, sha256 } => (size, sha256),
                _ => (None, None),
            };
            if size != Some(t.size) || sha.as_deref() != Some(t.sha256.as_str()) {
                return Err(format!(
                    "{url} is in the package registry already with other bytes ({size:?} bytes, SHA-256 {sha:?}): a new \
                     version, then"
                ));
            }
            println!("{url} is in the package registry already");
        }
        c => return Err(format!("PUT {url}: HTTP {c} {}", body.chars().take(300).collect::<String>())),
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_unreached_are_told_apart() {
        let f = "abc-llvm.tar.zst";
        let list = format!(r#"[{{"name":"{f}","Size":12,"sha256":"abc"}}]"#);
        assert_eq!(
            answer(format!("{list}\n200").as_bytes(), f),
            Ok(InStore::There { size: Some(12), sha256: Some("abc".into()) })
        );
        assert_eq!(answer(b"[]\n200", f), Ok(InStore::Missing));
        assert_eq!(answer(b"{\"message\":\"package does not exist\"}\n404", f), Ok(InStore::Missing));
        assert!(answer(b"\n000", f).is_err(), "not reached");
        assert!(answer(b"<html>bad gateway</html>\n502", f).is_err());
        assert!(answer(b"<html>a login page</html>\n200", f).is_err(), "not the list");
    }

    #[test]
    fn an_upload_goes_to_the_upload_api_s_server() {
        assert_eq!(upload_server(Some("http://forgejo:3000/api/v1")), "http://forgejo:3000");
        assert_eq!(upload_server(Some("http://forgejo:3000/api/v1/")), "http://forgejo:3000");
        assert_eq!(upload_server(None), SERVER);
        assert_eq!(upload_server(Some("elsewhere")), SERVER, "not an API address: the store's own");
    }

    #[test]
    fn an_upload_to_an_http_api_lets_curl_use_http() {
        // (curl's --proto =https refused the http:// FORGEJO_UPLOAD_API before it asked: "HTTP 000")
        let http = put_protocols("http://forgejo:3000/api/packages/x");
        assert_eq!(http[..2], crate::runtimes::CURL_SAFE[..2]);
        assert!(http.windows(2).any(|w| w == ["--proto", "=http,https"]));
        assert!(http.windows(2).any(|w| w == ["--proto-redir", "=https"]), "a redirect is still HTTPS alone");
        let https = put_protocols(&format!("{SERVER}/api/packages/x"));
        assert_eq!(https, crate::runtimes::CURL_SAFE);
        assert!(https.windows(2).any(|w| w == ["--proto", "=https"]));
    }
}
