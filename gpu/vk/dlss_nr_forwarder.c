// SPDX-FileCopyrightText: Copyright 2026 bbport contributors (the Bloodborne PC port, PMWork fork); after the OptiScaler
// DLSS-NR fork's forwarder by Dagherbou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// (longshot) longshot_nvngx.dll_nr.dll, Windows only: makes the calls into the community FP16 conversion of NVIDIA's DLSS 5
// model (nvngx_dlssnr.dll in an ngx folder's fp16 folder) for the renderer's direct path (vk_dlss.cpp NrDirect), and
// nothing else. The builder links it beside longshot_vk.dll (builder/crates/build-compile link_forwarder); it is not
// part of longshot_vk.dll (the plan takes gpu/vk's .cpp files only).
//
// The model looks up the module its caller returns to and refuses every call whose module path lacks "nvngx.dll" (NVIDIA's
// own caller is the driver's _nvngx.dll). This file's name carries it. The renderer passes the model's entry point and its
// arguments; the call is made from here.
//
// Each result goes into a volatile local before it is returned: a plain `return fn(...)` may compile to a tail call (a
// jump), and the model would then see the renderer as its caller. disable_tail_calls says the same to clang. (The OptiScaler
// DLSS-NR fork found both the check and the tail-call trap; tests/test_dlss_nr_forwarder.py checks the caller each call
// sees.) Ported from the Bloodborne port's gpu/dlss_bridge/dlssnr_forwarder.cpp, as C: no C++ runtime in it.

#include <stddef.h>   // (wchar_t)
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#define NR_EXPORT __declspec(dllexport)
#define NR_CALL __cdecl
#else   // (tests/test_dlss_nr_forwarder.py also builds it as a shared library on Linux: the same calls)
#define NR_EXPORT __attribute__((visibility("default")))
#define NR_CALL
#endif
#if defined(__clang__)
#define NR_NO_TAIL __attribute__((disable_tail_calls, noinline))
#else
#define NR_NO_TAIL __attribute__((noinline))
#endif

typedef int(NR_CALL* NrInit)(unsigned long long, const wchar_t*, void*, void*, void*, const void*, int);
typedef int(NR_CALL* NrCreate)(void*, int, void*, void**);
typedef int(NR_CALL* NrEvaluate)(void*, void*, void*, void*);
typedef int(NR_CALL* NrRelease)(void*);

/// The model's NVSDK_NGX_VULKAN_Init_Ext: application id, data folder, the Vulkan instance, physical device and device, the
/// feature info (none) and the SDK version
NR_EXPORT NR_NO_TAIL int NR_CALL longshot_nr_init(void* fn, unsigned long long app, const wchar_t* data, void* instance,
                                                  void* physical, void* device, const void* info, int sdk) {
    volatile int result = ((NrInit)fn)(app, data, instance, physical, device, info, sdk);
    return result;
}

/// The model's NVSDK_NGX_VULKAN_CreateFeature: the command buffer, the feature (18), the parameters and the handle's place
NR_EXPORT NR_NO_TAIL int NR_CALL longshot_nr_create(void* fn, void* command, int feature, void* params, void** handle) {
    volatile int result = ((NrCreate)fn)(command, feature, params, handle);
    return result;
}

/// The model's NVSDK_NGX_VULKAN_EvaluateFeature: the command buffer, the feature, the parameters and no progress callback
NR_EXPORT NR_NO_TAIL int NR_CALL longshot_nr_evaluate(void* fn, void* command, void* handle, void* params, void* callback) {
    volatile int result = ((NrEvaluate)fn)(command, handle, params, callback);
    return result;
}

/// The model's NVSDK_NGX_VULKAN_ReleaseFeature
NR_EXPORT NR_NO_TAIL int NR_CALL longshot_nr_release(void* fn, void* handle) {
    volatile int result = ((NrRelease)fn)(handle);
    return result;
}
