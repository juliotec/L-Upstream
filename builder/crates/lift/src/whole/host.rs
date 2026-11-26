//! What the lift reads from the host's sources (`host/*.c` and the game's `game/*.c` beside it, the files every game build compiles):
//! the HLE functions it defines (`void imp_<symbol>(Ctx` and `HLE(<symbol>)` at the start of a line) and the entry hooks
//! (`ENTRY_HOOK_SYM(<symbol>)`, `ENTRY_HOOK_SYM_FROM(<symbol>, <least size>)`).

use build_core::{IoContext, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The host's functions and hooks, as the lift needs them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    /// The C names of the HLE functions it defines (`imp_<symbol>`).
    pub implemented: BTreeSet<String>,
    /// The symbols it hooks at their entry, with the smallest size (X2) the hook is called for (0: any).
    pub entry_hooks: BTreeMap<String, u64>,
}

/// The C identifier at the start of `s`.
fn ident(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..end]
}

impl Host {
    /// The host's sources in `host` (and the game's, in `game` beside it), read.
    pub fn read(host: &Path) -> Result<Host> {
        let mut out = Host::default();
        let game = host.parent().map(|p| p.join("game"));
        for dir in std::iter::once(host.to_path_buf()).chain(game) {
            if !dir.is_dir() {
                continue;
            }
            let mut files: Vec<_> = std::fs::read_dir(&dir)
                .at(&dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "c"))
                .collect();
            files.sort();
            for path in files {
                let bytes = std::fs::read(&path).at(&path)?;
                out.scan(&String::from_utf8_lossy(&bytes));
            }
        }
        Ok(out)
    }

    /// One source's definitions added.
    pub fn scan(&mut self, text: &str) {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("void imp_") {
                let name = ident(rest);
                if !name.is_empty() && rest[name.len()..].starts_with("(Ctx") {
                    self.implemented.insert(format!("imp_{name}"));
                }
            } else if let Some(rest) = line.strip_prefix("HLE(") {
                let name = ident(rest);
                if !name.is_empty() && rest[name.len()..].starts_with(')') {
                    self.implemented.insert(format!("imp_{name}"));
                }
            } else if let Some(rest) = line.strip_prefix("ENTRY_HOOK_SYM_FROM(") {
                let name = ident(rest);
                let least = rest[name.len()..].strip_prefix(',').map(str::trim_start).and_then(|r| {
                    let digits = &r[..r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len())];
                    r[digits.len()..].starts_with(')').then(|| digits.parse::<u64>().ok()).flatten()
                });
                if let (false, Some(least)) = (name.is_empty(), least) {
                    self.entry_hooks.insert(name.to_string(), least);
                }
            } else if let Some(rest) = line.strip_prefix("ENTRY_HOOK_SYM(") {
                let name = ident(rest);
                if !name.is_empty() && rest[name.len()..].starts_with(')') {
                    self.entry_hooks.insert(name.to_string(), 0);
                }
            }
        }
    }

    /// The text the lift's key holds for it: what it found, not the files (a comment changed is no new lift).
    pub fn key_text(&self) -> String {
        let mut s = String::new();
        for n in &self.implemented {
            s.push_str(n);
            s.push('\n');
        }
        for (n, least) in &self.entry_hooks {
            s.push_str(&format!("hook {n} {least}\n"));
        }
        s
    }
}

/// The NVN proc a symbol stands for: a C name (`nvnFoo`), or a C++ one (`_Z<len>nvnFoo<args>`, as the driver exports a
/// few); `None` for any other symbol.
pub fn nvn_proc_name(sym: &str) -> Option<&str> {
    if sym.starts_with("nvn") {
        return Some(sym);
    }
    let rest = sym.strip_prefix("_Z")?;
    let digits = &rest[..rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len())];
    let len: usize = digits.parse().ok()?;
    let name = rest.get(digits.len()..digits.len() + len)?;
    name.starts_with("nvn").then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_definitions_and_hooks() {
        let mut h = Host::default();
        h.scan(
            "void imp_nnFoo(Ctx *c) {\nHLE(_ZN2nn3barEv) {\n  void imp_inner(Ctx *c);\nvoid imp_x(int)\n\
             ENTRY_HOOK_SYM(memcpy)\nENTRY_HOOK_SYM_FROM(memset, 8)\nENTRY_HOOK_SYM_FROM(bad, x)\n",
        );
        assert_eq!(h.implemented.iter().map(String::as_str).collect::<Vec<_>>(), ["imp__ZN2nn3barEv", "imp_nnFoo"]);
        assert_eq!(h.entry_hooks, BTreeMap::from([("memcpy".into(), 0), ("memset".into(), 8)]));
    }

    #[test]
    fn nvn_names() {
        assert_eq!(nvn_proc_name("nvnDeviceGetProcAddress"), Some("nvnDeviceGetProcAddress"));
        assert_eq!(nvn_proc_name("_Z24nvnNativeImageInitializeP1x"), Some("nvnNativeImageInitialize"));
        assert_eq!(nvn_proc_name("_ZN2nn3fooEv"), None);
        assert_eq!(nvn_proc_name("_Z99nvn"), None);
    }
}
