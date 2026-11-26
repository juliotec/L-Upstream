"""The new lifter's instruction semantics against Unicorn, instruction by instruction: random words over every integer,
memory, system, floating-point and Advanced SIMD encoding group, each lifted as a function of its own (`longshot-builder
dev words`), compiled with clang and run from random register, flag, SIMD and FP register, FPCR and memory states (floats
rich in NaNs, infinities, zeros and subnormals; FPCR with FZ and DN at random; a register offset's index with the bits
its extend drops or sign-extends at random); Unicorn (its "max" CPU: LSE, FlagM, CRC32, FP16, RDM, the dot product,
AES, SHA) runs the same word from the same state; every general register, SP, the flags, TPIDR_EL0, the 32 V registers,
FPSR.QC and the memory page must come out the same, and every 4 KB page of it the word changed must be marked written
for the GPU (the runtime's a64_dirty), unless it wrote through SP. Game-free: every word is generated.

Two more kinds of run use the same comparison. Fixed edge states (edge_cases), for what random states seldom reach:
SDIV and UDIV of the most negative number by -1 and by 0, CMP and CMN of the signed extremes and their neighbours,
CCMP and CCMN under every condition, TBL and TBX with indices at and past the table's end (and tables that wrap from
V31 to V0), EXT at every position; each of those words must lift. And sequences (--sequences): random runs of 2 to
SEQ_MAX of the covered words, lifted as one function (`dev words --seq`, `lift::straight`) and run by Unicorn from the
first to the last, so that one word's result is the next one's input and the IR passes see more than one instruction.
Each memory word's base register points at the page at the start; a run where Unicorn's accesses leave the page (an
earlier word changed the base) is not made in the lifted code, and at least half the runs must stay. Sequences run
with FPCR.FZ clear (see min_normal_kept below) and leave out LDSMAX and LDSMIN of less than 8 bytes (signed_minmax).

The float operations have a fast path (the host's IEEE operation) and a slow one (the exact semantics, out of line:
builder/crates/sem/src/fpfast.rs). The functions count how often each ran (`words --count-checks`), and the check fails
unless both did: the NaN-rich and subnormal-rich lanes, with FZ at random, send many runs down the slow path.

    python -I tests/test_lifter_semantics.py --cargo <cargo> --cxx <clang++> [--words N] [--trials N]
                                             [--sequences N] [--seed N]

Not compared, because there the lifter follows the host model rather than the architecture (or Unicorn models what the
Switch does not): pointer authentication and its hints (the identity here: PAC*, AUT*, XPAC*, LDRAA, LDRAB, HINT), the
exclusive monitor (the host runtime's: LDXR, LDAXR, STXR, STLXR, LDXP, LDAXP, STXP, STLXP, CLREX), MRS and MSR of every
system register but NZCV and TPIDR_EL0 (FPCR and FPSR, the counters, the cache and ZVA identification), literal loads
(they read near the code address, which only Unicorn maps), and SYS of XZR (address 0, which neither maps). A word the
lifter does not lift (undefined here, or not covered: it traps there) is not listed by `words` and so not run. The
page marks are checked to cover the pages written; a mark of a page left unchanged (a compare and swap that failed, a
store of the bytes already there) is not an error.

Floating point follows the host model of builder/crates/sem/src/fp.rs, which differs from the architecture on purpose,
so these are not compared: FPCR.RMode other than round to nearest (FPCR is drawn with RMode 0; FRINTX and FRINTI round
to nearest), FPSR's cumulative exception flags (IOC, DZC, OFC, UFC, IXC, IDC: not raised; QC is compared), FPCR.AHP and
FZ16 (drawn clear), and the FP16 forms the lifter does not cover (fused, estimates, fixed point: it traps there, and
`words` leaves them out); FEAT_FP16's other arithmetic is compared. One difference can show and is not drawn on
purpose: under FZ, a result tiny before rounding that rounds to the smallest normal stays that normal (the architecture
flushes it to zero); the check lets that lane differ (min_normal_kept), and nothing else.

Unicorn 2.1 compares the byte, halfword and word forms of LDSMAX and LDSMIN unsigned (its QEMU base gives the
operation no sign: LDSMAXH of 0xffff and 1 stores 0xffff). For those the memory they leave is the Arm ARM's, worked
out here (signed_minmax), not Unicorn's.
"""
import argparse
import collections
import ctypes
import os
import random
import struct
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cargo", "{cargo}", "--cxx", "{cxx}"]
RUN_ALL_MODULES = ["unicorn"]
RUN_ALL_SKIP = {"windows": "maps a page at a fixed host address (POSIX mmap): checked on Linux"}

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CODE = 0x80000000          # (the words' addresses: f_<CODE + 4 i>)
SEQ_CODE = 0x90000000      # (the sequences' addresses: f_<SEQ_CODE + SEQ_STRIDE k>)
SEQ_STRIDE = 0x40
SEQ_MAX = 8                # (words in a sequence: 2 to this)
PAGE = 0x20000000          # (the memory the accesses use, mapped in both)
PAGE_SIZE = 0x40000
BASE_AT = PAGE + 0x10000   # (where a base register points: offsets reach -0x1000 .. +0x10000 from it)
CTX_SIZE = 864
NZCV_AT = 264
VREG_AT = 272
TPIDR_AT = 784
FPCR_AT = 800
FPSR_AT = 808
FPSR_QC = 1 << 27
FPCR_FZ, FPCR_DN = 1 << 24, 1 << 25
DIRTY_FIRST, DIRTY_PAGES = PAGE >> 12, PAGE_SIZE >> 12   # (the window's entries in the runtime's a64_dirty)

# (mask, value) of the encoding groups the words are drawn from: as tests of the decoder (builder/crates/a64/tests)
GROUPS = [
    (0x1E000000, 0x10000000),   # data processing, immediate
    (0x0E000000, 0x0A000000),   # data processing, register
    (0x0A000000 | 1 << 26, 0x08000000),   # loads and stores of general registers
    (0x3F000000, 0x08000000),   # exclusive, ordered, compare and swap
    (0x3F200C00 | 1 << 26, 0x38200000),   # atomics
    (0x3F200C00 | 1 << 26, 0x38200800),   # register offset
    (0x3B000000 | 1 << 26, 0x39000000),   # unsigned offset
    (0x3A000000 | 1 << 26, 0x28000000),   # pairs
    (0x7FE00000, 0x5AC00000),   # data processing, 1 source
    (0x5FE00000, 0x1AC00000),   # data processing, 2 sources
    (0x1FE00000, 0x1A000000),   # carry, RMIF, SETF
    (0x1FE00000, 0x1A400000),   # conditional compare
    (0x1FE00000, 0x1A800000),   # conditional select
    (0x1F000000, 0x1B000000),   # 3 sources
    (0xFFFFF01F, 0xD503301F),   # barriers, CLREX
    (0xFFDFFFE0, 0xD51B4200),   # MRS and MSR of NZCV (Rt at random)
    (0xFFDFFFE0, 0xD51BD040),   # MRS and MSR of TPIDR_EL0
    (0xFFFFFFE0, 0xD50B7420),   # DC ZVA
    (0x1E000000, 0x0E000000),   # floating point and Advanced SIMD (x111)
    (0x5F200000, 0x1E200000),   # scalar FP
    (0x5F000000, 0x1F000000),   # FP 3 sources
    (0x5F200000, 0x1E000000),   # FP and fixed-point conversions
    (0x9F200400, 0x0E200400),   # vector three same
    (0x9F200C00, 0x0E200000),   # vector three different
    (0x9F3E0C00, 0x0E200800),   # vector two-register miscellaneous
    (0x9F3E0C00, 0x0E300800),   # across lanes
    (0x9F200400, 0x0E000400),   # copy, three same extra
    (0xBF208000, 0x0E000000),   # table, permute, extract
    (0x9F800400, 0x0F000400),   # shift by immediate, modified immediate
    (0x9F000400, 0x0F000000),   # by element
    (0xDF000000, 0x5E000000),   # scalar Advanced SIMD, crypto
    (0xDF000400, 0x5F000000),   # scalar shift, by element
    (0xBF800000, 0x0C000000),   # structure loads and stores, multiple
    (0xBF800000, 0x0C800000),   # the same, post-index
    (0xBF800000, 0x0D000000),   # structure loads and stores, single
    (0xBF800000, 0x0D800000),   # the same, post-index
    (0x3F000000, 0x3D000000),   # loads and stores of SIMD and FP registers, unsigned offset
    (0x3F000000, 0x3C000000),   # the same, other addressing
    (0x3E000000, 0x2C000000),   # pairs of SIMD and FP registers
]
# (forms whose behaviour is the host model's, not the architecture's: see the docstring)
LEFT_OUT = ("pac", "aut", "xpac", "ldraa", "ldrab", "hint", "ldxr", "ldaxr", "stxr", "stlxr", "ldxp", "ldaxp", "stxp",
            "stlxp", "clrex")
# (the system registers compared: the rest are the host model's)
SYSREGS_COMPARED = ("NZCV", "TPIDR_EL0")


def left_out(mnemonic, text):
    """Whether the form's behaviour is the host model's (the docstring's list), so that it is not compared"""
    if mnemonic in ("mrs", "msr"):
        return not any(r in text.replace(",", " ").split() for r in SYSREGS_COMPARED)
    if mnemonic == "sys" and text.endswith("xzr"):
        return True   # (a cache operation on XZR: address 0, which neither side maps)
    return mnemonic.startswith(LEFT_OUT)


def index_value(rng, text):
    """A register offset's index: small, so that the access stays in the page, with what the extend drops or
    sign-extends set at random (the upper 32 bits of a W index; a negative index of either width)"""
    small = rng.randrange(0, 0x40)
    negative = rng.random() < 0.5
    if "uxtw" in text:
        return rng.getrandbits(32) << 32 | small
    if "sxtw" in text:
        low = -small & 0xFFFFFFFF if negative else small
        return rng.getrandbits(32) << 32 | low
    return -small & (1 << 64) - 1 if negative else small
SPECIAL = [0, 1, 0xFFFFFFFFFFFFFFFF, 0x8000000000000000, 0x7FFFFFFFFFFFFFFF, 0x80000000, 0x7FFFFFFF, 0xFFFFFFFF]


def min_normal_kept(ours, theirs):
    """Whether two V registers differ only where ours holds the smallest normal and the architecture's a zero of the
    same sign, in lanes of single or double precision: the docstring's one difference under FZ (a result tiny before
    rounding that rounds to the smallest normal)"""
    for bits, min_normal in ((64, 1 << 52), (32, 1 << 23)):
        mask, sign = (1 << bits) - 1, 1 << (bits - 1)
        ok = True
        for i in range(128 // bits):
            a, b = ours >> (bits * i) & mask, theirs >> (bits * i) & mask
            if a != b and not (a & ~sign == min_normal and b == a & sign):
                ok = False
                break
        if ok:
            return True
    return False


def fp_lane(rng, bits):
    """a float of `bits` bits worth testing: the special values, subnormals, the edges, small numbers, anything"""
    frac = {16: 10, 32: 23, 64: 52}[bits]
    exp_bits = bits - 1 - frac
    sign = rng.getrandbits(1) << (bits - 1)
    exp_all = ((1 << exp_bits) - 1) << frac
    bias = (1 << (exp_bits - 1)) - 1
    r = rng.random()
    if r < 0.08:
        return sign                                           # zero
    if r < 0.14:
        return sign | exp_all                                 # infinity
    if r < 0.20:
        return sign | exp_all | 1 << (frac - 1) | rng.getrandbits(frac - 1)      # quiet NaN
    if r < 0.25:
        return sign | exp_all | (rng.getrandbits(frac - 1) or 1)                  # signalling NaN
    if r < 0.33:
        return sign | rng.getrandbits(frac)                   # subnormal (or zero)
    if r < 0.37:
        return sign | rng.choice([1 << frac, exp_all - (1 << frac) | ((1 << frac) - 1), ((1 << frac) - 1)])
    if r < 0.55:
        e = bias + rng.randrange(-4, 34)                      # small numbers, integers among them
        return sign | e << frac | (rng.getrandbits(frac) & ~((1 << max(0, frac - rng.randrange(0, 12))) - 1))
    if r < 0.75:
        return sign | rng.randrange(1, (1 << exp_bits) - 1) << frac | rng.getrandbits(frac)
    return rng.getrandbits(bits)


def vreg_value(rng):
    """a SIMD and FP register's 128 bits: lanes of one float format, or integers"""
    r = rng.random()
    if r < 0.25:
        return rng.getrandbits(128)
    if r < 0.35:
        return sum(reg_value(rng) % (1 << 64) << 64 * k for k in range(2))
    bits = rng.choice([32, 32, 64, 16])
    return sum(fp_lane(rng, bits) << (bits * k) for k in range(128 // bits))


def fpcr_value(rng):
    """FPCR: round to nearest (the lifter's model), FZ and DN at random"""
    return rng.choice([0, 0, FPCR_FZ, FPCR_DN, FPCR_FZ | FPCR_DN])


def words(rng, per_group):
    out = []
    for mask, value in GROUPS:
        for _ in range(per_group):
            out.append((rng.getrandbits(32) & ~mask & 0xFFFFFFFF) | value)
    return out


def reg_value(rng):
    r = rng.random()
    if r < 0.15:
        return rng.choice(SPECIAL)
    if r < 0.4:
        return rng.getrandbits(8)
    if r < 0.55:
        return rng.getrandbits(32)
    return rng.getrandbits(64)


def changed_pages(before, after):
    """The 4 KB pages (their indices) where two images of the window differ"""
    if before == after:
        return set()
    return {k for k in range(len(before) >> 12) if before[k << 12:(k + 1) << 12] != after[k << 12:(k + 1) << 12]}


def signed_minmax(mnemonic, word, size, x, sp, rn, page):
    """The page as the Arm ARM leaves it after LDSMAX* / LDSMIN* of `size` bytes (below 8): where Unicorn is wrong.
    None for any other instruction"""
    if not mnemonic.startswith(("ldsmax", "ldsmin")) or size >= 8:
        return None
    at = (sp if rn == 31 else x[rn]) - PAGE
    bits = 8 * size

    def signed(v):
        v &= (1 << bits) - 1
        return v - (1 << bits) if v >> (bits - 1) else v
    rs = (word >> 16) & 31
    old = signed(int.from_bytes(page[at:at + size], "little"))
    operand = signed(0 if rs == 31 else x[rs])
    new = max(old, operand) if mnemonic.startswith("ldsmax") else min(old, operand)
    return page[:at] + (new & ((1 << bits) - 1)).to_bytes(size, "little") + page[at + size:]


MASK64 = (1 << 64) - 1
MIN64, MAX64 = 1 << 63, (1 << 63) - 1
EDGES = [0, 1, MASK64, MIN64, MAX64, MIN64 + 1, MAX64 - 1]   # (-1, the signed extremes and their neighbours)


def edge_cases(rng):
    """The fixed states: (word, what the state must hold) for the edges random states seldom reach. SDIV and UDIV of
    the most negative number by -1, by 0 and by 1; CMP and CMN (SUBS and ADDS, to XZR and to a register) of every pair
    of 0, 1, -1, the signed extremes and their neighbours, where the signed overflow sets V; CCMP and CCMN, register and
    immediate, under every condition from flags that pass and fail it; TBL and TBX of one to four registers, with
    indices just inside and just past the table (16 n - 1, 16 n), 255, and tables that wrap from V31 to V0 or hold the
    index or the destination register; EXT at every position. A W form's operands carry garbage in their upper 32 bits,
    which it must ignore. What is not given is drawn at random as for any word."""
    cases = []

    def width(value, sf):
        return value if sf else rng.getrandbits(32) << 32 | value & 0xFFFFFFFF

    for sf in (0, 1):
        top = 1 << (63 if sf else 31)
        pairs = [(top, MASK64), (top, 1), (top, 0), (1, 0), (MASK64, MASK64), (top, top), (top - 1, MASK64),
                 (7, -2 & MASK64), (-7 & MASK64, 2)]
        for base in (0x1AC00C00, 0x1AC00800):   # SDIV, UDIV
            for n, m in pairs:
                cases.append((base | sf << 31 | 2 << 16 | 1 << 5 | 0, {"x": {1: width(n, sf), 2: width(m, sf)}}))
        edges = EDGES if sf else [0, 1, 0xFFFFFFFF, 0x80000000, 0x7FFFFFFF, 0x80000001, 0x7FFFFFFE]
        for base in (0x6B000000, 0x2B000000):   # SUBS (CMP), ADDS (CMN)
            for rd in (31, 0):
                for n in edges:
                    for m in edges:
                        cases.append((base | sf << 31 | 2 << 16 | 1 << 5 | rd, {"x": {1: width(n, sf), 2: width(m, sf)}}))
        for base in (0x7A400000, 0x3A400000):   # CCMP, CCMN (register)
            for cond in range(16):
                for flags in (0, 0xF, 0b0110, 0b1001):
                    for n, m in ((top, 1), (top - 1, MASK64), (0, top), (top, top), (1, top - 1)):
                        nzcv = rng.getrandbits(4)
                        cases.append((base | sf << 31 | 2 << 16 | cond << 12 | 1 << 5 | nzcv,
                                      {"x": {1: width(n, sf), 2: width(m, sf)}, "nzcv": flags}))
                    for n, imm in ((top, 1), (top - 1, 31), (0, 0), (top + 1 & MASK64, 1)):
                        nzcv = rng.getrandbits(4)
                        cases.append((base | sf << 31 | imm << 16 | cond << 12 | 1 << 11 | 1 << 5 | nzcv,
                                      {"x": {1: width(n, sf)}, "nzcv": flags}))
    indices = [0, 1, 8, 15, 16, 17, 31, 32, 33, 47, 48, 49, 63, 64, 128, 255]
    for q in (0, 1):
        for length in range(4):
            for op in (0, 1):   # TBL, TBX
                for rn, rm, rd in ((1, 9, 0), (1, 2, 0), (30, 9, 0), (31, 9, 0), (31, 9, 1)):
                    for _ in range(2):
                        order = indices[:]
                        rng.shuffle(order)
                        vm = sum(b << 8 * k for k, b in enumerate(order))
                        cases.append((0x0E000000 | q << 30 | rm << 16 | length << 13 | op << 12 | rn << 5 | rd,
                                      {"v": {rm: vm}}))
        for imm in range(16 if q else 8):
            for rn, rm, rd in ((1, 2, 0), (1, 1, 0), (1, 2, 1), (1, 2, 2)):
                cases.append((0x2E000000 | q << 30 | rm << 16 | imm << 11 | rn << 5 | rd, {}))
    return cases


def small_signed_minmax(mnemonic, size):
    """LDSMAX* / LDSMIN* of a byte, halfword or word: Unicorn's are unsigned (see the docstring)"""
    return mnemonic.startswith(("ldsmax", "ldsmin")) and (size or 0) < 8


def sequences(rng, rows, count):
    """`count` runs of 2 to SEQ_MAX words drawn from the lifted ones (no literal loads, nothing left out, no LDSMAX or
    LDSMIN that Unicorn gets wrong), as lists of rows"""
    pool = [r for r in rows if r[2] != "lit" and not left_out(r[6].split(" ")[0], r[6])
            and not small_signed_minmax(r[6].split(" ")[0], r[5])]
    return [[rng.choice(pool) for _ in range(rng.randint(2, SEQ_MAX))] for _ in range(count)]


def build(a, tmp, word_list, seq_words):
    lifter = os.path.join(ROOT, "builder")
    subprocess.run([a.cargo, "build", "--locked", "--release", "-q", "-p", "longshot-builder"], cwd=lifter, check=True)
    exe = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "release", "longshot-builder")
    words_in, seq_in, ll, listing = (os.path.join(tmp, n) for n in ("words.txt", "seq.txt", "words.ll", "list.txt"))
    with open(words_in, "w") as f:
        f.write("".join("%08x\n" % w for w in word_list))
    with open(seq_in, "w") as f:
        f.write("".join("%x %s\n" % (SEQ_CODE + SEQ_STRIDE * k, " ".join("%08x" % w for w in ws))
                        for k, ws in enumerate(seq_words)))
    subprocess.run([exe, "dev", "words", "--in", words_in, "--pc", "%x" % CODE, "--out", ll, "--list", listing,
                    "--seq", seq_in, "--count-checks"], check=True)
    so = os.path.join(tmp, "words.so")
    rt = os.path.join(ROOT, "tests", "lifter_semantics_rt.c")
    subprocess.run([a.cxx, "-shared", "-fPIC", "-O1", "-mcx16", "-Wno-override-module", "-x", "ir", ll, "-x", "c", rt,
                    "-o", so], check=True)
    return ctypes.CDLL(so)


def lift_listing(a, tmp, word_list):
    """The listing of `dev words` for `word_list` alone: which words lift, and their operands"""
    exe = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "release", "longshot-builder")
    subprocess.run([a.cargo, "build", "--locked", "--release", "-q", "-p", "longshot-builder"],
                   cwd=os.path.join(ROOT, "builder"), check=True)
    words_in, ll, listing = (os.path.join(tmp, n) for n in ("words.txt", "listing.ll", "list.txt"))
    with open(words_in, "w") as f:
        f.write("".join("%08x\n" % w for w in word_list))
    subprocess.run([exe, "dev", "words", "--in", words_in, "--pc", "%x" % CODE, "--out", ll, "--list", listing],
                   check=True, stdout=subprocess.DEVNULL)
    rows = []
    with open(listing) as f:
        for line in f:
            i, w, kind, rn, rm, size, text = line.rstrip("\n").split(" ", 6)
            rows.append((int(i), int(w, 16), kind, None if rn == "-" else int(rn), None if rm == "-" else int(rm),
                         None if size == "-" else int(size), text))
    return rows


def random_state(rng, fz=True):
    """A random starting state: general registers, SP (in the page), flags, TPIDR_EL0, V registers, FPCR, the page"""
    return {"x": [reg_value(rng) for _ in range(31)], "sp": BASE_AT + 16 * rng.randrange(0, 0x100),
            "nzcv": rng.getrandbits(4), "tpidr": rng.getrandbits(64), "v": [vreg_value(rng) for _ in range(32)],
            "fpcr": fpcr_value(rng) if fz else rng.choice([0, 0, FPCR_DN]),
            "page": bytes(rng.getrandbits(8) for _ in range(256)) * (PAGE_SIZE // 256)}


def place(rng, st, row):
    """Points a memory word's base register at the page and its index register at a small offset"""
    _, _, kind, rn, rm, _, text = row
    if kind != "mem":
        return
    base = BASE_AT + 16 * rng.randrange(0, 0x100)
    if rn == 31:
        st["sp"] = base
    else:
        st["x"][rn] = base
    if rm is not None and rm != 31:
        st["x"][rm] = index_value(rng, text)   # (rm: a register offset's index)


class Checker:
    """Runs a function of the lifted module and Unicorn from one state, and compares what they leave"""

    def __init__(self, lib, uc, A):
        self.lib, self.uc, self.A = lib, uc, A
        self.xregs = [getattr(A, "UC_ARM64_REG_X%d" % n) for n in range(31)]
        self.qregs = [getattr(A, "UC_ARM64_REG_Q%d" % n) for n in range(32)]
        self.checks = ctypes.c_uint64.in_dll(lib, "lifter_fast_checks")
        self.slow_checks = ctypes.c_uint64.in_dll(lib, "lifter_slow_checks")
        self.dirty = (ctypes.c_uint8 * (1 << 27)).in_dll(lib, "a64_dirty")
        self.outside = False   # (set by the watch: an access left the page)
        self.watch = None

    def watch_page(self):
        """From now on, note any access Unicorn makes outside the page (a sequence's base register an earlier word
        changed): such a run is not made in the lifted code, which has nothing mapped there"""
        from unicorn import UC_HOOK_MEM_READ, UC_HOOK_MEM_WRITE

        def hook(uc, access, address, size, value, data):
            if address < PAGE or address + size > PAGE + PAGE_SIZE:
                self.outside = True
        self.watch = self.uc.hook_add(UC_HOOK_MEM_READ | UC_HOOK_MEM_WRITE, hook)

    def theirs(self, st, start, count):
        """Unicorn's result from `st`, or None if it faulted (or, watching, went outside the page)"""
        from unicorn import UcError
        uc, A = self.uc, self.A
        for n in range(31):
            uc.reg_write(self.xregs[n], st["x"][n])
        uc.reg_write(A.UC_ARM64_REG_SP, st["sp"])
        uc.reg_write(A.UC_ARM64_REG_NZCV, st["nzcv"] << 28)
        uc.reg_write(A.UC_ARM64_REG_TPIDR_EL0, st["tpidr"])
        for n in range(32):
            uc.reg_write(self.qregs[n], st["v"][n])
        uc.reg_write(A.UC_ARM64_REG_FPCR, st["fpcr"])
        uc.reg_write(A.UC_ARM64_REG_FPSR, 0)
        uc.mem_write(PAGE, st["page"])
        self.outside = False
        try:
            uc.emu_start(start, start + 4 * count, count=count)
        except UcError:
            return None
        if self.outside:
            return None
        return {"x": [uc.reg_read(r) for r in self.xregs], "sp": uc.reg_read(A.UC_ARM64_REG_SP),
                "nzcv": uc.reg_read(A.UC_ARM64_REG_NZCV) >> 28 & 15, "page": bytes(uc.mem_read(PAGE, PAGE_SIZE)),
                "v": [uc.reg_read(r) for r in self.qregs], "qc": uc.reg_read(A.UC_ARM64_REG_FPSR) & FPSR_QC,
                "tpidr": uc.reg_read(A.UC_ARM64_REG_TPIDR_EL0)}

    def ours(self, st, start):
        """The lifted function's result from `st`, with the float paths it ran and the pages it marked"""
        lib = self.lib
        fn = getattr(lib, "f_%08x" % start)
        fn.argtypes = [ctypes.c_void_p]
        ctx = (ctypes.c_uint8 * CTX_SIZE)()
        nzcv = st["nzcv"]
        struct.pack_into("<31Q", ctx, 0, *st["x"])
        struct.pack_into("<Q", ctx, 31 * 8, st["sp"])
        struct.pack_into("<4B", ctx, NZCV_AT, nzcv >> 3 & 1, nzcv >> 2 & 1, nzcv >> 1 & 1, nzcv & 1)
        struct.pack_into("<Q", ctx, TPIDR_AT, st["tpidr"])
        for n in range(32):
            struct.pack_into("<QQ", ctx, VREG_AT + 16 * n, st["v"][n] & MASK64, st["v"][n] >> 64)
        struct.pack_into("<QQ", ctx, FPCR_AT, st["fpcr"], 0)
        lib.window_write(ctypes.c_uint64(PAGE), st["page"], ctypes.c_uint64(PAGE_SIZE))
        ctypes.c_int.in_dll(lib, "trapped").value = 0
        ctypes.memset(ctypes.addressof(self.dirty) + DIRTY_FIRST, 0, DIRTY_PAGES)
        self.checks.value = self.slow_checks.value = 0
        fn(ctypes.addressof(ctx))
        f = struct.unpack_from("<4B", ctx, NZCV_AT)
        page = ctypes.create_string_buffer(PAGE_SIZE)
        lib.window_read(ctypes.c_uint64(PAGE), page, ctypes.c_uint64(PAGE_SIZE))
        return {"x": list(struct.unpack_from("<31Q", ctx, 0)), "sp": struct.unpack_from("<Q", ctx, 31 * 8)[0],
                "nzcv": (f[0] & 1) << 3 | (f[1] & 1) << 2 | (f[2] & 1) << 1 | (f[3] & 1),
                "v": [lo | hi << 64 for lo, hi in (struct.unpack_from("<QQ", ctx, VREG_AT + 16 * n) for n in range(32))],
                "qc": struct.unpack_from("<Q", ctx, FPSR_AT)[0] & FPSR_QC,
                "tpidr": struct.unpack_from("<Q", ctx, TPIDR_AT)[0], "page": page.raw,
                "marked": bytes(self.dirty[DIRTY_FIRST:DIRTY_FIRST + DIRTY_PAGES]),
                "runs": self.checks.value, "slow": self.slow_checks.value}


def differences(st, ours, theirs, check_marks):
    """What differs between the two results, as text (empty: none); `check_marks`: also that every page the run
    changed is marked written"""
    diffs = ["x%d %#x, Unicorn %#x" % (n, ours["x"][n], theirs["x"][n]) for n in range(31)
             if ours["x"][n] != theirs["x"][n]]
    if ours["sp"] != theirs["sp"]:
        diffs.append("sp %#x, Unicorn %#x" % (ours["sp"], theirs["sp"]))
    diffs += ["v%d %#034x, Unicorn %#034x" % (n, ours["v"][n], theirs["v"][n]) for n in range(32)
              if ours["v"][n] != theirs["v"][n]
              and not (st["fpcr"] & FPCR_FZ and min_normal_kept(ours["v"][n], theirs["v"][n]))]
    if ours["qc"] != theirs["qc"]:
        diffs.append("FPSR.QC %d, Unicorn %d" % (ours["qc"] != 0, theirs["qc"] != 0))
    if ours["tpidr"] != theirs["tpidr"]:
        diffs.append("tpidr_el0 %#x, Unicorn %#x" % (ours["tpidr"], theirs["tpidr"]))
    if ours["nzcv"] != theirs["nzcv"]:
        diffs.append("nzcv %s, Unicorn %s" % (format(ours["nzcv"], "04b"), format(theirs["nzcv"], "04b")))
    if ours["page"] != theirs["page"]:
        at = next(k for k in range(PAGE_SIZE) if ours["page"][k] != theirs["page"][k])
        diffs.append("memory differs from %#x" % (PAGE + at))
    elif check_marks:
        # (a store marks the pages it writes, for the GPU: unless through SP, whose stack the GPU never reads)
        unmarked = sorted(changed_pages(st["page"], ours["page"]) - {k for k in range(DIRTY_PAGES) if ours["marked"][k]})
        if unmarked:
            diffs.append("page %#x written, not marked" % (PAGE + (unmarked[0] << 12)))
    return diffs


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--cargo", required=True)
    ap.add_argument("--cxx", required=True)
    ap.add_argument("--words", type=int, default=3000, help="random words per encoding group (3000)")
    ap.add_argument("--trials", type=int, default=3, help="random states per word (3)")
    ap.add_argument("--sequences", type=int, default=4000, help="random sequences of words (4000)")
    ap.add_argument("--seed", type=int, default=20261122)
    a = ap.parse_args()
    if "clang" not in os.path.basename(a.cxx):
        print("test_lifter_semantics: skipped, %s is no clang (the lifted code is LLVM IR)" % a.cxx)
        return 0
    from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM
    from unicorn import arm64_const as A

    rng = random.Random(a.seed)
    tmp = tempfile.mkdtemp(prefix="longshot-lifter-sem-")
    edges = edge_cases(rng)
    edge_words = sorted({w for w, _ in edges})
    word_list = words(rng, a.words) + edge_words
    rows = lift_listing(a, tmp, word_list)
    edge_at = {w: i for i, w, *_ in rows if i >= len(word_list) - len(edge_words)}
    not_lifted = [w for w in edge_words if w not in edge_at]
    if not_lifted:
        print("test_lifter_semantics: %d edge words do not lift: %s" % (
            len(not_lifted), " ".join("%08x" % w for w in not_lifted[:10])))
        return 1
    seqs = sequences(rng, [r for r in rows if r[0] < len(word_list) - len(edge_words)], a.sequences)
    lib = build(a, tmp, word_list, [[r[1] for r in s] for s in seqs])
    if lib.map_window(ctypes.c_uint64(PAGE), ctypes.c_uint64(PAGE_SIZE)) != 0:
        print("test_lifter_semantics: could not map the window page at the host base")
        return 1
    uc = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    uc.ctl_set_cpu_model(A.UC_CPU_ARM64_MAX)
    code_end = CODE + 4 * (max(r[0] for r in rows) + 1)
    uc.mem_map(CODE, (code_end - CODE + 0xFFF) & ~0xFFF)
    uc.mem_map(PAGE, PAGE_SIZE)
    uc.mem_map(SEQ_CODE, (SEQ_STRIDE * len(seqs) + 0xFFF) & ~0xFFF or 0x1000)
    for i, w, *_ in rows:
        uc.mem_write(CODE + 4 * i, struct.pack("<I", w))
    for k, s in enumerate(seqs):
        uc.mem_write(SEQ_CODE + SEQ_STRIDE * k, b"".join(struct.pack("<I", r[1]) for r in s))
    check = Checker(lib, uc, A)

    failures = collections.defaultdict(list)
    compared = skipped = emulator_faults = 0
    # (fast paths run and slow paths taken, with FZ set and without)
    paths = {fz: collections.Counter() for fz in (False, True)}
    by_mnemonic = collections.defaultdict(collections.Counter)
    faulted = collections.Counter()

    def tally(st, mnemonic, ours):
        t = paths[bool(st["fpcr"] & FPCR_FZ)]
        t["runs"] += ours["runs"]
        t["slow"] += ours["slow"]
        by_mnemonic[mnemonic]["runs"] += ours["runs"]
        by_mnemonic[mnemonic]["slow"] += ours["slow"]

    # Each word on its own, from random states.
    edge_rows = set(edge_at.values())
    for row in rows:
        i, w, kind, rn, rm, size, text = row
        mnemonic = text.split(" ")[0]
        if i in edge_rows:
            continue
        if kind == "lit" or left_out(mnemonic, text):
            skipped += 1
            continue
        for _ in range(a.trials):
            if kind == "mem" and rm is not None and rm == rn:
                continue
            st = random_state(rng)
            place(rng, st, row)
            theirs = check.theirs(st, CODE + 4 * i, 1)
            if theirs is None:
                emulator_faults += 1
                faulted[mnemonic] += 1
                continue
            ours = check.ours(st, CODE + 4 * i)
            tally(st, mnemonic, ours)
            fixed = signed_minmax(mnemonic, w, size or 0, st["x"], st["sp"], rn, st["page"])
            if fixed is not None:
                theirs["page"] = fixed
            compared += 1
            diffs = differences(st, ours, theirs, rn != 31)
            if diffs:
                failures[mnemonic].append("%08x %s (fpcr %#x): %s" % (w, text, st["fpcr"], "; ".join(diffs[:4])))
                break

    # The edges, each from its fixed state.
    edge_compared = 0
    texts = {r[1]: r[6] for r in rows}
    for w, given in edges:
        text = texts[w]
        mnemonic = text.split(" ")[0]
        st = random_state(rng)
        for n, value in given.get("x", {}).items():
            st["x"][n] = value
        for n, value in given.get("v", {}).items():
            st["v"][n] = value
        if "nzcv" in given:
            st["nzcv"] = given["nzcv"]
        at = CODE + 4 * edge_at[w]
        theirs = check.theirs(st, at, 1)
        if theirs is None:
            failures["edge " + mnemonic].append("%08x %s: Unicorn faulted" % (w, text))
            continue
        ours = check.ours(st, at)
        tally(st, mnemonic, ours)
        edge_compared += 1
        diffs = differences(st, ours, theirs, False)
        if diffs:
            failures["edge " + mnemonic].append("%08x %s from %s: %s" % (
                w, text, ", ".join("x%d %#x" % kv for kv in given.get("x", {}).items()) or "the state drawn",
                "; ".join(diffs[:4])))

    # Sequences: each word's result the next one's input. FZ is left clear here, so that the one difference the check
    # lets pass (min_normal_kept) cannot reach a register or a flag through a later word.
    check.watch_page()
    seq_compared = seq_outside = 0
    for k, s in enumerate(seqs):
        start = SEQ_CODE + SEQ_STRIDE * k
        for _ in range(a.trials):
            st = random_state(rng, fz=False)
            for row in reversed(s):   # (the first word's base wins where two share a register)
                place(rng, st, row)
            theirs = check.theirs(st, start, len(s))
            if theirs is None:
                seq_outside += 1
                continue
            ours = check.ours(st, start)
            tally(st, "sequence", ours)
            seq_compared += 1
            diffs = differences(st, ours, theirs, all(r[3] != 31 for r in s if r[2] == "mem"))
            if diffs:
                failures["sequence"].append("%s: %s" % (" ; ".join("%08x %s" % (r[1], r[6]) for r in s),
                                                        "; ".join(diffs[:4])))
                break

    print("test_lifter_semantics: %d runs compared over %d words, %d words left out, %d emulator faults" % (
        compared, len(rows) - skipped - len(edge_at), skipped, emulator_faults))
    print("test_lifter_semantics: %d fixed edge states compared over %d words" % (edge_compared, len(edge_at)))
    print("test_lifter_semantics: %d runs of %d sequences compared (%d left the page or faulted in Unicorn)" % (
        seq_compared, len(seqs), seq_outside))
    if faulted:
        print("test_lifter_semantics: Unicorn faulted on %s" % ", ".join("%s %d" % kv for kv in faulted.most_common(12)))
    for fz in (False, True):
        t = paths[fz]
        print("test_lifter_semantics: FZ %s: %d float fast paths ran, %d took the slow path (%.1f%%)" % (
            "set" if fz else "clear", t["runs"], t["slow"], 100.0 * t["slow"] / max(1, t["runs"])))
    one_path = sorted(m for m, t in by_mnemonic.items() if t["runs"] and t["slow"] in (0, t["runs"]))
    print("test_lifter_semantics: %d mnemonics ran fast paths, %d of them only one of the two%s" % (
        sum(1 for t in by_mnemonic.values() if t["runs"]), len(one_path),
        (": " + ", ".join(one_path[:20])) if one_path else ""))
    if failures:
        total = sum(len(v) for v in failures.values())
        print("test_lifter_semantics: %d words differ from Unicorn, by mnemonic:" % total)
        for m, cases in sorted(failures.items(), key=lambda kv: -len(kv[1])):
            print("  %s: %d" % (m, len(cases)))
            for c in cases[:3]:
                print("    " + c)
        return 1
    if seqs and seq_compared < a.trials * len(seqs) // 2:
        print("test_lifter_semantics: only %d of %d sequence runs stayed in the page: too few compared" % (
            seq_compared, a.trials * len(seqs)))
        return 1
    for fz in (False, True):
        t = paths[fz]
        if t["slow"] == 0 or t["slow"] == t["runs"]:
            print("test_lifter_semantics: with FZ %s, %d of %d fast paths took the slow path: both must run" % (
                "set" if fz else "clear", t["slow"], t["runs"]))
            return 1
    print("test_lifter_semantics: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
