//! What this PC's Vulkan says, asked as the game asks it at its start (gpu/vk/vk_main.cpp `create_instance` and
//! `pick_device`): the loader's version, then an instance and each GPU's properties, its driver's name and version.
//! `longshot-builder doctor` asks it in a process of its own, so a driver that crashes or hangs in the asking is said,
//! not fatal.
//!
//! The loader (`libvulkan.so.1`, `vulkan-1.dll`) is loaded at run time and only `vkGetInstanceProcAddr` taken from it:
//! the builder links no Vulkan. This module, like `sys.rs`, allows `unsafe`: each call is a plain Vulkan query on
//! structures this module owns, sized and typed as the Vulkan headers say.
#![allow(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void};

/// A GPU as its Vulkan driver lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    /// Its name (`deviceName`).
    pub name: String,
    /// The Vulkan version it offers (`apiVersion`, Vulkan's packed form).
    pub api: u32,
    /// The driver's version, as its vendor packs it (`driverVersion`).
    pub driver_version: u32,
    /// The PCI vendor id (0x10de NVIDIA, 0x1002 AMD, 0x8086 Intel).
    pub vendor: u32,
    /// The PCI device id.
    pub device: u32,
    /// `VkPhysicalDeviceType`: 1 integrated, 2 discrete, 3 virtual, 4 the CPU (a software renderer).
    pub kind: u32,
    /// `VkDriverId` (0 when the driver doesn't say: Vulkan before 1.2).
    pub driver_id: u32,
    /// The driver's name (`driverName`: "NVIDIA", "radv", "Intel open-source Mesa driver").
    pub driver_name: String,
    /// The driver's own version text (`driverInfo`: "580.95.05", "Mesa 25.2.4").
    pub driver_info: String,
}

/// What the loader and the drivers said.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vulkan {
    /// The loader's file, when one was found.
    pub loader_file: Option<String>,
    /// The loader's Vulkan version (Vulkan's packed form), when one was found.
    pub loader: Option<u32>,
    /// Whether an instance could be made.
    pub instance: bool,
    /// `vkCreateInstance`'s result (0: made; -9, `VK_ERROR_INCOMPATIBLE_DRIVER`: no driver for it).
    pub instance_result: i32,
    /// The GPUs listed.
    pub gpus: Vec<Gpu>,
}

/// Vulkan's packed version: major, minor.
pub const fn version(major: u32, minor: u32) -> u32 {
    (major << 22) | (minor << 12)
}

/// A packed Vulkan version as `major.minor`.
pub fn version_text(v: u32) -> String {
    format!("{}.{}", (v >> 22) & 0x7F, (v >> 12) & 0x3FF)
}

#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    next: *const c_void,
    application_name: *const c_char,
    application_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    next: *const c_void,
    flags: u32,
    application_info: *const ApplicationInfo,
    layer_count: u32,
    layers: *const *const c_char,
    extension_count: u32,
    extensions: *const *const c_char,
}

/// `VkPhysicalDeviceProperties`: the fields read, then room past the rest (the limits and the sparse properties, 532
/// bytes of the 824).
#[repr(C)]
struct Properties {
    api: u32,
    driver_version: u32,
    vendor: u32,
    device: u32,
    kind: u32,
    name: [u8; 256],
    cache_uuid: [u8; 16],
    rest: [u64; 128],
}

#[repr(C)]
struct DriverProperties {
    s_type: u32,
    next: *mut c_void,
    driver_id: u32,
    name: [u8; 256],
    info: [u8; 256],
    conformance: [u8; 4],
}

#[repr(C)]
struct Properties2 {
    s_type: u32,
    next: *mut c_void,
    properties: Properties,
}

const APPLICATION_INFO: u32 = 0;
const INSTANCE_CREATE_INFO: u32 = 1;
const PHYSICAL_DEVICE_PROPERTIES_2: u32 = 1_000_059_001;
const PHYSICAL_DEVICE_DRIVER_PROPERTIES: u32 = 1_000_196_000;

type Void = unsafe extern "system" fn();
type GetInstanceProcAddr = unsafe extern "system" fn(*mut c_void, *const c_char) -> Option<Void>;
type EnumerateInstanceVersion = unsafe extern "system" fn(*mut u32) -> i32;
type CreateInstance = unsafe extern "system" fn(*const InstanceCreateInfo, *const c_void, *mut *mut c_void) -> i32;
type DestroyInstance = unsafe extern "system" fn(*mut c_void, *const c_void);
type EnumeratePhysicalDevices = unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32;
type GetProperties = unsafe extern "system" fn(*mut c_void, *mut Properties);
type GetProperties2 = unsafe extern "system" fn(*mut c_void, *mut Properties2);

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]).into_owned()
}

/// The loader opened: its file's name and its `vkGetInstanceProcAddr`.
#[cfg(target_os = "linux")]
fn loader() -> Option<(String, GetInstanceProcAddr)> {
    for name in [c"libvulkan.so.1", c"libvulkan.so"] {
        // SAFETY: a NUL-ended name; the library stays loaded for the process (never closed: some drivers crash
        // when unloaded).
        let lib = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            continue;
        }
        // SAFETY: a NUL-ended symbol name in a library just opened.
        let f = unsafe { libc::dlsym(lib, c"vkGetInstanceProcAddr".as_ptr()) };
        if !f.is_null() {
            // SAFETY: the loader's vkGetInstanceProcAddr has this signature (Vulkan's PFN_vkGetInstanceProcAddr).
            return Some((name.to_string_lossy().into_owned(), unsafe {
                std::mem::transmute::<*mut c_void, GetInstanceProcAddr>(f)
            }));
        }
    }
    None
}

#[cfg(windows)]
fn loader() -> Option<(String, GetInstanceProcAddr)> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let name: Vec<u16> = "vulkan-1.dll".encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-ended name; the library stays loaded for the process.
    let lib = unsafe { LoadLibraryW(name.as_ptr()) };
    if lib.is_null() {
        return None;
    }
    // SAFETY: a NUL-ended symbol name in a library just loaded.
    let f = unsafe { GetProcAddress(lib, c"vkGetInstanceProcAddr".as_ptr().cast()) }?;
    // SAFETY: the loader's vkGetInstanceProcAddr has this signature (Vulkan's PFN_vkGetInstanceProcAddr).
    Some(("vulkan-1.dll".into(), unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, GetInstanceProcAddr>(f)
    }))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn loader() -> Option<(String, GetInstanceProcAddr)> {
    None
}

/// What this PC's Vulkan says now ([`Vulkan`]). It loads the drivers: a broken one may crash or hang here, so a caller
/// that must go on asks it in a process of its own.
pub fn probe() -> Vulkan {
    let mut out = Vulkan::default();
    let Some((file, gipa)) = loader() else { return out };
    out.loader_file = Some(file);
    /// A function of the loader's, by name, for `instance` (null: a global one).
    fn get<T>(gipa: GetInstanceProcAddr, instance: *mut c_void, name: &std::ffi::CStr) -> Option<T> {
        // SAFETY: a NUL-ended name; the loader answers null for a function it doesn't have. T is the function's
        // pointer type, as Vulkan's PFN for that name says (each caller's).
        let f = unsafe { gipa(instance, name.as_ptr()) }?;
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<Void>());
        // SAFETY: as above: a function pointer of the same size, of the type its name has.
        Some(unsafe { std::mem::transmute_copy::<Void, T>(&f) })
    }
    let null = std::ptr::null_mut();
    let mut loader_version = version(1, 0);
    if let Some(f) = get::<EnumerateInstanceVersion>(gipa, null, c"vkEnumerateInstanceVersion") {
        // SAFETY: a u32 this function owns, written by the call.
        unsafe { f(&mut loader_version) };
    }
    out.loader = Some(loader_version);
    let Some(create) = get::<CreateInstance>(gipa, null, c"vkCreateInstance") else { return out };
    let app = ApplicationInfo {
        s_type: APPLICATION_INFO,
        next: std::ptr::null(),
        application_name: c"longshot-builder doctor".as_ptr(),
        application_version: 0,
        engine_name: std::ptr::null(),
        engine_version: 0,
        // (the game asks for 1.3; a loader of before 1.1 refuses any version past 1.0)
        api_version: loader_version.min(version(1, 3)),
    };
    let info = InstanceCreateInfo {
        s_type: INSTANCE_CREATE_INFO,
        next: std::ptr::null(),
        flags: 0,
        application_info: &app,
        layer_count: 0,
        layers: std::ptr::null(),
        extension_count: 0,
        extensions: std::ptr::null(),
    };
    let mut instance: *mut c_void = null;
    // SAFETY: the create info and the application info live across the call; no allocator; `instance` is written.
    let made = unsafe { create(&info, std::ptr::null(), &mut instance) };
    out.instance_result = made;
    if made != 0 || instance.is_null() {
        return out;
    }
    out.instance = true;
    let enumerate = get::<EnumeratePhysicalDevices>(gipa, instance, c"vkEnumeratePhysicalDevices");
    let properties = get::<GetProperties>(gipa, instance, c"vkGetPhysicalDeviceProperties");
    let properties2 = if app.api_version >= version(1, 1) {
        get::<GetProperties2>(gipa, instance, c"vkGetPhysicalDeviceProperties2")
    } else {
        None
    };
    if let (Some(enumerate), Some(properties)) = (enumerate, properties) {
        let mut n = 0u32;
        // SAFETY: the count's query (a null list), on the instance made above.
        unsafe { enumerate(instance, &mut n, std::ptr::null_mut()) };
        let mut devices = vec![std::ptr::null_mut::<c_void>(); n as usize];
        // SAFETY: a list of `n` handles, as the count said; the call writes at most `n`.
        unsafe { enumerate(instance, &mut n, devices.as_mut_ptr()) };
        devices.truncate(n as usize);
        for d in devices {
            let mut driver = DriverProperties {
                s_type: PHYSICAL_DEVICE_DRIVER_PROPERTIES,
                next: std::ptr::null_mut(),
                driver_id: 0,
                name: [0; 256],
                info: [0; 256],
                conformance: [0; 4],
            };
            let blank = Properties {
                api: 0,
                driver_version: 0,
                vendor: 0,
                device: 0,
                kind: 0,
                name: [0; 256],
                cache_uuid: [0; 16],
                rest: [0; 128],
            };
            let mut p2 = Properties2 {
                s_type: PHYSICAL_DEVICE_PROPERTIES_2,
                next: (&mut driver as *mut DriverProperties).cast(),
                properties: blank,
            };
            match properties2 {
                // SAFETY: a device of this instance; the chain's structures live across the call, each at least the
                // size Vulkan writes.
                Some(f) => unsafe { f(d, &mut p2) },
                // SAFETY: as above, without the chain.
                None => unsafe { properties(d, &mut p2.properties) },
            }
            let p = &p2.properties;
            out.gpus.push(Gpu {
                name: text(&p.name),
                api: p.api,
                driver_version: p.driver_version,
                vendor: p.vendor,
                device: p.device,
                kind: p.kind,
                driver_id: driver.driver_id,
                driver_name: text(&driver.name),
                driver_info: text(&driver.info),
            });
        }
    }
    if let Some(destroy) = get::<DestroyInstance>(gipa, instance, c"vkDestroyInstance") {
        // SAFETY: the instance made above, destroyed once; no allocator.
        unsafe { destroy(instance, std::ptr::null()) };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_pack_as_vulkan_packs_them() {
        assert_eq!(version(1, 3), 0x0040_3000);
        assert_eq!(version_text(version(1, 3) | 289), "1.3");
        assert_eq!(version_text(version(1, 4)), "1.4");
        assert_eq!(std::mem::offset_of!(Properties, name), 20);
        assert_eq!(std::mem::offset_of!(Properties, cache_uuid), 276);
        assert!(std::mem::size_of::<Properties>() >= 824, "Vulkan's VkPhysicalDeviceProperties fits");
        assert_eq!(std::mem::offset_of!(Properties2, properties), 16);
        assert_eq!(std::mem::offset_of!(DriverProperties, info), 276);
    }

    #[test]
    fn a_probe_s_answer_goes_through_json_whole() {
        let v = Vulkan {
            loader_file: Some("libvulkan.so.1".into()),
            loader: Some(version(1, 4)),
            instance: true,
            instance_result: 0,
            gpus: vec![Gpu { name: "GPU".into(), api: version(1, 3), kind: 2, ..Gpu::default() }],
        };
        let back: Vulkan = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
    }
}
