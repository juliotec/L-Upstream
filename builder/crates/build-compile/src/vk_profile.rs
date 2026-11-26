//! The renderer's committed play profile (`assets/profiles/longshot_vk.profdata.xz`, its note in that folder's
//! README): every build of the renderer uses it unless `RX_VK_PGO` says otherwise (`gen`, or a developer's own
//! profile), and only when this compiler reads it (it is recorded with one LLVM, and another may not read it: the
//! renderer is then built without it, a little slower in play, and the log says why).
//!
//! It is unpacked into the work folder under a name of its own (`longshot_vk.shipped.profdata`), written only when its
//! bytes change: the objects' keys hold its content (plan.rs, the unit's `profile`), so the same profile anywhere
//! keeps every object.

use crate::options::Pgo;
use build_core::{IoContext, Result};
use std::path::{Path, PathBuf};

/// The committed profile, from the tree's root.
pub const SHIPPED: &str = "assets/profiles/longshot_vk.profdata.xz";
/// Its unpacked copy in the work folder.
pub const UNPACKED: &str = "longshot_vk.shipped.profdata";

/// The profile the renderer is built with: `asked` (`RX_VK_PGO`) when it says anything, else the committed one when
/// the tree has it and `cc` reads it, else none. `log` gets a line saying which.
pub fn choose(asked: &Pgo, root: &Path, work: &Path, cc: &Path, target: &[String], log: &dyn Fn(&str)) -> Result<Pgo> {
    if *asked != Pgo::Off {
        return Ok(asked.clone());
    }
    let xz = root.join(SHIPPED);
    let Ok(packed) = std::fs::read(&xz) else { return Ok(Pgo::Off) };
    let mut data = Vec::new();
    if let Err(e) = lzma_rs::xz_decompress(&mut &packed[..], &mut data) {
        log(&format!("the Vulkan back end: {SHIPPED} does not unpack ({e}): built without a profile"));
        return Ok(Pgo::Off);
    }
    let prof = unpack(work, &data)?;
    match readable(cc, target, &prof, work) {
        Ok(()) => Ok(Pgo::Use(prof)),
        Err(why) => {
            log(&format!(
                "the Vulkan back end: this compiler ({}) does not read the committed play profile, so the renderer is \
                 built without it (a little slower in play): {why}",
                cc.display()
            ));
            Ok(Pgo::Off)
        }
    }
}

/// `data` as `<work>/longshot_vk.shipped.profdata`, written only when it differs (a compiler still ending from a
/// stopped build may hold the old one open on Windows).
pub fn unpack(work: &Path, data: &[u8]) -> Result<PathBuf> {
    let prof = work.join(UNPACKED);
    if std::fs::read(&prof).ok().as_deref() != Some(data) {
        std::fs::create_dir_all(work).at(work)?;
        let tmp = work.join(format!("{UNPACKED}.new"));
        std::fs::write(&tmp, data).at(&tmp)?;
        std::fs::rename(&tmp, &prof).at(&prof)?;
    }
    Ok(prof)
}

/// Whether `cc` compiles a one-line C file with the profile: `Err` with its first line of complaint when not.
fn readable(cc: &Path, target: &[String], prof: &Path, work: &Path) -> std::result::Result<(), String> {
    let dir = work.join("probe");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let src = dir.join("vk_profile_probe.c");
    std::fs::write(&src, "int f(int x) { return x + 1; }\n").map_err(|e| e.to_string())?;
    let mut cmd = crate::toolchain::clang(cc);
    cmd.args(target)
        .args(["-O2", "-Wno-everything", "-c"])
        .arg(format!("-fprofile-instr-use={}", prof.display()))
        .arg(&src)
        .arg("-o")
        .arg(dir.join("vk_profile_probe.o"));
    let out = build_core::child::output(&mut cmd).map_err(|e| format!("{} did not start: {e}", cc.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("it stopped").trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vk-profile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_developer_s_choice_stands() {
        let d = scratch("asked");
        let cc = Path::new("/nonexistent/cc");
        for asked in [Pgo::Generate, Pgo::Use("/p.profdata".into())] {
            assert_eq!(choose(&asked, &d, &d, cc, &[], &|_| {}).unwrap(), asked);
        }
    }

    #[test]
    fn no_committed_profile_is_none() {
        let d = scratch("none");
        assert_eq!(choose(&Pgo::Off, &d, &d, Path::new("cc"), &[], &|_| {}).unwrap(), Pgo::Off);
    }

    #[test]
    fn a_compiler_that_cannot_read_it_builds_without_it() {
        let d = scratch("unread");
        let xz = d.join(SHIPPED);
        std::fs::create_dir_all(xz.parent().unwrap()).unwrap();
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut &b"not a profile"[..], &mut packed).unwrap();
        std::fs::write(&xz, packed).unwrap();
        let lines = std::cell::RefCell::new(Vec::new());
        let got =
            choose(&Pgo::Off, &d, &d, Path::new("/nonexistent/cc"), &[], &|l| lines.borrow_mut().push(l.to_string()));
        assert_eq!(got.unwrap(), Pgo::Off);
        assert!(lines.borrow()[0].contains("does not read the committed play profile"), "{:?}", lines.borrow());
        assert_eq!(std::fs::read(d.join(UNPACKED)).unwrap(), b"not a profile");
    }

    #[test]
    fn the_copy_is_written_only_when_it_differs() {
        let d = scratch("unpack");
        let p = unpack(&d, b"one").unwrap();
        let t = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        unpack(&d, b"one").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), t);
        unpack(&d, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
    }
}
