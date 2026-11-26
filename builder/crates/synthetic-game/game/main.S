// The synthetic game: a small AArch64 program, written here by hand, that the builder's end-to-end test builds as it
// builds the game (builder/crates/synthetic-game/tests/end_to_end.rs): assembled and linked by the pinned clang at test
// time into a module, made an NSO, packed in a .nup with a main.npdm and a small RomFS, then read, laid out, lifted,
// compiled, linked with the test's host (game/host/) and run. Nothing in it comes from the game.
//
// nnMain (the entry the host calls, as the game's) returns one 64-bit result made of:
//   - integer arithmetic in a loop (mul, eor and add with shifted operands, udiv, msub, madd, csel, ror);
//   - floating point (scvtf, fmul, fadd, fsqrt, fcvt, fcvtzs; double and single precision);
//   - memory (a .bss table written and read back with a register offset and ldp's post-index, a .data word changed);
//   - indirect calls through a table of function pointers in .data (relocated by the image);
//   - an SVC the host answers (a64_svc in game/host/synthetic_host.c);
//   - two RomFS files read through an import the host defines (imp_testhost_romfs_read, over host/romfs.c's API): one
//     the .nup stores as it is, one it stores as a zstd frame;
//   - `big`, a function large enough to be a unit of its own, so that a change elsewhere leaves it compiled.
// src/lib.rs's `expected_result` computes the same in Rust; the test compares the two.
//
// SVC_ARG is what ask_host passes the host: the test's second build changes it, one instruction of one function, as an
// ExeFS patch changes the game.

#ifndef SVC_ARG
#define SVC_ARG 5
#endif
#ifndef BIG_ADDS
#define BIG_ADDS 16400
#endif
#define BUFFER_SIZE 512

    // ---- the module's header: the word at offset 4 is the MOD0 header's offset (the system's loader reads it)
    .section .text.crt0, "ax"
    .global __module_start
__module_start:
    .word 0
    .word __mod0 - __module_start
    .balign 8
__mod0:
    .ascii "MOD0"
    .word _DYNAMIC - __mod0
    .word __bss_start - __mod0
    .word __bss_end - __mod0
    .word 0, 0                       // (no .eh_frame_hdr)
    .word 0                          // (no module object)

    .text

    // ---- integers: x0 = a
    .global int_ops
    .type int_ops, %function
int_ops:
    mov x1, #0                       // acc
    mov x2, #1                       // i
1:  mul x3, x2, x0
    eor x1, x1, x3, lsr #1           // acc ^= (i * a) >> 1
    add x1, x1, x2, lsl #4           // acc += i << 4
    add x2, x2, #1
    cmp x2, #20
    b.le 1b
    mov x4, #7
    udiv x5, x1, x4                  // q = acc / 7
    msub x6, x5, x4, x1              // r = acc - q * 7
    madd x0, x5, x6, x1              // q * r + acc
    cmp x6, #3
    mov x7, #0x55
    csel x7, x7, xzr, hi             // r > 3 ? 0x55 : 0
    add x0, x0, x7
    ror x0, x0, #13
    ret
    .size int_ops, . - int_ops

    // ---- floating point: x0 = n
    .global float_ops
    .type float_ops, %function
float_ops:
    scvtf d0, x0
    fmov d1, #2.5
    fmul d0, d0, d1
    fmov d2, #0.75
    fadd d0, d0, d2
    fsqrt d0, d0                     // sqrt(n * 2.5 + 0.75)
    mov x1, #1000
    scvtf d3, x1
    fmul d0, d0, d3
    fcvtzs x0, d0                    // (int64) (sqrt(...) * 1000)
    fcvt s4, d0
    fmov s5, #0.5
    fmul s4, s4, s5
    fcvtzs w6, s4                    // (int32) ((float) (sqrt(...) * 1000) * 0.5f)
    add x0, x0, x6
    ret
    .size float_ops, . - float_ops

    // ---- memory
    .global memory_ops
    .type memory_ops, %function
memory_ops:
    adrp x1, squares
    add x1, x1, :lo12:squares
    mov x2, #0
1:  mul x3, x2, x2
    str x3, [x1, x2, lsl #3]         // squares[i] = i * i
    add x2, x2, #1
    cmp x2, #16
    b.lt 1b
    mov x0, #0
    mov x2, #8
2:  ldp x3, x4, [x1], #16            // the sum, two at a time
    add x0, x0, x3
    add x0, x0, x4
    subs x2, x2, #1
    b.ne 2b
    adrp x5, counter
    add x5, x5, :lo12:counter
    ldr w6, [x5]
    add w6, w6, #1
    str w6, [x5]                     // counter += 1
    ldrb w7, [x5, #1]                // its second byte
    add x0, x0, x7
    add x0, x0, x6
    ret
    .size memory_ops, . - memory_ops

    // ---- the functions the indirect calls reach
    .global fn_square
    .type fn_square, %function
fn_square:
    mul x0, x0, x0
    ret
    .size fn_square, . - fn_square

    .global fn_triple
    .type fn_triple, %function
fn_triple:
    add x0, x0, x0, lsl #1
    ret
    .size fn_triple, . - fn_triple

    // ---- the host, by an SVC: it gives back x0 * 1000 + 1
    .global ask_host
    .type ask_host, %function
ask_host:
    mov x0, #SVC_ARG
    svc #0x7f
    ret
    .size ask_host, . - ask_host

    // ---- a checksum of x1 bytes at x0: the sum of each byte times its place (from 1)
    .global checksum
    .type checksum, %function
checksum:
    mov x2, #0
    mov x3, #0
    cbz x1, 2f
1:  ldrb w4, [x0, x3]
    add x3, x3, #1
    madd x2, x4, x3, x2
    cmp x3, x1
    b.lo 1b
2:  mov x0, x2
    ret
    .size checksum, . - checksum

    // ---- a RomFS file read by the host (x0: its path) into `buffer`: x0 its checksum plus its size << 32
    .global read_file
    .type read_file, %function
read_file:
    stp x29, x30, [sp, #-32]!
    mov x29, sp
    str x19, [sp, #16]
    adrp x1, buffer
    add x1, x1, :lo12:buffer
    mov x2, #BUFFER_SIZE
    bl testhost_romfs_read           // (an import: the host's imp_testhost_romfs_read)
    mov x19, x0                      // its size, or -1
    tbnz x19, #63, 1f
    mov x1, x19
    adrp x0, buffer
    add x0, x0, :lo12:buffer
    bl checksum
    add x0, x0, x19, lsl #32
1:  ldr x19, [sp, #16]
    ldp x29, x30, [sp], #32
    ret
    .size read_file, . - read_file

    // ---- the entry
    .global nnMain
    .type nnMain, %function
nnMain:
    stp x29, x30, [sp, #-48]!
    mov x29, sp
    stp x19, x20, [sp, #16]
    str x21, [sp, #32]
    mov x0, #0x1234
    bl int_ops
    mov x19, x0
    mov x0, #10
    bl float_ops
    add x19, x19, x0
    bl memory_ops
    add x19, x19, x0
    adrp x20, ops_table
    add x20, x20, :lo12:ops_table
    ldr x9, [x20, #8]                // fn_triple
    mov x0, #7
    blr x9
    add x19, x19, x0
    ldr x9, [x20]                    // fn_square
    mov x0, #9
    blr x9
    add x19, x19, x0
    bl ask_host
    add x19, x19, x0
    adrp x0, path_plain
    add x0, x0, :lo12:path_plain
    bl read_file
    add x19, x19, x0
    adrp x0, path_packed
    add x0, x0, :lo12:path_packed
    bl read_file
    eor x19, x19, x0, ror #8
    mov x0, #0
    bl big
    add x0, x19, x0
    ldr x21, [sp, #32]
    ldp x19, x20, [sp, #16]
    ldp x29, x30, [sp], #48
    ret
    .size nnMain, . - nnMain

    // ---- big: x0 + 3 * BIG_ADDS, one add at a time. Its estimated compile cost (instructions plus their square over
    // 4,096) is past a unit's budget (80,000), so the lift gives it a unit of its own (docs/dev/reference/builder.md,
    // "Units"), the last function of the .text
    .global big
    .type big, %function
big:
    .rept BIG_ADDS
    add x0, x0, #3
    .endr
    ret
    .size big, . - big

    // ---- data
    .section .rodata
path_plain:
    .asciz "Synthetic/plain.txt"
path_packed:
    .asciz "Synthetic/packed.bin"

    .data
    .balign 8
    .global ops_table
ops_table:
    .quad fn_square, fn_triple       // (relative relocations: the image writes their addresses)
counter:
    .word 0x11223344

    .bss
    .balign 8
squares:
    .zero 16 * 8
buffer:
    .zero BUFFER_SIZE
