# Hook sites

Longshot changes the game's code at **hook sites**: places in the game's functions, listed as data (what each site is,
once, and where it is, per game build), that the lift finds by their instructions and writes into the function's mid
IR as it builds it ([the builder](builder.md#the-whole-lift)). Nothing matches lifted text: a site that does not
resolve stops the build, naming it, where a text patch over lifted code was silently left out once the lift no longer
wrote its line. What is done another way, and why, is [below](#what-is-done-another-way-and-why).

| What | Where |
|---|---|
| The sites' definitions: what each is, its form and its callback's arguments, the same in every build | `game/sites/sites.toml` |
| Where each is, one file per supported game build | `game/sites/<build id>/locations.toml` |
| The reader, the resolver and the placement | `builder/crates/sites`, called by the lift (`lift::whole`) after discovery |
| Each form as mid IR, over the host's ABI | `builder/crates/edits/src/site.rs`, written in by the function builder (`builder/crates/lift`) |
| Each site's flag byte and callback slot, and the handlers on it | `game/sites.def` (the list), `game/sites.c`, `game/sites.h` |
| The checks | `cargo test -p sites`, `cargo test -p edits`, `cargo test -p lift` (game-free tables and functions), and each feature's own: its callback run behind its hook as the lift lowers it (`tests/sitehooks.py`, `longshot-builder dev site-hooks`: [the builder](builder.md#building-and-checking-the-builder)) |
| Skip title screen's (`menu.buttonWait`, `menu.buttonWaitNoDecideAnim`: the enter of the sequence nodes that wait for a screen's button, which tell the screen's name; the title's, the file list's and the check's get A, on every user of the skip: the row, `dev.py ab`, the game's test mode) | `game/oot_autoload.c`, `game/oot_sites.c`; how they were found: the table's comment |

The build id is main's 40-digit build id in lower case, as the lift reads it from the image (the supported dump's is
`e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348`, `e0f3ccfa` for short). A build with no locations file gets no hooks, and
none of the supported build's own patches. A new game build needs only its own `locations.toml`: the definitions,
the host's callbacks and their contract stay as they are.

## The format

A site is written twice, in two files. Its **definition**, in `game/sites/sites.toml`, says what the site is and what its
callback may expect. That is the same in every game build. Its **location**, in `game/sites/<build id>/locations.toml`,
says only where the site is in that build:

```toml
# game/sites/sites.toml
format = 2

[[site]]
name = "text.letterStep"
doc = "A text pane's letter animation is about to step: its letter count loaded, the letters' time in s0"
at = "after"
shape = "data"
thread = "worker"
args = ["x19:ptr:anim", "w9:u32:letters", "s0:f32:time"]
```

```toml
# game/sites/e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348/locations.toml
format = 2
build = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"   # (the folder's name)

[[site]]
name = "text.letterStep"
function = "f_80b1e140"
offset = 0x74                      # (ldrh w9, [x20, #0x48]; then cbz w9 to the return)
signature_offset = 0x74
signature = "79409289 34000009/ff00001f 37100008/fff8001f"   # (ldrh w9, [x20, #0x48]; cbz w9, *; tbnz w8, #2, *)
```

That is the shape of the supported build's first site, the Insert menu's Text speed and Text size (`game/oot_text_step.c`),
with a made-up signature in place of the table's. The order of
the definitions is the order of several hooks at one instruction. The locations may come in any order, but are kept in
the same one.

| Key | In | What | |
|---|---|---|---|
| `name` | both | dot-separated words, each a lower-case letter then letters and digits; its C identifier is the name with `_` for `.` (`text_letterStep`) | required, unique |
| `doc` | definition | one line: what has happened, or is about to, when the site is reached | required |
| `kind` | definition | `code` (found in the code), or `vslot` (below) | `code` |
| `at` | definition | the form: `entry`, `return`, `after`, `before`, `replace`, `branch`, `allow` or `jump` ([below](#forms)) | `entry` |
| `can_return` | definition | `entry` only: the callback's non-zero answer returns from the function at once | `false` |
| `rule` | definition | an inline data rule's template ([below](#inline-data-rules)) in place of a callback; `shape = "data"` | |
| `value` | definition | a rule's host value, by name (`game/sites.def`'s `HOS_VALUE` lines) | |
| `shape` | definition | `event` (tells), `filter` (may change the outcome; a serial site only) or `data` (a rule applied in C) | required |
| `thread` | definition | `serial` or `worker`: where the site is reached. A filter needs `serial`; say `worker` when not sure | required |
| `args` | definition; a location may repeat them | `<register>:<type>[:<name>]` for each register the site's subscribers read: `x` registers hold `ptr`, `u64`, `i64`; `w` ones `u32`, `i32`, `u16`, `i16`, `u8`, `i8`, `bool`; `s` ones `f32`; `d` ones `f64`. A build whose code holds them in other registers gives them in its location, each the same type and name, in the same order | `[]` |
| `returns` | definition | `void`, or a type: what a filter at an entry may return in x0 or s0 | `void` |
| `module` | definition | `main`, the game's code (the SDK's functions are hooked by symbol: `builder/crates/edits/src/entry_hooks.rs`) | `main` |
| `require` | definition | `false`: a build's locations may leave the site out (it is then not placed in that build). Every other site must be located in every supported build | `true` |
| `function` | location | the function's name in this build, `f_` and its address | required |
| `signature` | location | the instruction words that find the function and the site (below) | one of these three |
| `in` | location | another site's name: this site is in the function that site is found in ([anchors](#anchors)) | |
| `fingerprint` | location | what the function does, to find it by ([fingerprints](#fingerprints)); beside a `signature`, a check that both find the same function | |
| `signature_offset` | location | bytes from the function's entry to the signature's first word, a multiple of 4 | 0 |
| `offset` | location | bytes from the function's entry to the instruction of the site, a multiple of 4; the forms inside a function only | `offset` or `point` |
| `point` | location | the instruction of the site by what is there, in place of `offset` ([anchors](#anchors)) | |
| `to` | location | `jump` only: bytes from the function's entry to the instruction the jump goes to, in the same function | |
| `guards` | location | code elsewhere in the function the callback relies on: `[{ offset = <bytes from the entry>, signature = "<words>" }, ...]`, each checked where the site's function is found; one that differs stops the build as a signature not found does | `[]` |

The reader joins each definition with its location and checks the whole before anything is placed. It stops the lift on
the first fault, naming the files, the site and the rule: a location of a site with no definition, a required site the
build does not locate, a definition's key in a location (or a location's in the definitions), and other arguments than
the definition's. Unknown keys are faults, so a misspelt key is not silently left out. (`sites::parse_table` reads a table
with both in one file, `format = 1`, as the checks write them.)

### Signatures

A signature is a few of the function's instruction words, as the CPU reads them (little-endian 32-bit), written as 8 hex
digits each:

- `79409289`: this exact word;
- `9100?3fd`: `?` masks a nibble;
- `34000009/ff00001f`: a value and a mask, for the fields that are not nibbles: the bits set in the mask must match. A
  branch's target (`cbz`'s imm19: `ff00001f`; `tbz`'s imm14: `fff8001f`; `b`/`bl`'s imm26: `fc000000`) and an `adrp`'s page
  move when the game is rebuilt, so they are masked; the opcode, registers and a load's offset into a structure are what
  identify the place, and stay.

At least one word must be whole (the search starts from it), at most 16. Use as few as find the site exactly once, and put
them where the hook depends on the code: the text speed signature covers the load of the letter count from the control and
the test of that count, which the callback relies on.

### Anchors

A signature is the simplest way to find a site, but its words must say both which function it is and which instruction.
When the instruction has nothing unique near it, for example a call the function makes twice, a signature borrows other
words of the function and needs guards. An anchor says instead what the instruction **is**, and it is looked for only in
the function's own instructions as the lift found them, so it needs to be unique only there. Mixin's `@At` and
ReVanced's instruction filters do the same for Java bytecode.

```toml
[[site]]
name = "jump.takeoffVelocity"
function = "f_804f5030"
in = "physics.driveTargetOther"   # (the function that site is found in)
point = { from = "physics.driveTargetOther", vcall = 0x190, ordinal = 1, of = 2 }
```

That is the set-velocity call through the world's vtable slot `0x190`, the second of the two after
`physics.driveTargetOther`'s point (`of = 2`: a third, or one gone, stops the lift). (The first of the two hands Havok
a zero velocity. The same call is written twice more in the function, before `physics.driveTarget`'s point and between
the two sites, at +0x920.)

`point` is one of:

| Key | The match |
|---|---|
| `words = "<words>"`, `at = <n>` | the words in a row, as a signature's ([above](#signatures)); the point is word `at` of them (0) |
| `call = "<name>"` | a `bl` whose callee, followed past PLT stubs as the lift calls it, is the function another site is found in (that site's name), the function `f_<address>`, or the exported symbol of that name (an SDK function, the same in every build) |
| `vcall = <slot>` | a `blr xN` whose register was loaded `ldr xN, [xM, #<slot>]` in the four instructions before it (its nearest write of xN): a virtual call, by its method's slot in the vtable |
| `string = "<text>"` | the `add` of an `adrp` + `add` pair (the `add` within the four instructions after the `adrp`) that makes the address of that string, NUL-terminated, in the module's image |

and may add:

| Key | What |
|---|---|
| `from`, `to` | the slice: only matches after `from` and before `to`. Each is another site's name (its point, which must be in the same function) or an anchor of its own (with no `from` or `to`), found in the whole function |
| `ordinal` | the match to take, from 0, in address order inside the slice. Without it, there must be exactly one |
| `of` | with `ordinal`: how many matches the slice must hold. Another count stops the lift (`3 matches in f_..., not the 2 it expects`), so a match added before the point, or one gone, cannot move it unnoticed |
| `shift` | whole instructions from the match to the point, after it (negative: before it): `shift = 1` is the instruction after a call |

An anchor found other than once stops the lift with the places it found (`found 4 times in f_... (+0x4, +0xc, ...)`), or
the matches it had without its slice, and `ordinal` past the end names how many there are. A site that waits on another
(`in`, a `call`, `from` or `to` naming a site) is found after it, and a cycle is a fault. A site that names one that does
not resolve is reported with it.

`in` with an `offset`, and no `point`, needs only the other site's signature: it is found with the code alone, as a
signature is. An anchor needs the lift's functions, so it is found after discovery (`sites::locate`), where the lift has
every function's instructions.

### Fingerprints

A function can also be found by what it does, which a game rebuilt with other register allocation keeps when its masked
words change. A `fingerprint` lists tests, and the one function of the lift that passes all of them is the site's:

```toml
fingerprint = { calls = ["save.request", "_ZN2nn2os11SleepThreadENS_8TimeSpanE"], strings = ["Example_00"], size = [200, 400] }
```

| Key | The function |
|---|---|
| `calls` | calls each of these: a site's function, a function `f_<address>`, or an exported symbol, as `point.call` |
| `strings` | makes the address of each of these strings, as `point.string` |
| `words` | holds these words in a row, as a signature's |
| `size` | has `[low, high]` instructions, as the lift found them |
| `blocks` | has `[low, high]` blocks (its entry and each branch target inside it) |

At least one of `calls`, `strings` or `words` is given: those are searched in the whole module, the one with the fewest
places first, and the functions those places are in are tested against the rest. More than 256 places for that one is a
fault (too general). More than one function passing stops the lift, naming them; none stops it with the nearest misses:
each function that failed, and the tests it failed. Beside a `signature`, a fingerprint is a check: both must find the
same function. No site of the supported build's needs one yet. A fingerprint is for a second build, where it
tells the functions apart that a signature would have to find again.

### What a table may hold

The project's rule is that no game file, and nothing made from one, leaves your PC. A table holds function names and offsets,
and a handful of instruction words per site, most of them with their operands masked, which say which instruction is
there (`ldrh w9, [x20, #0x48]`). These words are not enough to run, rebuild or recover any of the game; they are what a patch has to
name to say where it goes. Never copy more of a function into a table than the signature needs, nor data, text, assets or
any other bytes from your dump. A `string` anchor or fingerprint names a short string of the game's, such as a screen's
name: use one only where nothing else finds the site, and never more than the name.

### Forms

| `at` | Where the hook runs | As mid IR (`edits::site::SiteCode`) |
|---|---|---|
| `entry` | first, before anything of the function; with `can_return`, `if (on && fn(c)) return;` | `Entry` |
| `return` | after the function returns, by any way out (each `ret`, a tail call, an indirect jump): the function is lifted as `f_1<address>` under a wrapper `f_<address>` that calls it, then the hook | `Wrap::Return`, the wrapper |
| `after` | after the plain instruction at `offset` | `After` |
| `before` | before the instruction at `offset`, of any kind: on every way into it | `Before` |
| `replace` | in place of the plain instruction at `offset` while the site is on, the instruction itself while it is off: `if (on) fn(c); else { <the instruction> }`. A call (`bl`, `blr`) too: the hook sets x30 to the call's return address first, and the callback makes the call itself when it wants it (below) | `Replace` |
| `branch` | before the conditional branch at `offset`: a non-zero answer takes the branch, else its own test runs | `Branch` |
| `allow` | the conditional branch at `offset` taken only when its own test holds and the callback, asked only then, answers non-zero; while the site is off, the branch as the game has it: `if (<its test> && (!on \|\| fn(c))) <its goto>` | `Allow` |
| `jump` | before the instruction at `offset` (as `before`), a non-zero answer jumps to the instruction at `to`, which starts a block of its own | `Before`, with a goto to `to` |

A `return` site's wrapper calls the body as a plain call, so the hook can run after it: a guest tail call into the
function is a native call there, its frame kept until the body returns. A loop of tail calls through the function (a
state machine's `b` back into it) grows the host's stack by the wrapper's frame each time round, where the game's
does not; put such a site where no tail call cycles through it.

The callback is `int fn(Ctx *A64_NOESC c)` for every form; only `can_return`, `branch`, `allow` and `jump` read the
answer. A callback is a host call with the context, so at the hook the guest state is synced with the context: the
registers the callback reads are there, and what it writes is read back after it (`builder/crates/llvm/src/sync.rs`).
Away from its hooks, a function's registers stay in machine registers.

The usual shapes of a change map onto these: a call first in a function is `entry`, an early return first is `entry`
with `can_return`, code at each return is `return`, a branch's condition widened (`if (host || cond) goto`) is `branch`,
narrowed (`if (cond && host) goto`) is `allow`, `if (host) goto elsewhere;` is `jump`, a constant or an instruction's
result swapped is `replace` (or `after`, overwriting the result), code on the fall-through of a branch or after a call
is `before` the next instruction when nothing else jumps there, and a call wrapped by the host (`host(c, the callee)` in
its place) is `replace` on the call.

A replaced call: the callback is the call's wrapper ([several handlers](#several-handlers-on-one-site) wrap it in turn). It finds the callee as the guest would: for a `bl`,
`a64_lookup(hos_site_callee)`, the function the lift calls there (past any PLT stub), which the hook stores on the thread
before the callback (`game/sites.h`), so the callback names no address of this build's; for a `blr`,
`a64_icall(c, c->x[<n>])`. It calls it with the context; x30 is already the return address. It may also not call it (the call
held), or call it and then change what it returned. A `bl` to a function the lift does not call plainly (no entry, or a
setjmp owner, whose call takes a host setjmp) cannot be replaced: the lift stops.

A callback that names an address of the game's, a global's slot (the time object's, the cloth's clock) or a function, belongs to the one build whose table names its site: `game/oot_sites.c` and `game/oot_wide_sites.c` keep those in
one block each, named, for build e0f3ccfa. A second build's table brings its own values for them before it can reuse the callbacks. What a
callback reads elsewhere in the function (a register a loop's head loaded, a stack slot written at the entry) the site
checks with `guards`, so code that moved them does not resolve.

A jump's edge is an edge of the function's mid IR like any other, so the passes see it: its target may read flags set
before the site, and they are kept for it. Its target starts a block in every function the code is lifted in (each must
have it, or the lift stops). A `jump` is the only site before its
instruction; several `before` sites at one instruction run in the table's order, and at a branch a `before` site runs ahead of
its `branch` or `allow`. One `branch` or `allow` per branch. A `jump` back (to an offset at or before its own) is reported as a
loop; a `before` site after a call on an instruction other paths reach (a label) is reported with a warning.

### Several handlers on one site

A site has one slot, its own callback: the engine's (`HOS_SITE_ENGINE`), or one set at run time with `hos_site_set`. More
handlers chain around it with `hos_site_add(name, fn, priority)` (`game/sites.h`), as MixinExtras' wrap handlers do
where a replacement would shut every other out:

- they run lowest `priority` first, the earlier added first among equal priorities, and the site's own callback last,
  innermost;
- each runs the rest with `hos_site_next(c)`, which returns their answer, and returns the site's answer itself. A wrap
  of a call (a `replace` site on a `bl`, whose own callback makes the call) or of a return value does its work before
  and after `hos_site_next`, and a condition (`allow`) is `return ok(c) && hos_site_next(c);`. A handler that does not
  call `hos_site_next` stops the chain there;
- `hos_site_remove(handle)` takes one off (`hos_site_add` returns the handle, or 0 for no such site, no handler, a
  site that changes the outcome on a worker thread ([below](#who-may-change-the-outcome)), or a site that already
  holds `HOS_SITE_HANDLERS`, 8). With none left, the site's own callback is in its slot itself again.
  A hook already in the site's chain on another thread may still run the removed handler; `hos_site_remove_then(handle,
  done, user)` calls `done(user)` once no hook is in the chain (the site's reader count seen at 0 after the removal, as
  a rule's table is freed), at the acquire phase's reclaim: where what the handler used may go.

While no handler is added, nothing changes: the hook calls the site's own callback, and a dormant site costs its flag's
load and a predicted branch. With handlers, the hook calls the site's dispatcher (`game/sites.c` `run`), one more
call, and each `hos_site_next` another, and the dispatcher counts the hooks in it. A chain is published whole, so a hook
that loaded the old one runs the old one, and a replaced chain is kept, never freed. A hook reached inside a handler's
call runs its own site's chain, and the first chain goes on after it. The place in a chain is the guest context's, not
the OS thread's (a fiber resumed on another thread goes on with it), and the chain keeps the replace site's callee across
a handler's own calls into the game. Only the host changes: the lifted code is the same with or without handlers.
`tests/test_site_chain.py` checks all of it.

### Who may change the outcome

A site reached on a `worker` thread (the job threads: most of the game's code) runs while other threads run the game,
with no frame phase around it to hand anything over. There, a site that changes the game's outcome, where it goes
(`branch`, `allow`, `jump`, and `entry` with `can_return`) or what an instruction or a call does (`replace`: the callback
does the work in its place), is one of two things:

- **an engine feature's**: a `HOS_SITE_ENGINE` line in `game/sites.def`, its callback Longshot's own C, on from the
  start (Disable camera centering's `jump`, a moving floor's carry's `allow`, the zstd decodes' `can_return`, the heap
  retries' replaced calls); or
- **an inline data rule** (below): the lifter's own code over a host value, no callback at all.

Never a callback set at run time (a `HOS_SITE` line, off until `hos_site_set()`): nothing orders its answer with the
game's other threads. The reader checks it (`sites::check_site`, with the engine's site names from
`game/sites.def` when it reads a build's table): a site with one of those forms on a worker thread that is
neither stops the lift, naming the site. Each such site has a `HOS_SITE_OUTCOME(<ident>, "<name>")` line in
`game/sites.def` too (the lift stops when those lines are not exactly the table's such sites: `sites::apply`), and
the host refuses it a handler or a callback at run time (`hos_site_add`, `hos_site_set` answer 0). The other forms
(`entry` without `can_return`, `return`, `after`, `before`) leave the game's own instructions and branches in place
(their callbacks may still change the registers the game reads next), and are not checked here; on the `serial` thread
any form may be any site's.

### Inline data rules

A data rule on a hot path should cost a load and a branch, not a call. A site with `rule` is inlined: the lifter's own
mid IR for that template (`edits::site`), reading a host value, with no flag and no callback (no `HOS_SITE` line; a `HOS_VALUE` line of the
template's type instead). Off is a value of 0: one load and a predicted branch.

| `rule` | Its `args` | Value | What it does |
|---|---|---|---|
| `mul_f32` | `sN:f32:out`, `sM:f32:in` | `uint32_t`, a float's bits | `out = in * value` when the value is not 0 (after a divide: the divide still runs, its flags as before) |
| `scale_f32` | `sN:f32:out` | `uint32_t`, a float's bits | `out *= value` unless the value is 0 or 1.0 |
| `note_u64` | `xN:...:what` | `uint64_t` | `value = what`: the game tells the host something with a store, no call |

A table names a template and its registers, never code. Templates are added to `Rule` (`builder/crates/sites/src/table.rs`)
and their lowering to `builder/crates/edits/src/site.rs` with their first user.

How the host fills a rule, with no lock on the game's side (`game/sites.h`):

- a **value** is one word (`HOS_VALUE(<ident>, "<name>", <type>, <symbol>)`, the symbol the feature already owns): the host
  stores it with one atomic store (`hos_value_set`, or the feature's own store), the hook reads it with one load and sees a
  whole value, old or new;
- a **table** (`HOS_TABLE(<ident>, "<name>", <symbol>)`, for rules over many keys: rewards, damage by source) is built
  whole by the host, never written again, and published with one atomic exchange of the pointer
  (`hos_table_publish`); a hook that loaded the old pointer finishes with the old table, so the old one is retired and
  freed only once no hook can still hold it (below). No template reads a table yet: the first is added with its first
  user, and reads it through `hos_table_enter` / `hos_table_leave`.

#### When a replaced table is freed

There is no point in the game's frame where no game thread can be inside a hook, so a table is not freed at one. The
frame phase `GAME_PHASE_ACQUIRE` (`game/phases.h`) is the main thread's alone: the game's background threads (actor
create/delete, loading, placement) run through it, and that the job workers have joined there is not proven.
A hook on any of those threads may be between its load of the pointer and its last read of the table at that moment.

Each table counts its readers instead (`hos_table_readers_<ident>`, beside the table's symbol). A rule that reads a table
does, in its inlined C:

```c
const T *t = (const T *)hos_table_enter(&hos_table_readers_<ident>, &<symbol>);   /* count + 1, then the pointer */
/* ... read t; NULL: the rule is off ... */
hos_table_leave(&hos_table_readers_<ident>);                                     /* count - 1 */
```

`hos_table_publish` exchanges the pointer and retires the old table with its table's counter; `hos_tables_reclaim` frees
each retired table whose counter it sees at 0, and keeps the others for its next call. Why that is safe: the enter's add
and load, the publisher's exchange and the reclaim's load of the counter are all sequentially consistent, and the
publisher's exchange comes before the reclaim (the same thread, or ordered by the retirement's lock). When the reclaim
reads 0, either it came before a reader's add in that order, so the exchange did too and the reader's load gets the new
table, or it read the 0 that reader's leave (a release) wrote, after the reader's last read of the old one. A reader can
never be left holding a freed table, and no reader waits for anything.

When it runs: `hos_table_publish` calls it, and the first table ever retired adds it to the acquire phase at order 20 (the
engine's range, 0 to 99), so a retired table goes within a frame of its last reader leaving. With no table replaced,
nothing is added to the phase (parity gate 5). Past 256 tables waiting for their readers, a further old table stays
allocated: a leak, never a free too early.

The cost is on the readers: two atomic adds on a counter all of the table's readers share. A template that reads a table
on a path many threads run at once is measured before it lands, and moves to a per-thread scheme if the shared counter
shows.

### `vslot` sites (planned)

A virtual method's slot in a vtable, hooked at run time by a host thunk in the slot. Its address is data
per build, not a signature:

```toml
# game/sites/sites.toml
[[site]]
name = "actor.update"
kind = "vslot"
doc = "..."
shape = "event"         # (events and data rules only: the hook runs on the caller's thread)
thread = "worker"
args = ["x0:ptr:actor"]

# game/sites/<build id>/locations.toml
[[site]]
name = "actor.update"
vtable = 0x80100000     # (the vtable's guest address in this build: 8-aligned; made up here)
slot = 5                # (the method's index in it)
```

The reader checks these keys today; the lift stops on a `vslot` site with a note that it is not implemented yet.

## How a site is placed

At lift time, after the lift has found the game's functions:

1. **Read**: `sites::load` reads `game/sites/sites.toml` and `game/sites/<build id>/locations.toml` for main's build
   id, and joins them. No locations file, or no sites: nothing is placed, and the functions are lifted as they are.
2. **Resolve**: each signature is looked for in main's code as the lift translates it (after the ExeFS patch mods are
   written into it). It must be found **exactly once**; zero or several places stop the build, naming every such site and
   up to four places, and, for a signature found nowhere, its nearest misses: the places where all its words but one
   match, and which word differs. A site at an `offset` `in` another's function is placed with that one. A hook is never
   placed by guess.
3. **Locate**: with the lift's functions, each site found by an [anchor](#anchors) or a [fingerprint](#fingerprints),
   exactly once, or the build stops naming what it found.
4. **Check against the lift**: the function found must be one the lift found, and the one `function` names; an `after`
   point must be a plain instruction of it (a branch has no "after" inside the function: pick the instruction before it
   and test the branch's condition in the callback, as text speed does with the letter count). The host must list the
   site in `game/sites.def`, or the game would not link.
5. **Edit**: the placed site (`sites::Placed`) becomes its form's code as mid IR (`edits::site`), planned with the
   function's other edits (`edits::plan_edits`) and written in by the function builder at the instruction it belongs
   to, never by matching lifted text. A site whose instruction is lifted in several functions (code they share) is in
   each. A point inside a loop is reported in the lift's output: the hook runs each time round and needs a
   measurement.

The lift prints a line per site, its function, form and point, and how it was found (`[by signature]`, `[by signature;
point: the call to f_80733620, +1 instruction]`, `[by in physics.driveTargetOther's function; point: ...]`), then a
summary: `hooks: 132 site(s), each found exactly once and placed (130 by signature, 2 by an anchor in the function)`.
`gen/edits.txt` lists each function the edits change, its edits' key and what changes it.

The hook itself (text speed's site) does what this C would:

```c
if (hos_site_on_text_letterStep) hos_site_fn_text_letterStep(c);    /* the state synced with c around the call */
```

### Why a build stops here

| The lift says | Why | What to do |
|---|---|---|
| `<site>: not found` | this game's code differs at the site: another game version, or several ExeFS patch mods together (one alone is refused before the lift, below) | turn the code mods off; for a new game version, find the site again (below). The nearest misses say which word moved |
| `<site>: found N times` | the signature is not specific enough in this build | add a word, or unmask a field |
| `its point, ...: found N times in f_...` / `not found in f_...` / `no number N` / `not the N it expects` | the anchor matches other than once in its function, or other than `of` times | an `ordinal`, a slice (`from`, `to`), or more `words`; for `of`, check the function again |
| `the fingerprint matches N functions` / `no function passes the fingerprint; nearest: ...` | the fingerprint's tests do not tell one function apart | another test (a call, a string), or the failed test fixed |
| `it names <site>, which does not resolve` | the site waits on another that stops | that one first |
| `its signature finds f_..., the table names f_...` | the signature (or `in`, or the fingerprint) found a unique place, but in another function | the signature is wrong for this build |
| `not located in this build, and game/sites/sites.toml requires it` | the build's locations leave a required site out | locate it, or `require = false` in its definition |
| `... is a cbz, not a plain instruction` | the form needs another kind of instruction there | another form (`before`, `branch`), or the instruction before |
| `... no HOS_VALUE line of the rule's type` | the host has no value for the rule, or one of another width | add the line, of the template's type |
| `game/sites.def has no HOS_SITE line` | the host has no flag and slot for the site | add the line |
| `vslot sites ... not implemented yet` | | take the site out for now |

## Adding a site

1. Find the function and the point, and the instructions there: `gen/functions.txt` in your work folder names each
   function's unit, the unit's `.ll` holds `f_<entry>`, and `longshot-builder dev decode <hex word>` prints an instruction as
   the decoder reads it (the words from `image/image.bin`). All of it is made from your dump: it stays on your PC.
2. Write its definition in `game/sites/sites.toml` (where its feature's other sites are: that order is the order of hooks at
   one instruction) and its location in the build's `locations.toml`. Find the function with a short signature. Find
   the point with an `offset` when the signature covers it, or with an [anchor](#anchors) when it is a call, a virtual
   call or a string and the signature would have to borrow words from elsewhere in the function. A site in a function
   another site already finds says `in` that site instead of repeating its words.
3. Check it: `longshot-builder lift --work <work folder>` places every site as a build does and prints a line per site
   saying how it was found, or stops naming what did not resolve ([below](#why-a-build-stops-here)); with every other
   function's key unchanged it lifts only the units the new site touches. Without a lift, `longshot-builder dev sites --image
   <work folder>/image --tables game` places the sites over the image's own discovery, a line each, and exits 1
   naming any that does not resolve or fit, as the lift would stop.
4. Give the host its line in `game/sites.def`: `HOS_SITE_ENGINE(<ident>, "<name>", <callback>)` for an engine
   feature, on from the start; `HOS_SITE(<ident>, "<name>")` for a named hook nothing in the engine uses yet; and
   `HOS_SITE_OUTCOME(<ident>, "<name>")` as well for a site that changes the outcome on a worker thread. The callback
   is `int <callback>(Ctx *A64_NOESC c)` (`HosSiteFn`), reads and writes the registers `args` names in `c`, and returns
   the site's answer (0 where its form takes none).
5. After a lift, read the hooked function: `gen/edits.txt` names it and what changes it, and its unit's `.ll` holds
   it (a `return` site's wrapper `f_<entry>` and its body `f_1<entry>`). It is lifted code: it stays in your work
   folder, on your PC.
6. If the change ran after a branch, the site goes on the instruction before the branch and the callback tests the
   branch's condition.
7. The [parity gates](parity-gates.md) before the pull request: gate 2 (speed, the `measure` skill: the build against
   `main`, inside the 2% band) and gate 3 (behaviour: a scripted run through the control bridge on a fixed save, map,
   position and changed flags as on `main`).

## What a site costs

- **At play**: a dormant site (its flag 0) costs one byte load and a branch the CPU predicts. A subscribed one adds an
  indirect call with the context. An inline rule costs its value's load and a branch, and its arithmetic when on, with
  no call. `entry` and `return` leave the function's body as lifted (a `return` site adds one call, the wrapper's, which
  the compiler may inline); a site inside a function syncs the guest state with the context at its hook, as any call
  out of the function does.
- **At build time**: the resolver reads main's code once and searches it once per signature; an anchor is searched in
  its function alone; a fingerprint's calls, strings or words are one more pass over the code each. It runs right
  after discovery, which takes about two seconds on the game; the sites' own share is not measured apart.
- **What builds again**: the site tables of this build and the host's list (`game/sites.def`, whose `HOS_VALUE`
  symbols the lift writes and whose lines it checks) are in the lift's key, and each function's edits in its own key
  ([the builder](builder.md#the-lifts-cache)), so a table changed lifts and compiles only the units of the functions
  whose sites changed. A change to `game/`'s C alone only compiles the host again.

## ExeFS patch mods

A mod's ExeFS patch file ([mods](mods.md)) that writes the code a site is found by (its signature's words) or hooked at (the instruction of its
point; a function's first for `entry` and `return`), or that makes its signature match a second place, would stop the
build. It is refused instead, when the read stage collects the patch files (build-driver's `clashes::refuse`): its
kind is `refused`, its note names the site, `exefs_patches.txt` and the Mods tab say so ("cannot be applied", with the
note), and the other files apply. Each file is checked on its own against the code as the game ships it; files that
stop a site only together still stop the lift, whose message says to turn the code mods off. The check covers what the
code alone finds: each signature's words, guards and point, and a site at an offset `in` another's function.
A point by an anchor, or a function by a fingerprint alone, is found only in the lift, which checks them there.

## Not yet

- Templates that read a table: added with their first users.
- Code after a call as a form of its own (today `before` the instruction after the call, when nothing else jumps there).
- An operand swapped for one instruction as a form of its own (today a `replace` site whose callback does the
  instruction's work with the host's operand: `anim.loop*`, `steer.rate*`).
- `vslot` sites.
- A tool that proposes a new build's locations from the last build's: the fingerprints' tests are what it would match.

## What is done another way, and why

Everything below is a mid IR edit too, written by the same function builder over the same host ABI, but as Rust data
in `builder/crates/edits` rather than as a site of the table:

- **The output size's patches** (`edits::patches`, for build e0f3ccfa only): the window's and the frame's sizes asked
  of the host, the 3D's largest size scaled with the output, the render targets' pool and the "Graphics" heap grown with
  the output's pixels, a native output rebuild deferred at its function's entry. They write guest memory from the
  host, return early from a function and change constants at places the table has no form for yet. Each must be found
  exactly once in its function, or the lift stops.
- **The gameplay HUD's wrapper** of a menu screen's update (f_802f1380, [#1855](https://git.magicbean.dev/MagicBeanDev/Longshot/pulls/1855), `edits::patches`, a `Wrap`).
  `hos_gameplay_hud_screen` calls the body itself, between code before it and after it
  that share the body's state on the host's stack (the map's scope and placement before, put back after, and a hidden
  HUD's projection zeroed and restored), every way out of the body included. No site form runs code around a whole
  function; an `entry` and a `return` site would need a stack of their own per thread for what the wrapper keeps in
  its locals.
- **Boot and a thread's entry** (`host/hos_main.c` `call_guest_args`, `host/hos_thread.c`): they start the game's code on
  a fresh stack, before anything a `game_call` needs exists. Every other call the host makes into the game's functions
  goes through `game_call` (`host/game_call.h`), or `game_call_more` where it needs x8, its own LR or s0 and s1 back.
- **The SDK's functions** are hooked by symbol (`edits::entry_hooks`, from `host/`'s and `game/`'s `ENTRY_HOOK_SYM` lines), not by
  the table: they are found by name in every build.
