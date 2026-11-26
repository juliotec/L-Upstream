/* The game's flags (OoT's GameDataMgr): the one reader and writer of them in host code (game/flags.c).

   A flag is named by the MurmurHash3 (x86, 32 bits, seed 0) of its name ("PlayerStatus.Life" is 0x31ab5580) and kept in
   one typed container of GameDataMgr: its entry count at +8, its entries at +16, and a {u32 hash, u32 entry} table at
   +24 probed from hash % size (+36) and walked on, a hash of 0 ending it. Every read is bounded (an address outside the
   guest window, a table larger than 1 << 22 slots or an entry past the count reads as no flag), never a fault, so any
   thread may read; the values are as the game left them at that moment (the game's own writes are not locked against). */
#ifndef LONGSHOT_GAME_FLAGS_H
#define LONGSHOT_GAME_FLAGS_H
#include <stddef.h>
#include <stdint.h>

#define GAME_FLAGS_SLOT 0x84c9e180ULL   /* GameDataMgr's holder, as ReserveBancChange's readers load it */

typedef enum { GAME_FLAG_BOOL, GAME_FLAG_INT, GAME_FLAG_FLOAT, GAME_FLAG_ENUM, GAME_FLAG_TEXT, GAME_FLAG_UINT, GAME_FLAG_KINDS } GameFlagKind;
/* each container's place in GameDataMgr, the size of its entries and where an entry keeps its value (a String64's: a
   pointer to the text) */
typedef struct { uint32_t at, stride, value; } GameFlagLayout;
extern const GameFlagLayout game_flag_layout[GAME_FLAG_KINDS];

/* the hash a flag is named by, of the n bytes at name */
uint32_t game_flag_hash(const char *name, size_t n);
/* the game's GameDataMgr, 0 before it is made */
uint64_t game_flags_mgr(void);

/* the guest address of the entry of flag hash in container kind of mgr, 0 when it has none */
uint64_t game_flag_entry(uint64_t mgr, int kind, uint32_t hash);
/* a Bool, Int, Enum or UInt flag into *v (an Int signed, the others unsigned; a Bool's byte as it is): 0 when none */
int game_flag_number(uint64_t mgr, int kind, uint32_t hash, int64_t *v);
/* the same, but an entry index past the container's count read as entry 0, as the game's own getters read it (the Voice
   Language option, game/oot_voice.c, which main read that way); game_flag_number refuses such an index */
int game_flag_number_as_game(uint64_t mgr, int kind, uint32_t hash, int64_t *v);
/* a Bool flag into *value (its low bit): 0 when none */
int game_flag_bool(uint64_t mgr, uint32_t hash, int *value);
/* a Float flag into *f, 0 when none or not a finite number */
int game_flag_float(uint64_t mgr, uint32_t hash, float *f);
/* a String64 flag's text into buf (size bytes, at most 64 read, always ended): 0 when none or unreadable ("" in buf) */
int game_flag_text(uint64_t mgr, uint32_t hash, char *buf, size_t size);
/* a Bool flag's live byte written (the game's own setter also queues the save's copy: this does not): 0 when none */
int game_flag_set_bool(uint64_t mgr, uint32_t hash, int value);
/* a Bool64bitKey flag (named by an actor's 64-bit placement id, at GameDataMgr + 0xc58: the count at +0, the entries at +8,
   16 bytes each, the id at +0 and the value at +12): 1 set, 0 not, -1 not known. *at keeps where id was found, so a read
   after the first is one entry, not a search (-1: not found yet) */
int game_flag_bool64(uint64_t mgr, uint64_t id, int *at);

/* A container's header, read whole and checked: its table and entries mapped, size at most 1 << 22, count at most size.
   1 when it is; a scan reads it again after a pass to know the pass saw one table */
typedef struct { uint64_t entries, table; uint32_t count, size; } GameFlagTable;
int game_flag_table(uint64_t mgr, int kind, GameFlagTable *h);
/* one slot of a checked table: 1 a flag (*key its hash, *value its value: a Bool's byte or the 32 bits of the others),
   0 an empty slot (both 0), -1 unreadable or torn (the slot changed while it was read, or names an entry past the count) */
int game_flag_slot(const GameFlagTable *h, int kind, uint32_t slot, uint32_t *key, uint32_t *value);

#endif
