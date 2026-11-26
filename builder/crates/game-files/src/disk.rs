//! The disk a folder is on: its free space, its file system, and whether it is a hard drive.
//!
//! A build reads and writes gigabytes for the best part of an hour, and an SSD makes that much faster. Whether a
//! folder is on a hard drive is only ever a warning: a player may have no other drive. Linux reads the kernel's
//! `queue/rotational` for the device behind the folder (through its partitions and device-mapper/RAID slaves; a Btrfs
//! folder's device number is no disk's, so its disks are those its mount names); Windows asks the volume for its seek
//! penalty.

use std::path::{Path, PathBuf};

/// The nearest folder at or above `path` that exists (the game folder is often made later).
pub fn existing_parent(path: &Path) -> PathBuf {
    let mut p = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    while !p.exists() {
        match p.parent() {
            Some(up) => p = up.to_path_buf(),
            None => break,
        }
    }
    p
}

/// The bytes free for this user on the volume of `path` (or the nearest existing folder above it).
pub fn free_bytes(path: &Path) -> Option<u64> {
    sys::free_bytes(&existing_parent(path))
}

/// The file system of `path`'s volume (NTFS, exFAT, FAT32, ReFS): Windows only.
pub fn file_system(path: &Path) -> Option<String> {
    sys::file_system(&existing_parent(path))
}

/// `Some(true)` on a hard drive, `Some(false)` on an SSD or flash, `None` when it cannot be told (a network share, an
/// unknown device, another system).
pub fn spinning(path: &Path) -> Option<bool> {
    let folder = existing_parent(path);
    #[cfg(target_os = "linux")]
    {
        linux::spinning(&folder, Path::new("/proc/self/mountinfo"), Path::new("/sys"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        sys::seek_penalty(&folder)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::path::{Path, PathBuf};

    fn node(sysfs: &Path, dev: u64) -> PathBuf {
        sysfs.join("dev/block").join(format!("{}:{}", libc::major(dev), libc::minor(dev)))
    }

    pub(super) fn spinning(path: &Path, mountinfo: &Path, sysfs: &Path) -> Option<bool> {
        let dev = std::fs::metadata(path).ok()?.dev();
        let n = node(sysfs, dev);
        if n.exists() {
            return rotational(&n);
        }
        // (Btrfs numbers its files' device itself, a number no disk has: the mount's source says which disks)
        let answers: Vec<bool> = mounted_from(path, mountinfo, sysfs).iter().filter_map(|n| rotational(n)).collect();
        if answers.is_empty() { None } else { Some(answers.iter().any(|&a| a)) }
    }

    /// A mountinfo field as the path it stands for (`\040` a space, `\011` a tab, `\012` a newline, `\134` a backslash).
    pub(super) fn unescaped(field: &str) -> String {
        let mut out = String::new();
        let mut rest = field;
        while let Some(i) = rest.find('\\') {
            out.push_str(&rest[..i]);
            let code = rest.get(i + 1..i + 4).filter(|c| c.bytes().all(|b| b.is_ascii_digit()));
            match code.and_then(|c| u8::from_str_radix(c, 8).ok()) {
                Some(b) => {
                    out.push(b as char);
                    rest = &rest[i + 4..];
                }
                None => {
                    out.push('\\');
                    rest = &rest[i + 1..];
                }
            }
        }
        out + rest
    }

    fn mounted_from(path: &Path, mountinfo: &Path, sysfs: &Path) -> Vec<PathBuf> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path = path.to_string_lossy();
        let Ok(text) = std::fs::read_to_string(mountinfo) else { return Vec::new() };
        let mut found: Option<(String, String, String)> = None;
        for line in text.lines() {
            // (id, parent, device, root, mount point, options... - type, source, options)
            let Some((mount, source)) = line.split_once(" - ") else { continue };
            let (mount, source): (Vec<&str>, Vec<&str>) = (mount.split(' ').collect(), source.split(' ').collect());
            if mount.len() < 5 || source.len() < 2 {
                continue;
            }
            let point = unescaped(mount[4]);
            let inside = path == point || path.starts_with(&format!("{}/", point.trim_end_matches('/')));
            // (the deepest; of two at one point, the later is on top)
            if inside && found.as_ref().is_none_or(|f| point.len() >= f.0.len()) {
                found = Some((point, source[0].to_string(), unescaped(source[1])));
            }
        }
        let Some((_, kind, device)) = found else { return Vec::new() };
        let Ok(meta) = std::fs::metadata(&device) else { return Vec::new() };
        if !meta.file_type().is_block_device() {
            return Vec::new();
        }
        let n = node(sysfs, meta.rdev());
        if kind == "btrfs"
            && let Ok(ids) = std::fs::read_dir(sysfs.join("fs/btrfs"))
        {
            let me = std::fs::canonicalize(&n).ok();
            for id in ids.flatten() {
                let Ok(devs) = std::fs::read_dir(id.path().join("devices")) else { continue };
                let mut nodes: Vec<PathBuf> = devs.flatten().map(|d| d.path()).collect();
                nodes.sort();
                if nodes.iter().any(|d| std::fs::canonicalize(d).ok() == me) {
                    return nodes;
                }
            }
        }
        vec![n]
    }

    /// The device behind a sysfs node: its slaves decide for device-mapper and RAID (any spinning one makes it slow);
    /// a partition has no `queue/rotational` of its own, so the disk above it is read. Without recursion; a loop of
    /// slaves has no answer.
    pub(super) fn rotational(node: &Path) -> Option<bool> {
        use std::collections::{HashMap, HashSet};
        let root = std::fs::canonicalize(node).ok()?;
        let mut answers: HashMap<PathBuf, Option<bool>> = HashMap::new();
        let mut active = HashSet::new();
        let mut pending: Vec<(PathBuf, Option<Vec<PathBuf>>)> = vec![(root.clone(), None)];
        while let Some((path, children)) = pending.pop() {
            match children {
                None => {
                    if answers.contains_key(&path) || active.contains(&path) {
                        continue;
                    }
                    active.insert(path.clone());
                    let mut kids: Vec<PathBuf> = std::fs::read_dir(path.join("slaves"))
                        .map(|d| d.flatten().filter_map(|e| std::fs::canonicalize(e.path()).ok()).collect())
                        .unwrap_or_default();
                    kids.sort();
                    pending.push((path, Some(kids.clone())));
                    pending.extend(kids.into_iter().rev().map(|k| (k, None)));
                }
                Some(kids) => {
                    let known: Vec<bool> = kids.iter().filter_map(|k| answers.get(k).copied().flatten()).collect();
                    let mut answer = if known.is_empty() { None } else { Some(known.iter().any(|&a| a)) };
                    if known.is_empty() {
                        let mut at = Some(path.as_path());
                        while let Some(p) = at {
                            if let Ok(v) = std::fs::read_to_string(p.join("queue/rotational")) {
                                answer = Some(v.trim() == "1");
                                break;
                            }
                            at = p.parent();
                        }
                    }
                    active.remove(&path);
                    answers.insert(path, answer);
                }
            }
        }
        answers.get(&root).copied().flatten()
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod sys {
    use std::path::Path;

    pub(super) fn free_bytes(path: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `c` is a NUL-terminated path and `st` a statvfs the call fills; it is read only when the call succeeds.
        if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: filled by the successful call above.
        let st = unsafe { st.assume_init() };
        // (their types are the platform's: u64 on Linux, narrower elsewhere)
        #[allow(clippy::unnecessary_cast)]
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }

    pub(super) fn file_system(_path: &Path) -> Option<String> {
        None
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn seek_penalty(_path: &Path) -> Option<bool> {
        None
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetVolumeInformationW, GetVolumePathNameW,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_QUERY,
        StorageDeviceSeekPenaltyProperty,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn volume_root(path: &Path) -> Option<Vec<u16>> {
        let p = wide(path);
        let mut root = vec![0u16; 1024];
        // SAFETY: `p` is NUL-terminated; `root` holds the 1024 units the call is told of.
        (unsafe { GetVolumePathNameW(p.as_ptr(), root.as_mut_ptr(), root.len() as u32) } != 0).then_some(root)
    }

    pub(super) fn free_bytes(path: &Path) -> Option<u64> {
        let p = wide(path);
        let mut free = 0u64;
        // SAFETY: `p` is NUL-terminated; `free` is written by the call; the other outputs are not asked for.
        (unsafe { GetDiskFreeSpaceExW(p.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } != 0)
            .then_some(free)
    }

    pub(super) fn file_system(path: &Path) -> Option<String> {
        let root = volume_root(path)?;
        let mut name = [0u16; 261];
        // SAFETY: `root` is NUL-terminated; `name` holds the 261 units the call is told of; the rest are not asked for.
        let ok = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..n]))
    }

    /// The volume's seek penalty (IOCTL_STORAGE_QUERY_PROPERTY): a spinning disk incurs one.
    pub(super) fn seek_penalty(path: &Path) -> Option<bool> {
        let root = volume_root(path)?;
        let n = root.iter().position(|&c| c == 0)?;
        // (\\.\C: : the volume as a device, from the root "C:\")
        let drive: Vec<u16> = "\\\\.\\"
            .encode_utf16()
            .chain(root[..n].iter().copied().filter(|&c| c != u16::from(b'\\')))
            .chain(Some(0))
            .collect();
        // SAFETY: `drive` is NUL-terminated; no security attributes or template.
        let h = unsafe {
            CreateFileW(
                drive.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return None;
        }
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceSeekPenaltyProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut answer = DEVICE_SEEK_PENALTY_DESCRIPTOR { Version: 0, Size: 0, IncursSeekPenalty: 0 };
        let mut returned = 0u32;
        // SAFETY: `h` is open; the buffers are these locals with the sizes given; no overlapped I/O.
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                (&mut answer as *mut DEVICE_SEEK_PENALTY_DESCRIPTOR).cast(),
                std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        // SAFETY: `h` was opened above and is closed once.
        unsafe { CloseHandle(h) };
        (ok != 0 && returned as usize >= std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>())
            .then_some(answer.IncursSeekPenalty != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_folder_is_measured_by_its_parent() {
        let p = std::env::temp_dir().join("game-files-no-such/a/b");
        assert_eq!(existing_parent(&p), std::env::temp_dir());
        assert!(free_bytes(&p).is_some_and(|b| b > 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_answers() {
        use std::fs;
        let d = std::env::temp_dir().join(format!("game-files-sysfs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        // a disk sda (rotational) with a partition sda1, a mapper dm-0 over sda1 and nvme0n1p1 (not rotational)
        for p in ["block/sda/sda1", "block/nvme0n1/nvme0n1p1", "block/dm-0/slaves"] {
            fs::create_dir_all(d.join(p)).unwrap();
        }
        for (p, v) in [("block/sda/queue", "1"), ("block/nvme0n1/queue", "0")] {
            fs::create_dir_all(d.join(p)).unwrap();
            fs::write(d.join(p).join("rotational"), v).unwrap();
        }
        assert_eq!(linux::rotational(&d.join("block/sda/sda1")), Some(true));
        assert_eq!(linux::rotational(&d.join("block/nvme0n1/nvme0n1p1")), Some(false));
        std::os::unix::fs::symlink(d.join("block/sda/sda1"), d.join("block/dm-0/slaves/sda1")).unwrap();
        std::os::unix::fs::symlink(d.join("block/nvme0n1/nvme0n1p1"), d.join("block/dm-0/slaves/nvme0n1p1")).unwrap();
        assert_eq!(linux::rotational(&d.join("block/dm-0")), Some(true), "any spinning slave");
        assert_eq!(linux::unescaped(r"/mnt/My\040Games\134x"), "/mnt/My Games\\x");
        fs::remove_dir_all(&d).unwrap();
    }
}
