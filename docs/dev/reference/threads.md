# The renderer's threads

What each of the renderer's threads does, how work passes between them, the orders that hold across them, where they run,
and how to see it in the log. The code is `gpu/vk/vk_replay.cpp` (the render thread and the presenter),
`gpu/vk/vk_recorder.h` (the recorder thread), `gpu/vk/threaded_rendering.h` (whether the render thread and the recorder
thread run), and the Threaded rendering row (`gpu/vk/vk_menu.cpp`
`threaded_choice`). The frame's waits, in time, are in [the frame pipeline](architecture.md#the-frame-pipeline); the switches' rows are
in [switches.md](switches.md).

## The layout

```
 the game's threads ──jobs──▶ render thread: memory work, the replay of every command buffer, every submit
   (submits, presents,              │ each submission's parts submitted as they end (maybe_split)
    captures at submit)             ├─vkCmd* as packets (a ring)──▶ recorder thread ──▶ the driver's vkCmd* calls
                                    └─present (copy done by a fence)──▶ presenter thread ──▶ vkQueuePresentKHR
```

| Thread | Named | Its job |
|---|---|---|
| The game's threads | the game's own (main thread, Presentation Thread, job threads) | Record NVN command streams into guest memory. At each submit the host takes the capture of the pages written since the last one and hands the render thread a job (`submit_q`); a present is a job too, handed over after the main thread's CPU signal for its frame |
| Render thread | `mkw_vk replay` (`replay_thread_main`) | Runs the jobs in hand-over order: the memory work (mirror syncs, write-backs), the replay of each command buffer into recordings, every queue submit, the frame's copy into the window. Off (Threaded rendering Off, or Auto under 4 cores): the replay runs on the game's submitting thread instead |
| Recorder thread | `mkw_vk recorder` (`vk_recorder.h` `Recorder::loop`) | Makes the driver's `vkCmd*` calls the render thread packed into its ring, in order, while the render thread goes on. The render thread waits for it to run everything at each command buffer end, submit, wait, readback and destruction. Threaded rendering On, or Auto from 6 cores (`RX_VK_RECORDER`) |
| Presenter | `mkw_vk presenter` (`presenter_loop`) | Waits for the copy's fence, then makes the present. On every PC |

Not covered here: frame interpolation's display thread (`gpu/vk/vk_framegen.cpp` `display_main`, which makes its own
presents), the background workers that translate shaders and build pipelines (`vk_jobs.cpp`), the capture helpers and
the texture threads.

## Who records

The render thread replays every command buffer; from 6 cores (`threaded::kAutoRecorderCores`) the recorder thread makes
the driver's calls for it.

- **The recorder thread** (`RX_VK_RECORDER=thread`) takes the replay's Vulkan calls as packets through a ring and makes
  them in order. It was removed on 2026-11-19 and brought back the same day: without it the render thread makes every one
  of the driver's calls, and on AMD's drivers (about twice NVIDIA's CPU time a call) the market's densest view, ~2,500
  draws a frame, fell 16 to 25% behind main on an RX 7900 XTX, RX 9070 XT and RX 9060 XT (the Test Farm on #1763). Its
  known costs stand: it wakes 20 to 25 times a frame, and on an i7-9700K those short sleeps let the CPU's clock control
  slow every core (Kokiri Forest 15 fps against 48 with it kept spinning); kept spinning, it cost a Ryzen 9 7900X 3 to
  11%. `RX_VK_RECORDER=off` (or any value but `sync`, `defer` and `thread`) turns it off over the row, for an A/B.
- **The segment workers** (`RX_VK_PARALLEL`, removed on 2026-11-19) recorded a big command buffer's segments in
  parallel, after a dry pass over it for each segment's start state, and the render thread submitted them in order.
  With every core's worth of them they lost on every PC tried: an RTX 3090 / Ryzen 9 7900X 10 to 25%, an RTX 3070 on
  Windows 10 to 35% with an access violation in both runs, an RX 7900 XTX on Linux 3 to 34% (the Test Farm on #1763). A
  Tracy capture on the 7900X showed why: the frame is the game's own draw building on its job threads, the main thread
  waiting for it; two workers made that 28% slower (they shared its cores) while the render thread, busy 22% of the time,
  gained nothing.

Every hand-off is a job or a present, but for the recorder's packets:

- **Game threads to the render thread:** a job per submit, present and CPU event signal, with the submit's capture taken
  first, on the game's thread. `RX_VK_PRESENTS_AHEAD` (1) bounds how far ahead the presents may run.
- **Within a submission:** its parts end where the GPU needs the next one (`maybe_split`: with no render pass open when
  the GPU has a part or less left, at 512 draws and dispatches at the latest), each submitted as it ends, so the GPU
  starts on the first while the rest are recorded.
- **Render thread to the recorder:** each `vkCmd*` call as a packet (its arguments and every array they point to copied),
  run in order; the render thread waits for the ring to empty before any call that ends, submits, waits for or destroys
  what a packet may name, and when it lets the replay's lock go (`DrainingMutex`). A thread without that lock makes its
  calls directly, after the recorder has run what was published.
- **Render thread to the presenter:** the image index and the copy's fence (`present_hand_off`); the render thread goes
  on at once.

## The orders that hold

1. **Cross-queue event waits are honoured on the CPU.** A submission is replayed as far as its event waits allow, with
   per-queue state, and an event's signal is applied on the CPU's view in order (`submit_job`; architecture.md, Rules for resources and presentation,
   rule 1).
2. **Captures are taken at submit, in hand-over order**, on the game's thread, before the job is queued. Nothing the
   replay reads comes from live guest memory (architecture.md, Rules for resources and presentation).
3. **Fence values are handed out at submit, and a value maps to its last part.** A submission split into parts signals
   its timeline value from its last part only; the parts' recording slots go free with it.
4. **A readback waits for the part that writes its range back**, the camera block aside. The culling camera block
   (0x3980 bytes) is read without its wait on every PC (`RX_VK_READBACK_LATE`, `gpu/vk/readback_late.h`): the game reads
   it right after submitting the work that fills it, so on the console it cannot count on that work either, and the
   reads of it were 40 to 45% of the readback wait on a Steam Deck.
5. **One queue, one submission order, under `dev.present_mu`.** Every submit to the replay's queue, and a present on it,
   takes that lock. The presenter thread holds the queue's lock only for its `vkQueuePresentKHR` call, after the copy's
   fence, so the call does not wait for the GPU with the replay's submits held behind it.
6. **Every wait is bounded.** The presenter waits for a copy at most 4 s and a drain at most 5 s; a readback waits at
   most `RX_VK_READBACK_WAIT_MS` (250 ms) for its value; a device lost ends the waits.

The barrier bookkeeping is the recording thread's own (`vk_work.h`: `work_epoch` and `other_work_since_barrier` are
`thread_local`), so a background thread's work never makes its barrier look needed or not.

## Where the threads run

- **The render thread** is pinned to the core the host keeps from the game's threads (`host/hos_thread.c`, on machines
  with 8 or more logical processors; on a hybrid CPU the last core of the fastest kind; `HOS_RESERVE_CORE=0`: none).
- **The presenter** is not pinned: it spends its time waiting on a fence.

## The presenter thread

On every PC since 2026-11-19 (`RX_VK_PRESENT_THREAD` is gone). With NVIDIA's driver on Linux `vkQueuePresentKHR` waits
on the CPU until the frame's copy into the window is done on the GPU, on the frame's critical path: ~2.4 ms a frame on an
RTX 3090 in Kokiri Forest on Wayland (architecture.md, The frame pipeline), 4.5 ms of the copy's fence in [the frame
pipeline](architecture.md#the-frame-pipeline)'s measurement. The presenter waits for the copy's fence itself, then presents on the presents' own queue:
the replay's queue under `dev.present_mu`, or the Present queue row's queue under its own lock (`RX_VK_PRESENT_QUEUE`).
Elsewhere the call returns at once and the thread costs a hand-off.

- No second queue. The presenter once had a queue of its own (a second queue of the replay's family, else a compute
  family's); on the GPU it took turns with the replay, and presents came 2 to 4 ms later ([architecture.md, Frame generation's presents](architecture.md#frame-generations-presents)).
- One present out at most: the next frame's acquire waits until it is made, so the swapchain is one thread's at a time;
  under V-Sync On (FIFO) with `VK_KHR_present_wait`, until it is shown. `VK_KHR_present_id` and `VK_KHR_present_wait`
  are enabled wherever the driver has them.
- With NVIDIA Reflex its present id goes with the present, and the presenter marks the present's end.
- Frame interpolation's presents are made by its display thread, as before.

## Measuring

The log lines, in the order they come:

- `vk: Threaded rendering: ...` at the start: the row, Auto's tier, and the cores and logical processors it counted
  (`Auto: the render thread (...)`).
- `vk: presenter thread: each present made on a thread of its own once its copy is done`, and `vk: the presenter thread
  waits for its presents to be shown: yes (VK_KHR_present_wait)` or `no`. Every 600 presents: `vk: presenter thread: the
  copy's fence X ms, the present Y ms`, with how long after it the frame was shown under FIFO.
- `vk: a submission's parts ended: X where the game had ended its render pass, Y at 512 draws` every 20,000 parts: a
  part ends at the first place the game has ended its render pass once it is `RX_VK_PART_MIN` (120) draws and dispatches
  long (a submission's first part `RX_VK_PART_FIRST`, 32, so the GPU starts sooner), and inside a pass only at
  `RX_VK_PART_MAX` (512).
- `vk: a frame: X full barriers recorded, Y left out (nothing recorded since the one before), Z pass starts ordered in
  their fragment stages only (draw passes after draw passes)` every 120 presents: a draw pass that came after only draw
  passes since the last full barrier on its command buffer waits for them in its fragment stages and its geometry starts
  at once; after a dispatch, copy, fill, blit, image clear or upscaler library's work, a full barrier.
- `vk: readbacks read without their wait: the culling camera block (0x3980) by default (...)` at the first readback.

To compare against the old behaviour in one build, each change has its switch: `RX_VK_PART_FIRST=0 RX_VK_PART_MIN=240
RX_VK_PART_MAX=240` (a part every 240 draws, wherever it falls, as before), `RX_VK_PASS_BARRIER=full` (every draw pass
starts with a full barrier), `RX_VK_READBACK_LATE=0` (every readback waits), `RX_VK_INDEX_REBASE=0` (an indexed draw's
index buffer bound at the draw's own offset, a bind for nearly every draw). `RX_VK_CALL_COUNT=1` logs the replay's
Vulkan calls by kind, a frame and a draw. `RX_VK_GPU_TIMELINE=1` writes each recording's submit and GPU start and end,
the jobs, the game's fence waits and the presents to `longshot_gpu_timeline.csv`. In a Tracy capture
([profiling.md](profiling.md#tracy)) the render thread is `mkw_vk replay` and the presenter is `mkw_vk presenter`. Runs
and their write-up follow the measure skill (`.claude/skills/measure/SKILL.md`).