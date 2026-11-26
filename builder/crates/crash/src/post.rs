//! Posting a report with the system's curl, as the game's crash helper did: `--disable` (no `.curlrc` adds a redirect,
//! a proxy or `--insecure`), `--proto =https`, no redirect followed, 10 s to connect and 25 s in all. On Windows it is
//! `%SystemRoot%\System32\curl.exe` (Windows 10 and 11 have one); on Linux the system's, without the game's
//! `LD_LIBRARY_PATH` and `LD_PRELOAD`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The system's curl: its path, None without one. `CRASH_TEST_CURL` stands in for it in tests (cfg(test) only).
pub fn find_curl() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(p) = std::env::var_os("CRASH_TEST_CURL") {
        return Some(PathBuf::from(p));
    }
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let curl = root.join("System32").join("curl.exe");
        return curl.is_file().then_some(curl);
    }
    ["/usr/bin/curl", "/bin/curl", "/usr/local/bin/curl", "/run/host/usr/bin/curl"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// curl's arguments for posting the file `payload` to `address`, its headers to `headers` and its answer to `answer`.
pub fn curl_args(payload: &Path, headers: &Path, answer: &Path, address: &str, agent: &str) -> Vec<String> {
    [
        "--disable",
        "--silent",
        "--show-error",
        "--max-time",
        "25",
        "--connect-timeout",
        "10",
        "--proto",
        "=https",
        "--header",
        "Content-Type: application/json",
        "--user-agent",
        agent,
        "--data-binary",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        format!("@{}", payload.display()),
        "--dump-header".into(),
        headers.display().to_string(),
        "--output".into(),
        answer.display().to_string(),
        address.to_string(),
    ])
    .collect()
}

/// Posts the JSON file `payload` to `address` (https://), its scratch files beside it: the HTTP status (0 when there
/// was no answer) and the answer's body.
pub fn post(payload: &Path, address: &str, agent: &str) -> (u16, String) {
    let Some(curl) = find_curl() else { return (0, String::new()) };
    let headers = payload.with_extension("headers.txt");
    let answer = payload.with_extension("answer.txt");
    let _ = std::fs::remove_file(&headers);
    let _ = std::fs::remove_file(&answer);
    let mut c = Command::new(curl);
    c.args(curl_args(payload, &headers, &answer, address, agent))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c.env_remove("LD_LIBRARY_PATH").env_remove("LD_PRELOAD");
    let ran = match c.spawn() {
        Ok(mut child) => {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) if start.elapsed() < Duration::from_secs(30) => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break false;
                    }
                }
            }
        }
        Err(_) => false,
    };
    let read = |p: &Path| crate::store::read_text(p, 16 << 10).unwrap_or_default();
    let (status, body) =
        if ran { (crate::names::http_status(&read(&headers)), read(&answer)) } else { (0, String::new()) };
    let _ = std::fs::remove_file(&headers);
    let _ = std::fs::remove_file(&answer);
    (status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_is_told_the_safe_rules() {
        let a = curl_args(
            Path::new("p.json"),
            Path::new("h"),
            Path::new("a"),
            "https://x.example/t",
            "longshot-crash-report",
        );
        assert_eq!(a[0], "--disable", "first: no .curlrc of the player's");
        for pair in [["--proto", "=https"], ["--max-time", "25"], ["--connect-timeout", "10"]] {
            assert!(a.windows(2).any(|w| w == pair), "{pair:?}");
        }
        assert!(!a.iter().any(|s| s == "-L" || s == "--location"), "no redirect followed");
        assert_eq!(a.last().map(String::as_str), Some("https://x.example/t"));
    }
}
