//! What only the system can tell or do: the memory free, what a process tree holds, keeping the PC awake, a file
//! removed when the PC restarts (Windows, for an administrator), the registry, a hidden console, a message box, a
//! game's window brought to the front, and the children tied to the builder ([`crate::child`]).
//!
//! Linux reads `/proc` and asks `prctl` for a child's end; Windows calls the Win32 functions through `windows-sys`.
//! This is the builder's one place with `unsafe` (each call is a plain query on memory this module owns).

/// The memory available now, in bytes.
pub(crate) fn available_memory() -> Option<u64> {
    imp::available_memory()
}

/// The resident bytes of process `pid` and of the processes it started.
pub(crate) fn tree_resident(pid: u32) -> Option<u64> {
    imp::tree_resident(pid)
}

/// Keeps the PC from sleeping and this process from Windows' power throttling (EcoQoS: issue 87, a background
/// window's build at 0 % CPU) while the build runs. Nothing to do elsewhere.
pub fn stay_awake() {
    imp::stay_awake();
}

/// Windows: `path` (a file a running program holds) marked to be removed when the PC restarts: whether Windows took
/// it. Windows keeps that list in the machine's registry, so it takes it only from an administrator; a player's build
/// gets false, and the next build removes the file instead. Nothing to do elsewhere (a held file can be removed
/// there): false.
pub(crate) fn delete_at_restart(path: &std::path::Path) -> bool {
    imp::delete_at_restart(path)
}

/// This process, and every process it starts from now on, at a low priority, so the PC stays usable while it compiles
/// (`from-source`'s compile of a builder): Linux, a nice of 10 (the calling thread's, which the threads and
/// processes it starts inherit: call it first); Windows, the below-normal priority class, which the processes it
/// starts inherit. Whether it was lowered.
pub fn lower_priority() -> bool {
    imp::lower_priority()
}

/// A child's command tied to the builder before it starts ([`crate::child`]): Linux, killed when the thread that
/// starts it ends; Windows, no console window of its own.
pub(crate) fn tie(cmd: &mut std::process::Command) {
    imp::tie(cmd);
}

/// A child just started put in the builder's job (Windows: ended with the builder), when the builder itself could not
/// be put in it at its start ([`join_job`]); nothing to do otherwise, or elsewhere.
pub(crate) fn adopt(child: &std::process::Child) {
    imp::adopt(child);
}

/// Windows: the builder put in its own job at its start (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), so every process it
/// starts is in the job from its creation, with every process that one starts: whatever ends the builder ends them.
/// The job lets a child break away when it asks ([`detached`]: the game the player keeps). Whether the builder is in
/// it (false: Windows refused, a builder in a job that takes none inside it, before Windows 8; each child is then put
/// in once started, [`adopt`]). Nothing to do elsewhere (false).
pub fn join_job() -> bool {
    imp::join_job()
}

/// A command for a program the player keeps (the game), started outside the builder's job on Windows
/// (`CREATE_BREAKAWAY_FROM_JOB`), so it outlives the builder: its status once ended. Where Windows refuses the break
/// away (a job around the builder's that allows none: a launcher's, Steam's), it is started in the job as it is.
/// Elsewhere, [`std::process::Command::status`].
pub fn detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    imp::detached(cmd)
}

/// A key of the registry's HKEY_LOCAL_MACHINE (`path`, its parts split by `\\`): its values, each a name and its text
/// (a string's, a list's lines joined by `\n`, a number's digits, "" for another kind), and its subkeys' names.
/// Nothing where there is no registry or the key can't be read.
pub fn machine_registry(path: &str) -> (Vec<(String, String)>, Vec<String>) {
    imp::registry(false, path)
}

/// The same of HKEY_CURRENT_USER: this user's settings (Steam's folder).
pub fn user_registry(path: &str) -> (Vec<(String, String)>, Vec<String>) {
    imp::registry(true, path)
}

/// The file names of the programs running now (`steam.exe`; Linux: each process's first argument's last part,
/// `steam`), the processes this user may see. Empty where the system can't say.
pub fn running_programs() -> Vec<String> {
    imp::running_programs()
}

/// Windows: for a process that has no console (the version folder's `Longshot.exe`, a windowed program): its parent's,
/// when it was started from one (a developer's terminal: what it says shows there, the handles it was given kept);
/// else a hidden one (started by a launcher, a shortcut or Steam), so the console programs it starts (the compilers,
/// the shader preparation) share it instead of each opening a window of its own. Its standard handles stay the ones it was given, so what it starts
/// inherits those (the game, given none, writes its log file). Nothing to do with a console already, or elsewhere.
pub fn hidden_console() {
    imp::hidden_console();
}

/// The game a player plays, started with `cmd` and waited for: its status. Windows: out of the builder's job
/// ([`detached`]). Linux: SIGTERM and SIGINT sent to the builder by another program (a launcher stopping it, `kill`)
/// are passed on to the game while it runs (a terminal's Ctrl+C reaches the game itself, in the same process group,
/// and is not sent twice); the builder lives on until the game ends. Elsewhere, [`std::process::Command::status`].
pub fn run_game(cmd: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    imp::run_game(cmd, &mut |_| {})
}

/// [`run_game`], `started` told the game's process id once it is started (play's crash watch: its records are found
/// by it, and on Linux its cgroup's out-of-memory count is read then).
pub fn run_game_with(
    cmd: &mut std::process::Command,
    started: &mut dyn FnMut(u32),
) -> std::io::Result<std::process::ExitStatus> {
    imp::run_game(cmd, started)
}

/// Windows: `text` in a message box titled `title` (an error's icon when `error`), until the player closes it: true
/// when it was shown. Nothing elsewhere (false).
pub fn message_box(title: &str, text: &str, error: bool) -> bool {
    imp::message_box(title, text, error)
}

/// Windows: the window of a program process `parent` started (the game a builder plays) brought to the front, restored
/// when it was minimised: whether there was one. Nothing elsewhere (false: Linux's desktops each do it their own way,
/// and gamescope shows one program anyway).
pub fn raise_window_of_child(parent: u32) -> bool {
    imp::raise_window_of_child(parent)
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn hidden_console() {}

    #[cfg(target_os = "linux")]
    #[allow(unsafe_code)]
    pub(super) fn run_game(
        cmd: &mut std::process::Command,
        started: &mut dyn FnMut(u32),
    ) -> std::io::Result<std::process::ExitStatus> {
        use std::sync::atomic::{AtomicI32, Ordering};
        /// The game's pid while it runs, 0 otherwise.
        static GAME: AtomicI32 = AtomicI32::new(0);
        extern "C" fn pass_on(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
            // SAFETY: the kernel's siginfo for this signal (SA_SIGINFO), read only. kill is async-signal-safe.
            unsafe {
                // (si_code 0 or less: sent by a program (SI_USER, SI_QUEUE); a terminal's is the kernel's)
                let game = GAME.load(Ordering::SeqCst);
                if game > 0 && !info.is_null() && (*info).si_code <= 0 {
                    libc::kill(game, sig);
                }
            }
        }
        let mut child = cmd.spawn()?;
        GAME.store(child.id() as i32, Ordering::SeqCst);
        started(child.id());
        let sigs = [libc::SIGTERM, libc::SIGINT];
        // SAFETY: zeroed sigaction structures are valid empty ones; each is filled before use and outlives its call.
        let mut old: [libc::sigaction; 2] = unsafe { std::mem::zeroed() };
        for (k, sig) in sigs.into_iter().enumerate() {
            // SAFETY: as above; the handler is an extern "C" function of the right signature for SA_SIGINFO.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = pass_on as *const () as usize;
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(sig, &sa, &mut old[k]);
            }
        }
        let status = child.wait();
        for (k, sig) in sigs.into_iter().enumerate() {
            // SAFETY: the handlers there before, put back.
            unsafe { libc::sigaction(sig, &old[k], std::ptr::null_mut()) };
        }
        GAME.store(0, Ordering::SeqCst);
        status
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn run_game(
        cmd: &mut std::process::Command,
        started: &mut dyn FnMut(u32),
    ) -> std::io::Result<std::process::ExitStatus> {
        let mut child = cmd.spawn()?;
        started(child.id());
        child.wait()
    }

    pub(super) fn message_box(_title: &str, _text: &str, _error: bool) -> bool {
        false
    }

    pub(super) fn raise_window_of_child(_parent: u32) -> bool {
        false
    }

    pub(super) fn registry(_user: bool, _path: &str) -> (Vec<(String, String)>, Vec<String>) {
        (Vec::new(), Vec::new())
    }

    pub(super) fn running_programs() -> Vec<String> {
        let Ok(rd) = fs::read_dir("/proc") else { return Vec::new() };
        rd.flatten()
            .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|e| {
                // (a process that ended while it was read, or another user's that can't be: left out)
                let cmdline = fs::read(e.path().join("cmdline")).ok()?;
                let first = cmdline.split(|&b| b == 0).next()?;
                let name = String::from_utf8_lossy(first);
                Some(name.rsplit('/').next().unwrap_or("").to_string()).filter(|n| !n.is_empty())
            })
            .collect()
    }

    use std::fs;

    pub(super) fn available_memory() -> Option<u64> {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }

    pub(super) fn tree_resident(pid: u32) -> Option<u64> {
        // (statm's second field: resident pages; the page size is 4 KB on every PC the builder runs on)
        const PAGE: u64 = 4096;
        let statm = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let mut bytes = statm.split_whitespace().nth(1)?.parse::<u64>().ok()? * PAGE;
        if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
            for task in tasks.flatten() {
                let Ok(children) = fs::read_to_string(task.path().join("children")) else { continue };
                for child in children.split_whitespace().filter_map(|c| c.parse::<u32>().ok()) {
                    bytes += tree_resident(child).unwrap_or(0);
                }
            }
        }
        Some(bytes)
    }

    pub(super) fn stay_awake() {}

    #[cfg(target_os = "linux")]
    #[allow(unsafe_code)]
    pub(super) fn lower_priority() -> bool {
        // SAFETY: a plain system call on this thread (who 0: the caller), no memory passed.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) == 0 }
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn lower_priority() -> bool {
        false
    }

    pub(super) fn delete_at_restart(_path: &std::path::Path) -> bool {
        false
    }

    #[cfg(target_os = "linux")]
    #[allow(unsafe_code)]
    pub(super) fn tie(cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        // (the builder's pid, before the fork: a parent that ended between the fork and the prctl left the child
        // to another parent already, and the death signal asked for after would never come)
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: the closure runs in the child between fork and exec and makes three system calls (prctl, getppid,
        // _exit), each async-signal-safe; it reads a copied pid and touches no memory of the parent's.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn tie(_cmd: &mut std::process::Command) {}

    pub(super) fn adopt(_child: &std::process::Child) {}

    pub(super) fn join_job() -> bool {
        false
    }

    pub(super) fn detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
        cmd.status()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ,
        RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetProcessInformation,
    };

    pub(super) fn available_memory() -> Option<u64> {
        let mut m =
            MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..unsafe { std::mem::zeroed() } };
        // SAFETY: `m` is a MEMORYSTATUSEX of this size, owned here, as the call requires.
        (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then_some(m.ullAvailPhys)
    }

    /// UTF-16 up to its first NUL, as text.
    fn text(w: &[u16]) -> String {
        String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
    }

    pub(super) fn registry(user: bool, path: &str) -> (Vec<(String, String)>, Vec<String>) {
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut key: HKEY = std::ptr::null_mut();
        let root = if user { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
        // SAFETY: `wide` is NUL-ended and lives across the call; `key` is written by it, and closed below.
        if unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
            return (Vec::new(), Vec::new());
        }
        let (mut values, mut subkeys) = (Vec::new(), Vec::new());
        let mut name = vec![0u16; 16_384];
        let mut data = vec![0u8; 65_536];
        for i in 0.. {
            let (mut name_len, mut data_len, mut kind) = (name.len() as u32, data.len() as u32, 0u32);
            // SAFETY: each buffer's length is the one given; nothing is kept past the call.
            let e = unsafe {
                RegEnumValueW(
                    key,
                    i,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if e != 0 {
                break; // (ERROR_NO_MORE_ITEMS, or a value past 64 KB: no driver's is)
            }
            let bytes = &data[..data_len as usize];
            let wide: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let value = match kind {
                REG_SZ | REG_EXPAND_SZ => text(&wide),
                REG_MULTI_SZ => wide
                    .split(|&c| c == 0)
                    .filter(|s| !s.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect::<Vec<_>>()
                    .join("\n"),
                REG_DWORD if bytes.len() == 4 => {
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string()
                }
                _ => String::new(),
            };
            values.push((String::from_utf16_lossy(&name[..name_len as usize]), value));
        }
        for i in 0.. {
            let mut name_len = name.len() as u32;
            // SAFETY: the name buffer's length is the one given; the optional outputs are null, as allowed.
            let e = unsafe {
                RegEnumKeyExW(
                    key,
                    i,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if e != 0 {
                break;
            }
            subkeys.push(String::from_utf16_lossy(&name[..name_len as usize]));
        }
        // SAFETY: the key opened above, closed once.
        unsafe { RegCloseKey(key) };
        (values, subkeys)
    }

    pub(super) fn tree_resident(_pid: u32) -> Option<u64> {
        // (not told: the gate then counts a running job's whole need as still to come, the careful side)
        None
    }

    pub(super) fn lower_priority() -> bool {
        use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, SetPriorityClass};
        // SAFETY: a plain call on this process's own pseudo handle.
        unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) != 0 }
    }

    pub(super) fn stay_awake() {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0,
        };
        // SAFETY: plain calls on this process; `state` lives across the call and its size is the one given.
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }

    pub(super) fn hidden_console() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetConsoleCP, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
            STD_OUTPUT_HANDLE, SetStdHandle,
        };
        // SAFETY: a plain query: 0 when this process has no console.
        if unsafe { GetConsoleCP() } != 0 {
            return;
        }
        let ids = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        // SAFETY: plain queries of this process's standard handles.
        let given = ids.map(|id| unsafe { GetStdHandle(id) });
        // (started from a console (a developer's terminal running the windowed Longshot.exe): that one, so what it
        // says is seen there and what it starts shares it; the handles it was given (a launcher's pipes) kept, the
        // others the console's)
        // SAFETY: joins the parent's console, if it has one; nothing is kept past the call.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
            use windows_sys::Win32::Storage::FileSystem::{
                CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            };
            for (id, handle) in ids.into_iter().zip(given) {
                let given = !handle.is_null() && handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
                let name: Vec<u16> =
                    if id == STD_INPUT_HANDLE { "CONIN$" } else { "CONOUT$" }.encode_utf16().chain([0]).collect();
                // SAFETY: a NUL-ended name living across the call; the handle opened is kept for the process.
                let h = if given {
                    handle
                } else {
                    unsafe {
                        CreateFileW(
                            name.as_ptr(),
                            0xC000_0000, // (GENERIC_READ | GENERIC_WRITE)
                            FILE_SHARE_READ | FILE_SHARE_WRITE,
                            std::ptr::null(),
                            OPEN_EXISTING,
                            0,
                            std::ptr::null_mut(),
                        )
                    }
                };
                // SAFETY: a handle of this process's, set as its standard one.
                unsafe { SetStdHandle(id, h) };
            }
            return;
        }
        // (a console made with no window belongs to a process started so: cmd.exe, waiting on its own input, holds it
        // while this process joins it, then goes; the console stays while this process is in it)
        let system = std::env::var_os("SystemRoot").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from);
        let Ok(mut holder) = std::process::Command::new(system.join("System32").join("cmd.exe"))
            .args(["/d", "/q", "/k"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        else {
            return;
        };
        // (its console is made as it starts: tried again for up to a second)
        for _ in 0..100 {
            // SAFETY: joins the console of a running process of ours; nothing is kept past the call.
            if unsafe { AttachConsole(holder.id()) } != 0 {
                for (id, handle) in ids.into_iter().zip(given) {
                    // SAFETY: the handle this process had there before, given back.
                    unsafe { SetStdHandle(id, handle) };
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = holder.kill();
        let _ = holder.wait();
    }

    pub(super) fn run_game(
        cmd: &mut std::process::Command,
        started: &mut dyn FnMut(u32),
    ) -> std::io::Result<std::process::ExitStatus> {
        use std::os::windows::process::CommandExt;
        // (as detached: out of the builder's job where the job allows it)
        let mut child = match cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
            Err(e) if e.raw_os_error() == Some(5) => cmd.creation_flags(0).spawn()?,
            done => done?,
        };
        started(child.id());
        child.wait()
    }

    pub(super) fn message_box(title: &str, text: &str, error: bool) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW,
        };
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (title, text) = (wide(title), wide(text));
        let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
        // SAFETY: both texts are NUL-ended and live across the call; no owner window.
        let shown = unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | icon | MB_SETFOREGROUND)
        };
        shown != 0
    }

    /// Every process running now: its pid, its parent's pid and its program's file name. Empty when Windows can't say.
    pub(super) fn processes() -> Vec<(u32, u32, String)> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
        };
        // SAFETY: a plain call; the snapshot handle is checked and closed below.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return Vec::new();
        }
        let mut all = Vec::new();
        // SAFETY: a zeroed PROCESSENTRY32W is a valid one; its size is set as the calls require.
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: `e` is owned here and its size is the one set; the snapshot is open.
        let mut more = unsafe { Process32FirstW(snap, &mut e) } != 0;
        while more {
            all.push((e.th32ProcessID, e.th32ParentProcessID, text(&e.szExeFile)));
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap, &mut e) } != 0;
        }
        // SAFETY: the snapshot opened above, closed once.
        unsafe { CloseHandle(snap) };
        all
    }

    pub(super) fn running_programs() -> Vec<String> {
        processes().into_iter().map(|p| p.2).collect()
    }

    pub(super) fn raise_window_of_child(parent: u32) -> bool {
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
            SetForegroundWindow, ShowWindow,
        };
        struct Look {
            pids: Vec<u32>,
            found: HWND,
        }
        unsafe extern "system" fn each(window: HWND, look: LPARAM) -> BOOL {
            // SAFETY: `look` is the `Look` EnumWindows was given below, alive for its whole call; `window` is Windows'.
            unsafe {
                let look = &mut *(look as *mut Look);
                if IsWindowVisible(window) == 0 || !GetWindow(window, GW_OWNER).is_null() {
                    return 1;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(window, &mut pid);
                if look.pids.contains(&pid) {
                    look.found = window;
                    return 0;
                }
                1
            }
        }
        let pids: Vec<u32> = processes().into_iter().filter(|p| p.1 == parent && p.0 != parent).map(|p| p.0).collect();
        if pids.is_empty() {
            return false;
        }
        let mut look = Look { pids, found: std::ptr::null_mut() };
        // SAFETY: the callback reads `look` only while EnumWindows runs, which returns before `look` goes.
        unsafe { EnumWindows(Some(each), &mut look as *mut Look as LPARAM) };
        if look.found.is_null() {
            return false;
        }
        // SAFETY: a window handle Windows just gave; a window that closed since makes the calls fail, nothing worse.
        unsafe {
            if IsIconic(look.found) != 0 {
                ShowWindow(look.found, SW_RESTORE);
            }
            SetForegroundWindow(look.found) != 0
        }
    }

    pub(super) fn delete_at_restart(path: &std::path::Path) -> bool {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated path that outlives the call; a null target asks for a removal.
        let taken = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                wide.as_ptr(),
                std::ptr::null(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_DELAY_UNTIL_REBOOT,
            )
        };
        taken != 0
    }

    /// CREATE_NO_WINDOW: a console program started so opens no window of its own.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub(super) fn tie(cmd: &mut std::process::Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    /// The builder's job: made at its start ([`join_job`]) or its first child, its handle never closed (the process's
    /// end closes it, and with it ends every process in it). A child may leave it when it asks (the game).
    /// `None` when Windows refused one.
    fn job() -> Option<windows_sys::Win32::Foundation::HANDLE> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        // (kept as a number: a HANDLE is a pointer, which a static can't hold)
        static JOB: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        let made = JOB.get_or_init(|| {
            // SAFETY: no attributes and no name, as allowed; the handle is checked below and kept for the process.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return None;
            }
            // SAFETY: a zeroed limit structure is a valid one (no limits); its size is the one given.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            // SAFETY: `limits` lives across the call; the job is the one just made.
            let set = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            (set != 0).then_some(job as usize)
        });
        made.map(|j| j as windows_sys::Win32::Foundation::HANDLE)
    }

    /// Whether the builder itself is in its job ([`join_job`]): its children are then from their creation.
    static IN_JOB: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub(super) fn join_job() -> bool {
        let Some(job) = job() else { return false };
        // SAFETY: the job is the builder's own, open for the process; GetCurrentProcess's pseudo handle needs no close.
        let joined =
            unsafe { windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(job, GetCurrentProcess()) } != 0;
        if joined {
            IN_JOB.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        joined
    }

    /// CREATE_BREAKAWAY_FROM_JOB: a process started outside its parent's job (the job allows it).
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    pub(super) fn detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
        use std::os::windows::process::CommandExt;
        // (ERROR_ACCESS_DENIED: a job around the builder's allows no break away; the game then starts in the job, and
        // the builder waits on it as before)
        match cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB).status() {
            Err(e) if e.raw_os_error() == Some(5) => cmd.creation_flags(0).status(),
            done => done,
        }
    }

    pub(super) fn adopt(child: &std::process::Child) {
        use std::os::windows::io::AsRawHandle;
        if IN_JOB.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let Some(job) = job() else { return };
        // SAFETY: the job is the builder's own, open for the process; the child's handle is open while `child` lives.
        // (refused, the child is not tied: a builder already in a job that takes none inside it, before Windows 8)
        unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(job, child.as_raw_handle());
        }
    }
}
