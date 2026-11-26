//! Cargo, run as the release's workflow runs it (scripts/longshot_builder.py's `build`, scripts/renhdrum.py's): the
//! source's pinned Rust alone (`RUSTC`, its `bin/` first on the PATH), the crates its Cargo.lock pins (`cargo fetch
//! --locked`, then every build `--locked --offline`), into a cargo home of the cache's, linked by the source's own
//! tools (Linux: tools.lock's clang against its sysroot, glibc 2.35, with lld; Windows: its llvm-mingw), with the
//! release's flags (no path of this PC in the program: the tree and the crates by names of their own). Nothing of
//! the environment's Rust or Cargo settings is passed on (`RUSTFLAGS`, `CARGO_*`, `RUSTUP_TOOLCHAIN`, a wrapper).

use super::pins::triple;
use build_core::report::Reporter;
use build_core::{Error, Result};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The places and the toolchain a cargo run is given.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// The pinned Rust's folder (its `bin/rustc`, `bin/cargo`).
    pub rust: PathBuf,
    /// The cargo home: the crates' registry and its downloads, kept.
    pub home: PathBuf,
    /// The C compiler and linker: Linux, a small script running tools.lock's clang with its sysroot and lld;
    /// Windows, llvm-mingw's clang.
    pub linker: PathBuf,
    /// Folders put on the PATH after the Rust's (Windows: llvm-mingw's `bin/`).
    pub path: Vec<PathBuf>,
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

impl Toolchain {
    /// `bin/cargo` of the Rust.
    pub fn cargo(&self) -> PathBuf {
        self.rust.join("bin").join(exe("cargo"))
    }

    /// A cargo command in `dir` with this toolchain's environment and none of the PC's Rust settings; `flags` the
    /// RUSTFLAGS (the target's alone: build scripts and proc macros get none, as `--target` is always given).
    pub fn command(&self, dir: &Path, flags: &str) -> Command {
        let mut c = build_core::child::command(self.cargo());
        c.current_dir(dir);
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().into_owned();
            let rusty = k.starts_with("CARGO_") || k.starts_with("RUST") || k == "LONGSHOT_SOURCES";
            if rusty
                || ["CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "LDFLAGS", "CRATE_CC_NO_DEFAULTS"].contains(&k.as_str())
            {
                c.env_remove(&k);
            }
            // (the cc crate's per-target and host variables: CC_<target>, TARGET_CC, HOST_CC, ...)
            if ["CC_", "CXX_", "AR_", "CFLAGS_", "CXXFLAGS_", "TARGET_", "HOST_"].iter().any(|p| k.starts_with(p)) {
                c.env_remove(&k);
            }
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![self.rust.join("bin")];
        dirs.extend(self.path.iter().cloned());
        dirs.extend(std::env::split_paths(&path));
        let under = triple(cfg!(windows)).replace('-', "_");
        c.env("PATH", std::env::join_paths(dirs).unwrap_or(path))
            .env("RUSTC", self.rust.join("bin").join(exe("rustc")))
            .env("CARGO_HOME", &self.home)
            .env(format!("CARGO_TARGET_{}_LINKER", under.to_uppercase()), &self.linker)
            .env("RUSTFLAGS", flags)
            // (a wrapper in a config file of the PC's, ~/.cargo/config.toml, set to none)
            .env("RUSTC_WRAPPER", "")
            .env("CARGO_BUILD_RUSTC_WRAPPER", "")
            .env("CARGO_TERM_COLOR", "never");
        c
    }
}

/// The release's flags for a program built from the tree at `src` (scripts/longshot_builder.py's): the tree's and
/// the crates' paths by names of their own; Windows: the C runtime linked in (UCRT, llvm-mingw's unwinder), no time
/// stamp; Linux: no build id.
pub fn release_flags(src: &Path, home: &Path) -> String {
    let mut f = format!(
        "--remap-path-prefix={}=longshot --remap-path-prefix={}=rust --remap-path-prefix={}=crates",
        src.display(),
        src.join("builder").display(),
        home.join("registry").join("src").display()
    );
    f += if cfg!(windows) {
        " -C target-feature=+crt-static -C link-arg=-Wl,--no-insert-timestamp"
    } else {
        " -C link-arg=-Wl,--build-id=none"
    };
    f
}

/// Linux: the linker script at `at`, which runs `llvm`'s clang against `sysroot` with lld (the release's link of the
/// builder: scripts/longshot_builder.py's `--linker <clang> --sysroot`), written when it is not that already.
#[cfg(unix)]
pub fn linker_script(at: &Path, llvm: &Path, sysroot: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    let text = format!(
        "#!/bin/sh\n# from-source's C compiler and linker: tools.lock's clang, its sysroot (glibc 2.35), lld\nexec {} \
         --sysroot={} -fuse-ld=lld \"$@\"\n",
        q(&llvm.join("bin").join("clang")),
        q(sysroot)
    );
    build_core::fsx::write_if_changed(at, text.as_bytes())?;
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).map_err(|e| Error::io(at, e))
}

/// The tail of a failed run's output, for its error's text (at most `lines` lines).
fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// `cargo fetch --locked --target <this system's>` of the workspace at `dir`: the crates its Cargo.lock pins, each
/// checked by cargo against the lock's SHA-256, into the cargo home. The bytes downloaded (the home's `registry/cache`
/// grown).
pub fn fetch(t: &Toolchain, dir: &Path) -> Result<u64> {
    let cache = t.home.join("registry").join("cache");
    let before = build_core::fsx::files_under(&cache).map(|f| size_of(&cache, &f)).unwrap_or(0);
    let mut c = t.command(dir, "");
    c.args(["fetch", "--locked", "--target", triple(cfg!(windows))]);
    let out = build_core::child::output(&mut c).map_err(|e| Error::io(t.cargo(), e))?;
    if !out.status.success() {
        return Err(Error::player(
            "crates_download_failed",
            format!(
                "The crates the source's Cargo.lock pins could not be downloaded from crates.io ({}):\n{}",
                out.status,
                tail(&String::from_utf8_lossy(&out.stderr), 12)
            ),
        ));
    }
    let after = build_core::fsx::files_under(&cache).map(|f| size_of(&cache, &f)).unwrap_or(0);
    Ok(after.saturating_sub(before))
}

fn size_of(dir: &Path, files: &[PathBuf]) -> u64 {
    files.iter().filter_map(|f| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len()).sum()
}

/// The packages a build of `package` compiles (its normal and build dependencies for this system, each once): its
/// progress's whole. From `cargo tree`, offline; 0 when it can't tell.
pub fn units(t: &Toolchain, dir: &Path, package: &str) -> usize {
    let mut c = t.command(dir, "");
    c.args(["tree", "--locked", "--offline", "-p", package, "-e", "normal,build", "--prefix", "none"])
        .args(["--target", triple(cfg!(windows))]);
    let Ok(out) = build_core::child::output(&mut c) else { return 0 };
    let text = String::from_utf8_lossy(&out.stdout);
    let names: BTreeSet<String> =
        text.lines().map(|l| l.split_whitespace().take(2).collect::<Vec<_>>().join(" ")).collect();
    names.into_iter().filter(|n| !n.is_empty()).count()
}

/// What a cargo build gave: its program's path and how long it took.
pub struct Built {
    /// The file cargo made.
    pub file: PathBuf,
    /// Seconds.
    pub seconds: f64,
}

/// `cargo build <args>` (the package, the profile, `--bin <name>` or `--lib`) in `dir` with `flags`, `env` added, `jobs`
/// at once, into `target`: its file `artifact` (under [`out_dir`]). Its compiled units told to `progress` against
/// `whole` (0: none told), its diagnostics as log lines, its failure as `builder_compile_failed` with the tail of what
/// it said (`what`: what did not compile, for the player).
#[allow(clippy::too_many_arguments, reason = "one cargo run's settings, each its own")]
pub fn build(
    t: &Toolchain,
    dir: &Path,
    args: &[&str],
    flags: &str,
    env: &[(String, String)],
    jobs: usize,
    target: &Path,
    artifact: &Path,
    whole: usize,
    what: &str,
    r: &Reporter,
    progress: &dyn Fn(f64),
) -> Result<Built> {
    let started = std::time::Instant::now();
    let mut c = t.command(dir, flags);
    c.args(["build", "--locked", "--offline", "--target", triple(cfg!(windows)), "--jobs", &jobs.to_string()])
        .args(["--message-format", "json-render-diagnostics"])
        .args(args)
        .env("CARGO_TARGET_DIR", target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = build_core::child::spawn(&mut c).map_err(|e| Error::io(t.cargo(), e))?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let said = std::sync::Mutex::new(Vec::<String>::new());
    std::thread::scope(|s| {
        s.spawn(|| {
            // (cargo's own lines and the diagnostics: the log, but its "Compiling" lines, which progress stands for)
            for line in BufReader::new(stderr.expect("piped")).lines().map_while(std::result::Result::ok) {
                if !line.trim_start().starts_with("Compiling ") {
                    r.log(line.clone());
                }
                said.lock().unwrap_or_else(|p| p.into_inner()).push(line);
            }
        });
        let mut done = 0usize;
        for line in BufReader::new(stdout.expect("piped")).lines().map_while(std::result::Result::ok) {
            let Ok(m) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let compiled = m["reason"] == "compiler-artifact"
                && !m["target"]["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "custom-build"));
            if compiled && whole > 0 {
                done += 1;
                progress((done as f64 / whole as f64).min(0.97));
            }
        }
    });
    let status = child.wait().map_err(|e| Error::io(t.cargo(), e))?;
    let said = said.into_inner().unwrap_or_else(|p| p.into_inner()).join("\n");
    if !status.success() || !artifact.is_file() {
        return Err(Error::player(
            "builder_compile_failed",
            format!(
                "{what} did not compile from this source ({status}): a fault of the source, or of this PC's build. \
                 The end of what cargo said:\n{}",
                tail(&said, 20)
            ),
        ));
    }
    Ok(Built { file: artifact.to_path_buf(), seconds: started.elapsed().as_secs_f64() })
}

/// The folder cargo puts `--target`'s `profile` in, under `target`.
pub fn out_dir(target: &Path, profile: &str) -> PathBuf {
    target.join(triple(cfg!(windows))).join(if profile == "dev" { "debug" } else { profile })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_s_flags_name_no_path_of_this_pc() {
        let f = release_flags(Path::new("/c/fs/src"), Path::new("/c/cargo"));
        assert!(f.contains("--remap-path-prefix=/c/fs/src=longshot --remap-path-prefix=/c/fs/src/builder=rust"));
        assert!(f.contains("--remap-path-prefix=/c/cargo/registry/src=crates"));
        if cfg!(windows) {
            assert!(f.contains("+crt-static") && f.contains("--no-insert-timestamp"));
        } else {
            assert!(f.ends_with("-C link-arg=-Wl,--build-id=none"));
        }
        assert_eq!(tail("a\n\nb\nc\n", 2), "b\nc");
    }

    #[test]
    #[cfg(unix)]
    fn the_linker_script_runs_the_tools_clang_with_its_sysroot() {
        let d = crate::from_source::source::tests::scratch("linker");
        let at = d.join("cc");
        linker_script(&at, Path::new("/t/llvm"), Path::new("/t/it's root")).unwrap();
        let text = std::fs::read_to_string(&at).unwrap();
        assert!(text.contains("exec '/t/llvm/bin/clang' --sysroot='/t/it'\\''s root' -fuse-ld=lld \"$@\""), "{text}");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&at).unwrap().permissions().mode() & 0o777, 0o755);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
