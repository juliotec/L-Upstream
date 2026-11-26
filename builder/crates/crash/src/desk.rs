//! The desktop: the clipboard and opening a link, a folder or a file.
//!
//! - Linux: the clipboard through a tool that keeps serving it once this program has ended (wl-copy on Wayland, xclip
//!   or xsel on X11); a link or a path opened with xdg-open.
//! - Windows: its own clipboard (the text stays after this program ends) and ShellExecuteW.

use crate::names::Copied;

/// `text` onto the clipboard.
pub fn clipboard(text: &str) -> Copied {
    imp::clipboard(text)
}

/// Opens a link, a folder or a file with what the system opens it with: whether it was handed over.
pub fn open(what: &str) -> bool {
    imp::open(what)
}

#[cfg(not(windows))]
mod imp {
    use crate::names::{self, Copied};
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn tool(argv: &[&str], text: &str) -> bool {
        let Ok(mut child) = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_PRELOAD")
            .spawn()
        else {
            return false;
        };
        let wrote = child.stdin.take().is_some_and(|mut i| i.write_all(text.as_bytes()).is_ok());
        // (wl-copy, xclip and xsel exit 0 once a background copy of theirs serves the selection; one still running
        // after 3 s with all of the text serves it itself)
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            match child.try_wait() {
                Ok(Some(s)) => return wrote && s.success(),
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => return false,
            }
        }
        wrote
    }

    pub(super) fn clipboard(text: &str) -> Copied {
        let set = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty());
        for argv in names::clip_tools(set("WAYLAND_DISPLAY"), set("DISPLAY")) {
            if tool(argv, text) {
                return Copied::Kept;
            }
        }
        Copied::No
    }

    pub(super) fn open(what: &str) -> bool {
        Command::new("xdg-open")
            .arg(what)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_PRELOAD")
            .status()
            .is_ok_and(|s| s.success())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use crate::names::Copied;
    use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    const CF_UNICODETEXT: u32 = 13;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub(super) fn clipboard(text: &str) -> Copied {
        // (Notepad and the like want \r\n)
        let crlf = text.replace("\r\n", "\n").replace('\n', "\r\n");
        let w = wide(&crlf);
        let bytes = w.len() * 2;
        for _ in 0..20 {
            // SAFETY: the clipboard opened with no owner, filled with a moveable block of the text's UTF-16 and its
            // NUL (copied in while locked), closed; the block is the system's once SetClipboardData took it.
            unsafe {
                if OpenClipboard(std::ptr::null_mut()) == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                EmptyClipboard();
                let g = GlobalAlloc(GMEM_MOVEABLE, bytes);
                let mut ok = false;
                if !g.is_null() {
                    let p = GlobalLock(g) as *mut u16;
                    if !p.is_null() {
                        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                        GlobalUnlock(g);
                        ok = !SetClipboardData(CF_UNICODETEXT, g as HANDLE).is_null();
                    }
                    if !ok {
                        GlobalFree(g);
                    }
                }
                CloseClipboard();
                return if ok { Copied::Kept } else { Copied::No };
            }
        }
        Copied::No
    }

    pub(super) fn open(what: &str) -> bool {
        let (verb, w) = (wide("open"), wide(what));
        // SAFETY: NUL-ended UTF-16 strings that outlive the call.
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        r as isize > 32
    }
}
