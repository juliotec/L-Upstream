//! The network by the system's curl, as the builder's other downloads (build-driver's runtimes.rs and tools/store.rs):
//! Windows' own `%SystemRoot%\System32\curl.exe` (every Windows 10 and 11 has it), else the system's on Linux. No
//! `.curlrc` read (`-q`), the URL as it is (`--globoff`), https alone and on every redirect, anonymous: no token or
//! cookie is ever sent. A download that stalls (under 1 KB/s for a minute) is given up; one larger than asked is cut.
//! Cancellable: the curl is ended at once, its partial file left for the caller to delete.

use crate::Cancel;
use crate::forge::{Net, NetError};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// The options every curl here starts with.
const SAFE: [&str; 15] = [
    "-q",
    "--globoff",
    "--proto",
    "=https",
    "--proto-redir",
    "=https",
    "-fsSL",
    "--connect-timeout",
    "30",
    "--speed-limit",
    "1024",
    "--speed-time",
    "60",
    "-A",
    "longshot-builder",
];

/// The system's curl, None without one.
#[must_use]
pub fn find() -> Option<PathBuf> {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        return Some(root.join("System32").join("curl.exe")).filter(|p| p.is_file());
    }
    let fixed = ["/usr/bin/curl", "/bin/curl", "/usr/local/bin/curl", "/run/host/usr/bin/curl"].map(PathBuf::from);
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    fixed.into_iter().chain(path.into_iter().filter(|d| d.is_absolute()).map(|d| d.join("curl"))).find(|p| p.is_file())
}

/// curl, asked through [`Net`].
pub struct Curl {
    program: PathBuf,
    cancel: Cancel,
}

impl Curl {
    /// The system's curl, cancelled by `cancel`; Err when there is none.
    pub fn new(cancel: Cancel) -> Result<Curl, String> {
        let program = find().ok_or("no curl on this PC (Windows has one in System32; on Linux install curl)")?;
        Ok(Curl { program, cancel })
    }

    /// curl with `args`, its answer into `to` (or returned, from its stdout, without one), watched: `progress` told
    /// the file's size as it grows; ended when cancelled, or past `max` bytes (curl's own --max-filesize goes by the
    /// Content-Length alone in older curls: an answer with none streams on).
    fn run(
        &self,
        args: &[&str],
        to: Option<&Path>,
        max: u64,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Vec<u8>, NetError> {
        let mut c = build_core::child::command(&self.program);
        c.args(SAFE).args(args).stdin(Stdio::null()).stderr(Stdio::piped());
        match to {
            Some(f) => c.arg("-o").arg(f).stdout(Stdio::null()),
            None => c.stdout(Stdio::piped()),
        };
        c.env_remove("LD_LIBRARY_PATH").env_remove("LD_PRELOAD");
        let mut child =
            build_core::child::spawn(&mut c).map_err(|e| NetError::new(format!("curl did not start: {e}")))?;
        let reader = |r: Option<Box<dyn std::io::Read + Send>>, cap: u64| {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                if let Some(r) = r {
                    let _ = std::io::Read::read_to_end(&mut std::io::Read::take(r, cap), &mut v);
                }
                v
            })
        };
        let err = reader(child.stderr.take().map(|e| Box::new(e) as Box<dyn std::io::Read + Send>), 1 << 20);
        let out =
            reader(child.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), max.saturating_add(1));
        let size = || to.and_then(|f| std::fs::metadata(f).ok()).map_or(0, |m| m.len());
        let status = loop {
            if self.cancel.is_set() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(NetError { text: "cancelled".into(), cancelled: true });
            }
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if size() > max {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(NetError::new(format!("the answer is over {max} bytes")));
                    }
                    progress(size());
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(NetError::new(format!("curl: {e}"))),
            }
        };
        let said = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
        let answer = out.join().unwrap_or_default();
        if answer.len() as u64 > max || size() > max {
            return Err(NetError::new(format!("the answer is over {max} bytes")));
        }
        if status.success() {
            progress(size());
            Ok(answer)
        } else {
            let why = said.trim().lines().last().unwrap_or("").trim_start_matches("curl: ").to_string();
            Err(NetError::new(if why.is_empty() { format!("curl failed ({status})") } else { why }))
        }
    }
}

impl Net for Curl {
    fn text(&self, url: &str, max: u64) -> Result<String, NetError> {
        let most = max.to_string();
        let args = ["--max-time", "120", "-H", "Accept: application/json", "--max-filesize", &most, url];
        let bytes = self.run(&args, None, max, &mut |_| {})?;
        String::from_utf8(bytes).map_err(|_| NetError::new(format!("{url}: not text")))
    }

    fn download(&self, url: &str, to: &Path, max: u64, progress: &mut dyn FnMut(u64)) -> Result<(), NetError> {
        let _ = std::fs::remove_file(to);
        self.run(&["--max-filesize", &max.to_string(), url], Some(to), max, progress).map(|_| ())
    }
}
