# DLSS 5

DLSS 5 is NVIDIA's neural rendering model. It adds detail to faces, skin and surfaces after the upscaler, before the HUD.
It costs a lot of GPU time: about 8.9 ms a frame at 3416x1440 on an RTX 5090.

NVIDIA's own file holds its FP8 model, which runs on RTX 50 GPUs only. On Windows, RTX 20, 30 and 40 GPUs can run DLSS
5 with the community FP16 conversion of that file ([below](#rtx-20-30-and-40-the-fp16-file)).

## Put NVIDIA's file in place

Longshot does not include or download the model. Put NVIDIA's `nvngx_dlssnr.dll` in one of these folders:

- the game's `ngx` folder, beside `nvngx_dlss.dll`;
- `%LOCALAPPDATA%\Longshot\ngx`, which a Launcher update does not touch;
- the folder `RX_VK_DLSS_DIR` names.

Start the game. Longshot looks for the file once at each start. NVIDIA's runtime loads only a file that NVIDIA signed: a
changed or converted file does not start there. NVIDIA's FP8 model runs on RTX 50 GPUs.

## RTX 20, 30 and 40: the FP16 file

On Windows, Longshot also runs the community FP16 conversion of `nvngx_dlssnr.dll` (a modified NVIDIA file: it runs on
RTX 20, 30, 40 and 50 GPUs). Longshot does not include or download it either. Put it, named `nvngx_dlssnr.dll`, in an
`fp16` folder inside one of the folders above, for example `%LOCALAPPDATA%\Longshot\ngx\fp16\nvngx_dlssnr.dll`.

- Longshot uses only the known file: its SHA-256 must be
  `CDFA6C1EC784867182164C7836F324F087B5C9F374D0C9FE46F54999FEABF3ED` (276,162,560 bytes, the file the Bloodborne PC port
  checks for). Any other file there is not loaded (one of another size is not even read); the log says so. Longshot
  checks the file once at each start, on a thread of its own (about 0.6 s on a Ryzen 7 9800X3D; until then the DLSS 5
  row's line says it waits), and no program can change, move or delete it while the game runs: the file loaded is the
  file checked.
- NVIDIA's runtime refuses a modified file, so Longshot loads this one itself and calls it through
  `longshot_nvngx.dll_nr.dll`, beside `longshot_vk.dll` (a build with NVIDIA DLSS makes it). It does not turn off any
  check of NVIDIA's and changes no setting of Windows.
- Linux: no. The FP16 file is a Windows file.

Which file runs:

| GPU | Files in place | Runs | DLSS 5 model row |
|---|---|---|---|
| RTX 50 | both | the row's choice: Auto (NVIDIA's), NVIDIA or FP16 | shown, applies after a restart |
| RTX 50 | one | that one | not shown |
| RTX 20, 30, 40 | the FP16 file (with or without NVIDIA's) | the FP16 file | not shown |
| RTX 20, 30, 40 | NVIDIA's only | nothing: the DLSS 5 row's line says NVIDIA's file does not run there and names the FP16 file | not shown |

`RX_VK_DLSS_NR_MODEL=auto`, `nvidia` or `fp16` sets the DLSS 5 model row for one run (only where the row shows).

## The rows

The **DLSS 5** rows of the Graphics tab, under the Upscaler's rows (after Sharpness), show only when all of these are true:

- the file (NVIDIA's, or on Windows the FP16 one) is in one of the folders above;
- the GPU is an NVIDIA GPU;
- the build has NVIDIA DLSS;
- the Upscaler is NVIDIA DLSS, NVIDIA DLAA or AMD FSR.

Without them there is no DLSS 5 row and no message. Turn **DLSS 5** On: **DLSS 5 preset** and **DLSS 5 settings** show,
and **DLSS 5 model** on an RTX 50 GPU with both files ([above](#rtx-20-30-and-40-the-fp16-file)).
**DLSS 5 settings** opens a page with the tuning rows below (Back returns to Graphics). The DLSS 5 row's line says what
the model does now, or why it does not run.

The rows are the DLSS 5 rows of the Bloodborne PC port. **DLSS 5 preset** is **Default**: the values below, tuned in
the game (Kokiri Forest, an RTX 5090 at 5120x2160 with DLAA). Change a row and the preset says **Custom**; choose
**Default** to put all of them back.

| Row (DLSS 5 settings page) | What it does | Default |
|---|---|---|
| Model resolution | The size the model works at, as a share of the picture. Lower is much faster: the model sees an averaged copy and only its edit is scaled back up | 65% |
| Detail strength | How far the picture moves toward the model's: 0 the upscaler's alone, 1 the model's, above 1 further still | 1.64 |
| Colour strength | 0 the game's own colour with the model's light, 1 the model's colour too, above 1 more saturated (as much as Detail strength takes of the model's picture, up to 1: none at Detail strength 0) | 1.26 |
| Style | Standard (the strongest), Natural (gentler skin tones), Cinematic (less shine, a film-like look) | Cinematic |
| Model strengths | Whether the next four rows reach the model. As in OptiScaler and Model defaults: the model's own values. Applied: the rows' values | Applied |
| Intensity | The model's own strength (NVIDIA's default 1) | 1.62 |
| Local structure | The fine structure the model adds | 1.22 |
| Local tone | The local contrast and shading the model adds | 1.14 |
| Skin structure | The structure on skin: Follows local structure, or a value of its own | 0.74 |
| Automatic skin mask | The model finds skin by itself to treat it apart | On |
| Highlight guard | The most the model may brighten or darken a pixel, as a multiple: keeps bright lights from breaking up | 1.7x |

Detail strength, colour strength and the highlight guard change at once. The other rows make the model again, which
takes a moment; the sliders among them change when you let go.

With the FPS counter at its Dev level, **NR time** under GPU time is the model's own GPU time a frame (not the
composition after it), and **NR VRAM** under the VRAM line is the memory of the images Longshot makes for it (NVIDIA's
runtime allocates more of its own).

## Switches

`RX_VK_DLSS_NR=0/1` turns DLSS 5 off or on for one run. `RX_VK_DLSS_NR_DEBUG=1..5` shows its input, the model's raw
output, the difference x20, depth or motion vectors, for one run. `RX_VK_DLSS_NR_MODEL=auto/nvidia/fp16` chooses the
file on an RTX 50 GPU with both, for one run. On Linux the rows show only with
`RX_VK_DLSS_NR_LINUX=1`, and the native NGX must offer neural rendering: the Windows file cannot be used there.

## How it works

DLSS 5 is NGX feature 18, made and evaluated through NGX's own API (`gpu/vk/vk_dlss.cpp`, `dlss_nr_chain`), with the
NVIDIA-signed file NGX finds in the folders above. For that file Longshot loads nothing itself and changes nothing in
NVIDIA's files.

The FP16 file takes a second path (`NrDirect` in `gpu/vk/vk_dlss.cpp`, Windows only), as the Bloodborne PC port's DLSS
bridge runs it (PMWork fork; after the OptiScaler DLSS-NR fork by Dagherbou): Longshot loads the file, starts it with
its own `NVSDK_NGX_VULKAN_Init_Ext`, and makes, evaluates and lets go of its feature with its own exports, with the same
`DLSSNR.*` parameters in a block NGX allocates. The model refuses a caller whose module name lacks `nvngx.dll`, so each
call goes through `longshot_nvngx.dll_nr.dll` (`gpu/vk/dlss_nr_forwarder.c`), which only makes the call. Everything
around the model (its inputs, the proxy, the composition, the rows) is the same for both files.
The model runs over the upscaler's picture, before the HUD, with the scene's depth and motion at its size. Its answer
comes back through a composition (`gpu/vk/shaders/dlss_nr_composite.comp`): the detail and colour strengths and the
highlight guard, as the Bloodborne port composes it, except that the extra saturation above colour strength 1 follows
the detail strength up to 1, so detail strength 0 is the upscaler's picture. Below 100% model resolution, and with HDR
colour (DLAA before the bloom, Native HDR), the model sees a proxy and only its edit is laid over the picture. After the
upscaler the picture has no jitter, so the model keeps its history from frame to frame; it starts again only on a cut or
a change of size.

Feature 18 and the `DLSSNR.*` parameter names are not in NVIDIA's public NGX SDK. They come from HypeZ85's Hookshot
[pull request 1590](https://github.com/NotRexed/Hookshot/pull/1590) (commit `a1f730de`), which TheDevil brought to Longshot
(#173, #619, #620); the public [OptiScaler DLSS-NR fork](https://github.com/Dagherbou/OptiScaler_DLSSNR) (Dagherbou,
GPL-3.0) drives the model the same way. The composition follows the Bloodborne PC port's, which follows that fork's; its
blend between the picture's own colour and the model's follows RenoDX's DLSS 5 addon (clshortfuse). NVIDIA
[says](https://forums.developer.nvidia.com/t/official-dlss-5-neural-rendering-plugin-access-for-unreal-engine-5-8-2/382731)
the DLSS 5 SDK is not public yet.
