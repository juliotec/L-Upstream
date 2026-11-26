//! The symbols the image binds to the host (image::build's `Options::hle`): the host's
//! HLE functions that some module has, every NVN function the sdk defines (all routed to host/nvn2.c, so the console's
//! driver never runs, whether the game reaches a function through nvnDeviceGetProcAddress or imports it), and the
//! OS-facing functions the game's own modules import and no module defines (each a stub that says its first call).

use game_files::nso::Nso;
use lift::whole::host::{Host, nvn_proc_name};
use std::collections::{BTreeSet, HashMap};

/// The OS services whose free functions are the host's (`nn::<namespace>::f`, or `nn::<namespace>::sdk::f`).
const OS_NAMESPACES: [&str; 41] = [
    "audio", "hid", "oe", "prepo", "nfp", "time", "account", "vi", "swkbd", "pl", "err", "codec", "diag", "detail",
    "settings", "ns", "applet", "album", "friends", "nifm", "ssl", "socket", "ec", "bcat", "web", "ldn", "irsensor",
    "aoc", "olsc", "pctl", "apm", "ro", "chat", "camera", "gs", "movie", "npns", "news", "fatal", "lbl", "capture",
];

/// The components of an Itanium nested name (`_ZN[K]<len><id>...E`), or `None` when it is not that simple shape.
fn nested_name(sym: &str) -> Option<Vec<&str>> {
    let b = sym.as_bytes();
    let mut i = if sym.starts_with("_ZNK") {
        4
    } else if sym.starts_with("_ZN") {
        3
    } else {
        return None;
    };
    let mut parts = Vec::new();
    while i < b.len() && b[i].is_ascii_digit() {
        let j = i + b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        let n: usize = sym[i..j].parse().ok()?;
        parts.push(sym.get(j..j + n)?);
        i = j + n;
    }
    (i < b.len() && b[i] == b'E').then_some(parts)
}

/// A free function of an OS-facing namespace (class members and data stay lifted). The Switch 2 SDK keeps many
/// service calls a level down: `nn::camera::sdk::CreateCameraContext`.
pub fn os_facing(sym: &str) -> bool {
    let Some(p) = nested_name(sym) else { return false };
    p.len() >= 2 && p[0] == "nn" && OS_NAMESPACES.contains(&p[1]) && (p.len() == 3 || (p.len() == 4 && p[2] == "sdk"))
}

/// What [`hle_set`] found.
#[derive(Clone, Debug, Default)]
pub struct HleSet {
    /// The symbols.
    pub symbols: BTreeSet<String>,
    /// The host's functions no module has (unused: an older SDK's, or a typo).
    pub unused: Vec<String>,
    /// Of the symbols: the host's, the sdk's NVN functions, the stubbed imports.
    pub counts: (usize, usize, usize),
}

/// The set for these modules and host.
pub fn hle_set(modules: &[Nso], host: &Host) -> HleSet {
    let known: BTreeSet<&str> = modules.iter().flat_map(|m| m.symbols.iter().map(|s| s.name.as_str())).collect();
    let mut hle: BTreeSet<String> =
        host.implemented.iter().filter_map(|n| n.strip_prefix("imp_")).map(String::from).collect();
    let unused: Vec<String> = hle.iter().filter(|s| !known.contains(s.as_str())).cloned().collect();
    for u in &unused {
        hle.remove(u);
    }
    let function: HashMap<&str, bool> = modules
        .iter()
        .flat_map(|m| {
            m.symbols.iter().filter(|s| s.defined && !s.name.is_empty()).map(|s| (s.name.as_str(), s.is_function()))
        })
        .collect();
    let nvn: BTreeSet<String> = modules
        .iter()
        .filter(|m| m.name == "sdk")
        .flat_map(|m| m.symbols.iter())
        .filter(|s| s.defined && s.is_function() && nvn_proc_name(&s.name).is_some() && !hle.contains(&s.name))
        .map(|s| s.name.clone())
        .collect();
    let stubbed: BTreeSet<String> = modules
        .iter()
        .filter(|m| m.name != "sdk" && m.name != "rtld") // (every game module: main and the subsdk* the exefs has)
        .flat_map(|m| m.symbols.iter())
        .filter(|s| !s.defined && !s.name.is_empty() && os_facing(&s.name))
        .filter(|s| function.get(s.name.as_str()).copied().unwrap_or(true) && !hle.contains(&s.name))
        .map(|s| s.name.clone())
        .collect();
    let counts = (hle.len(), nvn.len(), stubbed.len());
    hle.extend(nvn);
    hle.extend(stubbed);
    HleSet { symbols: hle, unused, counts }
}

/// The set for the exefs in `exefs` and the host in `root/host`, its counts said.
pub fn for_exefs(
    root: &std::path::Path,
    exefs: &std::path::Path,
    r: &build_core::report::Reporter,
) -> build_core::Result<BTreeSet<String>> {
    let modules = game_files::nso::load_exefs(exefs)?;
    let set = hle_set(&modules, &Host::read(&root.join("host"))?);
    if !set.unused.is_empty() {
        r.log(format!(
            "host/ defines {} HLE functions for symbols no module has (unused): {}",
            set.unused.len(),
            set.unused.join(" ")
        ));
    }
    r.log(format!(
        "the host's functions: {} HLE, {} NVN functions routed, {} OS-facing imports stubbed",
        set.counts.0, set.counts.1, set.counts.2
    ));
    Ok(set.symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_facing_free_functions_only() {
        assert!(os_facing("_ZN2nn3hid10InitializeEv"));
        assert!(os_facing("_ZN2nn6camera3sdk19CreateCameraContextEv"));
        assert!(!os_facing("_ZN2nn3hid6detail3Foo3BarEv"), "a member a level further down");
        assert!(!os_facing("_ZN2nn2gx10InitializeEv"), "not an OS namespace");
        assert!(!os_facing("nvnDeviceInitialize"));
        assert_eq!(nested_name("_ZNK2nn3hid1fEv"), Some(vec!["nn", "hid", "f"]));
        assert_eq!(nested_name("_ZN2nn99x"), None);
    }
}
