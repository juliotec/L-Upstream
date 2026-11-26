/* (longshot) The hook sites' host side (builder/crates/sites, docs/dev/reference/hooks.md).

   Callback sites: for each site of game/sites.def a flag byte and a callback slot, which the hook the lift emits at
   the site reads:

       if (hos_site_on_<ident>) hos_site_fn_<ident>(c);

   A dormant site (its flag 0) costs one byte load and a predicted branch. The callback gets the guest context at the site and
   may read and write the registers the site's table entry names (its args). It returns an int, which only some forms read:
   an entry site with can_return returns from the function when it is not 0, a branch site takes the branch. One slot per
   site: the engine's own features take theirs statically (HOS_SITE_ENGINE); more handlers chain around it (hos_site_add).

   Data rules: the lift inlines the rule's C at the site, reading a host value (HOS_VALUE: a word the C reads with one load)
   or, later, a table (HOS_TABLE: a pointer to a table nobody writes again). The host fills them without a lock on the game's
   side: a value with one atomic store (hos_value_set), a table by publishing a new one in place of the old with one atomic
   exchange of the pointer (hos_table_publish).

   A hook that loaded the old pointer finishes with the old table, so the old one is retired, not freed, and freed only once
   no hook can still hold it. There is no point in the game's frame where no game thread can be inside a hook: the frame
   phase GAME_PHASE_ACQUIRE (game/phases.h) is the main thread's alone, and the game's background threads (actor
   create/delete, loading, placement) run through it, the job workers perhaps too (not proven). So a table counts its readers
   instead: a rule that reads a table enters and leaves it (hos_table_enter / hos_table_leave below, inlined: two atomic
   adds on the table's own counter), and a retired table is freed once its counter has been seen at 0 after it was replaced
   (hos_tables_reclaim): every reader that could have loaded it has left by then, and every later one loads the new
   pointer. docs/dev/reference/hooks.md#inline-data-rules has the argument. */
#ifndef HOS_GAME_SITES_H
#define HOS_GAME_SITES_H

#include <stdint.h>
#include "a64.h"   /* (Ctx, A64_NOESC) */

typedef int (*HosSiteFn)(Ctx *A64_NOESC c);

/* A replace site on a bl: its hook stores the function the lift calls there (past any PLT stub) here, on the thread, before
   the callback, which calls the game's function with a64_lookup(hos_site_callee): by what this build's code calls, not by an
   address written into the host (builder/crates/edits, a replaced call). Read it before anything that could replace another call. */
extern _Thread_local uint64_t hos_site_callee;

#define HOS_SITE(id, name) extern volatile uint8_t hos_site_on_##id; extern HosSiteFn volatile hos_site_fn_##id;
#define HOS_SITE_ENGINE(id, name, fn) HOS_SITE(id, name) int fn(Ctx *A64_NOESC c);
#define HOS_VALUE(id, name, type, symbol) extern volatile type symbol;
#define HOS_TABLE(id, name, symbol) extern const void *volatile symbol; extern volatile uint32_t hos_table_readers_##id;
#define HOS_SITE_OUTCOME(id, name)
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME

/* A callback that does nothing and answers 0: an unsubscribed site's slot, so a hook that read the flag just before it was
   cleared calls this, never a null pointer */
int hos_site_none(Ctx *A64_NOESC c);

/* The site named name (its table name, "text.letterStep") calls fn from now on, its flag set; fn NULL turns the site off.
   Returns 0 when the host has no such site */
int hos_site_set(const char *name, HosSiteFn fn);

/* The callback of the site named name (hos_site_none when off), or NULL when the host has no such site */
HosSiteFn hos_site_get(const char *name);

/* Several handlers on one site, around its own callback (its HOS_SITE_ENGINE fn, or hos_site_set's), as MixinExtras' wrap
   handlers chain where a replacement would conflict (docs/dev/reference/hooks.md#several-handlers-on-one-site).

   hos_site_add puts fn on the site named name; the site's handlers run lowest priority first, the earlier added first among
   equal priorities, and the site's own callback last, innermost. Each handler runs the rest with hos_site_next(c), which
   returns their answer, and returns the site's answer itself: the rest's, or its own. A handler that does not call
   hos_site_next stops the chain there (the call a replace site wraps is then not made, unless it makes it itself).

       int my_wrap(Ctx *c) { before(c); int r = hos_site_next(c); after(c); return r; }   (a call, a return value)
       int my_cond(Ctx *c) { return ok(c) && hos_site_next(c); }                           (an allow site's condition)

   A site with no handler added calls its own callback directly, as before: the dormant cost is still the flag's load and a
   predicted branch, and a site's own callback costs nothing more. With handlers, the hook calls the site's dispatcher, one
   more call. Any thread, at any time: a chain is published whole (a hook that loaded the old one runs the old one), and a
   replaced chain is kept, never freed, a few dozen bytes per change.

   hos_site_add returns a handle (> 0) for hos_site_remove, or 0 when the host has no such site, fn is NULL, the site
   changes the game's outcome on a worker thread (a HOS_SITE_OUTCOME line: its engine feature's callback alone), or the
   site has HOS_SITE_HANDLERS handlers already. hos_site_remove returns 1 when it removed one; a hook on another thread already
   in the site's chain may still run the handler after that. hos_site_remove_then does the same and, once no hook is in
   the site's chain (its readers seen at 0 after the removal, as a rule's table is freed), calls done(user) at the next
   reclaim (hos_tables_reclaim, at the acquire phase): where what the handler uses may go. Past 256 waiting, done is not
   called (never early). A handler's own calls into the game reach other sites, whose hooks set hos_site_callee: the chain
   keeps the site's own and puts it back before each handler and the site's callback. */
#define HOS_SITE_HANDLERS 8
int hos_site_add(const char *name, HosSiteFn fn, int priority);
int hos_site_remove(int handle);
int hos_site_remove_then(int handle, void (*done)(void *user), void *user);
/* (in a handler added with hos_site_add: the rest of the site's chain, then its own callback; 0 past the end. With a
   context no handler runs for: 0. The chain's place is the guest context's, so a handler whose fiber resumed on another
   OS thread goes on with it there) */
int hos_site_next(Ctx *A64_NOESC c);

/* The sites the host knows, in game/sites.def's order: their count, and each one's name (NULL past the end) */
int hos_site_count(void);
const char *hos_site_name(int i);

/* A rule's value set (its low bytes, as wide as the value), one atomic store: 0 when the host has no such value */
int hos_value_set(const char *name, uint64_t v);

/* A rule's table: table (the caller's, never written again, from malloc) in place of the one before, which is retired and
   freed by hos_tables_reclaim once no reader can hold it; NULL turns the rule off. 0 when the host has no such table.
   Any thread. The first table retired adds hos_tables_reclaim to the frame phase GAME_PHASE_ACQUIRE (order 20, in the
   engine's 0..99), so a retired table goes within a frame of its last reader leaving; with no table ever replaced nothing
   is added (parity gate 5) */
int hos_table_publish(const char *name, const void *table);

/* The retired tables freed whose readers have been seen at 0 since they were replaced; the others are kept for the next
   call. Any thread, at any point: it waits for nothing and needs no quiescent point (hos_table_publish calls it, and so does
   the acquire phase once a table has been retired) */
void hos_tables_reclaim(void);

/* The read side of a table, for a rule's inlined C (ident as HOS_TABLE names it):

       const T *t = (const T *)hos_table_enter(&hos_table_readers_<ident>, &<symbol>);
       ... read t (NULL: the rule is off) ...
       hos_table_leave(&hos_table_readers_<ident>);

   The count first, then the pointer, both sequentially consistent, as the publisher's exchange and the reclaim's look at
   the count are: a reclaim that sees the count at 0 came either before this enter in that order (so the exchange did too,
   and this load sees the new table) or after this leave. Two atomic adds on a line the table's readers share: a template
   on a path many threads run at once is measured before it reads a table. */
static inline const void *hos_table_enter(volatile uint32_t *readers, const void *const volatile *symbol) {
    __atomic_add_fetch(readers, 1, __ATOMIC_SEQ_CST);
    return __atomic_load_n(symbol, __ATOMIC_SEQ_CST);
}
static inline void hos_table_leave(volatile uint32_t *readers) { __atomic_sub_fetch(readers, 1, __ATOMIC_RELEASE); }

#endif
