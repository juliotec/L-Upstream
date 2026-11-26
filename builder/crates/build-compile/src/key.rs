//! An object's key: exactly what changes the object, and nothing else.
//!
//! An object is made from its source, by one compiler, with its flags, reading the headers its source includes. Its key
//! is the hash of all of these, in two steps:
//!
//! - the **base**: the key scheme, the compiler's identity ([`compiler_identity`]: its version and the bytes of the
//!   programs that compile, not its path), the flags as they change the object ([`normal_flags`]: the folders that
//!   differ from PC to PC named by what they are, the depfile's own flags left out), the source's content, and what
//!   else the unit names (a profile's content);
//! - the **object key**: the base and every header the compile read, as its depfile names them, each by its place
//!   (named the same way) and its content.
//!
//! The headers are known only once the source has compiled, so the store keeps, by base, the headers each compile of
//! it read and the key they gave ([`crate::store`]'s manifests): a build looks the headers up by the base, hashes them
//! as they are now, and finds the object when they are as they were (ccache's "direct mode"). Every header counts, the
//! system's and the compiler's own too, so a new SDK or C library compiles what read it, and nothing else; no stamp of
//! the system headers is needed.
//!
//! What this does not see, as ccache does not: a header that would now be found before the one read (a new file of the
//! same name earlier in the include path). The include path itself is in the flags.

use build_core::Result;
use build_core::key::Key;
use std::path::{Path, PathBuf};

/// The key scheme: raised when what a key holds changes, so no object of another scheme is taken for this one's.
pub const SCHEME: &str = "build-compile 1";

/// The folders a path is named by in keys and manifests, so the same file elsewhere (another work folder, the
/// tree a release unpacks into another cache, the builder's tools unpacked in another folder) has the same name.
#[derive(Clone, Debug)]
pub struct Anchors {
    /// (name, folder as given, folder as compared), the longest folder first.
    list: Vec<(String, String, String)>,
    /// Whether folders are found without regard to case (Windows).
    fold: bool,
}

/// A path's text as keys compare it: `/` separated, and on Windows, whose file names are not case sensitive (a
/// depfile and a flag may spell one folder two ways), ASCII letters in lower case ([`fold`]).
pub fn path_text(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) { fold(&s) } else { s }
}

/// `s` with its ASCII letters in lower case, the rest as it is. Not `to_lowercase`: that changes the length of some
/// letters (`İ` becomes two characters, the Kelvin sign `K` one byte), so a place found in the folded text would not
/// be the same place in the text itself. Folders that differ only in the case of a letter past ASCII are not taken
/// for one: two spellings of them name the same file by two names, which costs a compile, never a wrong object.
pub fn fold(s: &str) -> String {
    s.to_ascii_lowercase()
}

impl Default for Anchors {
    fn default() -> Anchors {
        Anchors::new()
    }
}

impl Anchors {
    /// No anchors, folders compared as this system's file names are (without regard to case on Windows).
    pub fn new() -> Anchors {
        Anchors::folding(cfg!(windows))
    }

    /// No anchors, folders found without regard to (ASCII) case when `fold` (a test's other system).
    pub fn folding(fold: bool) -> Anchors {
        Anchors { list: Vec::new(), fold }
    }

    /// `folder` named `<name>` from now on.
    pub fn add(&mut self, name: &str, folder: &Path) {
        let mut f = folder.to_string_lossy().replace('\\', "/");
        while f.ends_with('/') && f.len() > 1 {
            f.pop();
        }
        if f.is_empty() || f == "/" {
            return;
        }
        let compared = if self.fold { fold(&f) } else { f.clone() };
        self.list.push((format!("<{name}>"), f, compared));
        self.list.sort_by_key(|a| std::cmp::Reverse(a.2.len()));
    }

    /// `text` with every anchored folder in it named.
    pub fn name(&self, text: &str) -> String {
        let mut t = text.replace('\\', "/");
        for (name, _, folder) in &self.list {
            if self.fold {
                // (found in the folded copy, replaced in the text: ASCII folding keeps every byte where it is)
                let mut out = String::new();
                let mut rest = t.as_str();
                while let Some(i) = fold(rest).find(folder.as_str()) {
                    out.push_str(&rest[..i]);
                    out.push_str(name);
                    rest = &rest[i + folder.len()..];
                }
                out.push_str(rest);
                t = out;
            } else {
                t = t.replace(folder.as_str(), name);
            }
        }
        t
    }

    /// The file a named path is (the inverse of [`Anchors::name`] for one path), its folder spelled as it was given.
    pub fn resolve(&self, named: &str) -> PathBuf {
        for (name, folder, _) in &self.list {
            if let Some(rest) = named.strip_prefix(name.as_str()) {
                return PathBuf::from(format!("{folder}{rest}"));
            }
        }
        PathBuf::from(named)
    }
}

/// The flags as they change the object: the depfile's (`-MD`, `-MF <file>`) left out, as they change only where the
/// list of headers goes; every anchored folder named. `-I` folders stay (their order decides which header is found).
pub fn normal_flags(flags: &[String], anchors: &Anchors) -> String {
    let mut out = Vec::with_capacity(flags.len());
    let mut skip = false;
    for f in flags {
        if std::mem::take(&mut skip) {
            continue;
        }
        match f.as_str() {
            "-MD" | "-MMD" => continue,
            "-MF" => {
                skip = true;
                continue;
            }
            _ => out.push(anchors.name(f)),
        }
    }
    out.join("\0")
}

/// A unit's base key ([`self`]'s notes).
pub fn base(compiler: &str, flags: &str, source_sha1: &str, extra: &[(String, Vec<u8>)]) -> String {
    let mut k = Key::new();
    k.text("scheme", SCHEME).text("compiler", compiler).text("flags", flags).text("source", source_sha1);
    for (name, bytes) in extra {
        k.part(name, bytes);
    }
    k.hex()
}

/// The object key of a base and the headers its compile read ((named path, content hash), in the depfile's order:
/// the same source and flags read them in the same order).
pub fn object(base: &str, headers: &[(String, String)]) -> String {
    let mut k = Key::new();
    k.text("base", base);
    for (path, sha1) in headers {
        k.text(path, sha1);
    }
    k.hex()
}

pub use build_core::key::file_sha1;

/// A compiler's identity: its version line and the bytes of its driver and of the program the driver runs to compile
/// (`-###`'s first command: llvm-mingw's `clang.exe` is a small driver that runs `clang-<n>.exe`), not where they
/// are. A rebuilt compiler of the same version is another compiler.
pub fn compiler_identity(cc: &Path) -> Result<String> {
    let version = build_core::child::output(crate::toolchain::clang(cc).arg("--version"))
        .map_err(|e| build_core::Error::io(cc, e))?;
    let line = String::from_utf8_lossy(&version.stdout).lines().next().unwrap_or("").trim().to_string();
    let real = std::fs::canonicalize(cc).unwrap_or_else(|_| cc.to_path_buf());
    let mut k = Key::new();
    k.text("version", &line).text("driver", &file_sha1(&real)?);
    let probe = build_core::child::output(crate::toolchain::clang(cc).args(["-###", "-x", "c", "-c", "-", "-o", "-"]));
    if let Ok(out) = probe {
        let err = String::from_utf8_lossy(&out.stderr);
        if let Some(first) = err.lines().find(|l| l.trim_start().starts_with('"')) {
            let prog = first.trim_start()[1..].split('"').next().unwrap_or("");
            let prog = Path::new(prog);
            let prog = std::fs::canonicalize(prog).unwrap_or_else(|_| prog.to_path_buf());
            if prog.is_file() && prog != real {
                k.text("compiler", &file_sha1(&prog)?);
            }
        }
    }
    Ok(k.hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn flags_name_their_folders_and_leave_the_depfile_out() {
        let mut a = Anchors::new();
        a.add("root", Path::new("/src/longshot"));
        a.add("work", Path::new("/src/longshot/work"));
        let f = s(&["-I/src/longshot/work/gen", "-MD", "-MF", "/x/a.d", "-I/src/longshot/host", "-O2"]);
        assert_eq!(normal_flags(&f, &a), "-I<work>/gen\0-I<root>/host\0-O2");
        assert_eq!(a.resolve("<work>/gen/a.h"), PathBuf::from("/src/longshot/work/gen/a.h"));
        assert_eq!(a.resolve("/usr/include/stdio.h"), PathBuf::from("/usr/include/stdio.h"));
    }

    #[test]
    fn a_windows_folder_with_letters_past_ascii_is_named_in_place_and_resolved_as_given() {
        // (İ lower-cased by Unicode's rules is two characters, the Kelvin sign K one byte: offsets of a
        // to_lowercase copy are not the text's)
        let mut a = Anchors::folding(true);
        a.add("work", Path::new("C:\\Users\\İsmail\\K\u{212A}\\work"));
        let text = "-IC:\\USERS\\İsmail\\K\u{212A}\\WORK\\gen -DX=\"c:/users/İsmail/k\u{212A}/work/a.h\"";
        assert_eq!(a.name(text), "-I<work>/gen -DX=\"<work>/a.h\"");
        assert_eq!(a.resolve("<work>/gen/a.h"), PathBuf::from("C:/Users/İsmail/K\u{212A}/work/gen/a.h"));
        // (not folded past ASCII: another case of İ is another folder, left as it is)
        assert_eq!(a.name("C:/Users/i̇smail/K\u{212A}/work/x"), "C:/Users/i̇smail/K\u{212A}/work/x");
        // (and on a system whose names are case sensitive, only the folder as given)
        let mut b = Anchors::folding(false);
        b.add("work", Path::new("/home/İsmail/work"));
        assert_eq!(b.name("/home/İsmail/work/a /HOME/İsmail/work/b"), "<work>/a /HOME/İsmail/work/b");
    }

    #[test]
    fn the_sysroot_s_place_is_not_part_of_a_key() {
        // (a sysroot mounted at another /tmp/.mount_* at each start, as an AppImage's is: a key holding its place would
        // compile everything again; the sysroot is named by what it is)
        let key = |mount: &str| {
            let sysroot = format!("/tmp/.mount_{mount}/usr/lib/longshot/sysroot");
            let mut a = Anchors::new();
            a.add("sysroot", Path::new(&sysroot));
            normal_flags(&s(&[&format!("--sysroot={sysroot}"), "-O2"]), &a)
        };
        assert_eq!(key("aaa"), key("bbb"));
        assert_eq!(key("aaa"), "--sysroot=<sysroot>\0-O2");
    }

    #[test]
    fn every_part_moves_the_key() {
        let b = base("cc1", "-O2", "abc", &[]);
        assert_ne!(b, base("cc2", "-O2", "abc", &[]));
        assert_ne!(b, base("cc1", "-O1", "abc", &[]));
        assert_ne!(b, base("cc1", "-O2", "abd", &[]));
        assert_ne!(b, base("cc1", "-O2", "abc", &[("profile".into(), b"p".to_vec())]));
        let h = vec![("<root>/a.h".to_string(), "1".to_string())];
        assert_ne!(object(&b, &h), object(&b, &[]));
        assert_ne!(object(&b, &h), object(&b, &[("<root>/a.h".to_string(), "2".to_string())]));
    }
}
