/* (longshot) setjmp / longjmp in lifted code.

   The guest's __setjmp and longjmp (sdk+0xaec40 / +0xaed50, musl's with pointer-auth hashes) run as lifted code and
   save and restore the guest registers themselves. What lifting breaks is longjmp's last instruction, BR x30 to the
   setjmp call's return address: in lifted code that address is the middle of a C function, not an entry, so the
   indirect jump trapped (OoT's UpdateTextBoxTextureThread, at 0x81681690). And the function that called setjmp may
   have returned by then: 0x81681670 is a wrapper that returns setjmp's result, which works on the console because
   its stack frame is still intact when longjmp comes back to it.

   So the lifter (a64_lift.py find_setjmp_owners) makes each setjmp return address an entry, and wraps every call of
   a function that calls setjmp (its "owner") as

       { void **sj = recomp_sj_push(c, r0..r3); if (recomp_sj_setjmp(sj) == 0) f_owner(c);
         else a64_icall(c, recomp_sj_target(sj)); recomp_sj_pop(sj); }

   in the caller's own frame, which is alive while the owner runs and after it returns. A jump that misses
   (a64_call_miss) to one of a pushed slot's return addresses is a longjmp: the guest registers are restored already
   (x0 the value, SP and x30 as setjmp saw them), so the newest such slot is taken (slots above it dropped), and
   recomp_sj_longjmp brings the host back to that caller, which runs the owner's code from the return address: what
   the owner would have done, ending in its own return to the caller.

   (recomp_sj_setjmp / recomp_sj_longjmp are this file's own, in assembly: every Win64 callee-saved register, xmm6-15
   with them, and the stack pointer and return address. clang's __builtin_setjmp, which this used first, saves the
   frame address, which on Win64 sits 128 bytes below the frame pointer clang addresses the frame's spill slots with,
   and its longjmp set rbp to it: the caller's reloads of c and the slot read garbage, and the text box thread
   called a stack address after its first longjmp.) */
#include "hos.h"
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <windows.h>
#include <string.h>

#define DEPTH 256
typedef struct SjStack SjStack;
typedef struct { uint64_t rets[4], target; SjStack *stack; void *buf[32]; } SjSlot;   /* (buf: recomp_sj_setjmp's 248 bytes) */
struct SjStack { SjSlot slot[DEPTH]; int depth, overflow; };

/* The slots are a guest context's, not a host thread's: a guest fiber that set a jump on one host thread may be resumed
   on another (hos_thread.c fiber_switch) and longjmp there, on the fiber's own host stack, which went with it. So each
   context (a thread's or a fiber's Ctx) has its stack, found by its address; hos_setjmp_forget lets a context's go when
   the context ends (a thread destroyed, a fiber finalized or set up again), so a context made at the same address later
   starts empty. A slot knows its stack, so recomp_sj_pop needs no context */
typedef struct { const Ctx *c; SjStack *stack; } SjOwner;
static SRWLOCK owners_lock = SRWLOCK_INIT;
static SjOwner *owners;
static int nowners, cap_owners;

static SjStack *stack_of(const Ctx *c, int make) {
    SjStack *found = NULL;
    AcquireSRWLockShared(&owners_lock);
    for (int i = 0; i < nowners && !found; i++)
        if (owners[i].c == c) found = owners[i].stack;
    ReleaseSRWLockShared(&owners_lock);
    if (found || !make) return found;
    SjStack *s = (SjStack *)calloc(1, sizeof *s);
    if (!s) return NULL;
    AcquireSRWLockExclusive(&owners_lock);
    if (nowners == cap_owners) {
        const int cap = cap_owners ? 2 * cap_owners : 16;
        SjOwner *grown = (SjOwner *)realloc(owners, sizeof *owners * (size_t)cap);
        if (!grown) {
            ReleaseSRWLockExclusive(&owners_lock);
            free(s);
            return NULL;
        }
        owners = grown, cap_owners = cap;
    }
    owners[nowners++] = (SjOwner){c, s};   /* (only c's own thread makes c's: no other can have raced it in) */
    ReleaseSRWLockExclusive(&owners_lock);
    return s;
}

void hos_setjmp_forget(const Ctx *c) {
    SjStack *gone = NULL;
    AcquireSRWLockExclusive(&owners_lock);
    for (int i = 0; i < nowners; i++)
        if (owners[i].c == c) {
            gone = owners[i].stack;
            owners[i] = owners[--nowners];
            break;
        }
    ReleaseSRWLockExclusive(&owners_lock);
    free(gone);
}

/* (tests: how many slots c has pushed) */
int hos_setjmp_depth(const Ctx *c) {
    const SjStack *s = stack_of(c, 0);
    return s ? s->depth : 0;
}

/* a push refused (more than DEPTH owners nested): its setjmp still runs, but no return address matches it. One for the
   process: its buffer is only ever written (two threads refused at once write their registers over each other's, which
   nothing reads: no longjmp goes to it, its target stays 0), and it outlives every thread, so a fiber that pushed it on
   one host thread pops it on another after the first has ended (a thread's own was freed with that thread). recomp_sj_pop
   knows it by its address */
static SjSlot refused;

#ifdef _WIN32
/* buf: rbx rbp rdi rsi r12 r13 r14 r15 at 0-56, rsp (after the return) at 64, the return address at 72, xmm6-15 at
   80-224, mxcsr at 240 and the x87 control word at 244 */
__attribute__((naked, returns_twice)) int recomp_sj_setjmp(void **buf) {
    __asm__ volatile(
        "movq %rbx, 0(%rcx)\n\t"
        "movq %rbp, 8(%rcx)\n\t"
        "movq %rdi, 16(%rcx)\n\t"
        "movq %rsi, 24(%rcx)\n\t"
        "movq %r12, 32(%rcx)\n\t"
        "movq %r13, 40(%rcx)\n\t"
        "movq %r14, 48(%rcx)\n\t"
        "movq %r15, 56(%rcx)\n\t"
        "leaq 8(%rsp), %rdx\n\t"
        "movq %rdx, 64(%rcx)\n\t"
        "movq (%rsp), %rdx\n\t"
        "movq %rdx, 72(%rcx)\n\t"
        "movdqu %xmm6, 80(%rcx)\n\t"
        "movdqu %xmm7, 96(%rcx)\n\t"
        "movdqu %xmm8, 112(%rcx)\n\t"
        "movdqu %xmm9, 128(%rcx)\n\t"
        "movdqu %xmm10, 144(%rcx)\n\t"
        "movdqu %xmm11, 160(%rcx)\n\t"
        "movdqu %xmm12, 176(%rcx)\n\t"
        "movdqu %xmm13, 192(%rcx)\n\t"
        "movdqu %xmm14, 208(%rcx)\n\t"
        "movdqu %xmm15, 224(%rcx)\n\t"
        "stmxcsr 240(%rcx)\n\t"
        "fnstcw 244(%rcx)\n\t"
        "xorl %eax, %eax\n\t"
        "ret\n\t");
}

__attribute__((naked, noreturn)) static void recomp_sj_longjmp(void **buf, int value) {
    __asm__ volatile(
        "movq 0(%rcx), %rbx\n\t"
        "movq 8(%rcx), %rbp\n\t"
        "movq 16(%rcx), %rdi\n\t"
        "movq 24(%rcx), %rsi\n\t"
        "movq 32(%rcx), %r12\n\t"
        "movq 40(%rcx), %r13\n\t"
        "movq 48(%rcx), %r14\n\t"
        "movq 56(%rcx), %r15\n\t"
        "movdqu 80(%rcx), %xmm6\n\t"
        "movdqu 96(%rcx), %xmm7\n\t"
        "movdqu 112(%rcx), %xmm8\n\t"
        "movdqu 128(%rcx), %xmm9\n\t"
        "movdqu 144(%rcx), %xmm10\n\t"
        "movdqu 160(%rcx), %xmm11\n\t"
        "movdqu 176(%rcx), %xmm12\n\t"
        "movdqu 192(%rcx), %xmm13\n\t"
        "movdqu 208(%rcx), %xmm14\n\t"
        "movdqu 224(%rcx), %xmm15\n\t"
        "ldmxcsr 240(%rcx)\n\t"
        "fldcw 244(%rcx)\n\t"
        "movl %edx, %eax\n\t"
        "testl %eax, %eax\n\t"
        "jnz 1f\n\t"
        "incl %eax\n\t"
        "1:\n\t"
        "movq 64(%rcx), %rsp\n\t"
        "jmpq *72(%rcx)\n\t");
}

#else
/* (linux) the System V ABI's: the buffer as above, its callee-saved registers only (rbx rbp r12-r15: rdi, rsi and the
   xmm registers are the caller's to keep there), buf in rdi and the value in esi */
__attribute__((naked, returns_twice)) int recomp_sj_setjmp(void **buf) {
    __asm__ volatile(
        "movq %rbx, 0(%rdi)\n\t"
        "movq %rbp, 8(%rdi)\n\t"
        "movq %r12, 32(%rdi)\n\t"
        "movq %r13, 40(%rdi)\n\t"
        "movq %r14, 48(%rdi)\n\t"
        "movq %r15, 56(%rdi)\n\t"
        "leaq 8(%rsp), %rdx\n\t"
        "movq %rdx, 64(%rdi)\n\t"
        "movq (%rsp), %rdx\n\t"
        "movq %rdx, 72(%rdi)\n\t"
        "stmxcsr 240(%rdi)\n\t"
        "fnstcw 244(%rdi)\n\t"
        "xorl %eax, %eax\n\t"
        "ret\n\t");
}

__attribute__((naked, noreturn)) static void recomp_sj_longjmp(void **buf, int value) {
    __asm__ volatile(
        "movq 0(%rdi), %rbx\n\t"
        "movq 8(%rdi), %rbp\n\t"
        "movq 32(%rdi), %r12\n\t"
        "movq 40(%rdi), %r13\n\t"
        "movq 48(%rdi), %r14\n\t"
        "movq 56(%rdi), %r15\n\t"
        "ldmxcsr 240(%rdi)\n\t"
        "fldcw 244(%rdi)\n\t"
        "movl %esi, %eax\n\t"
        "testl %eax, %eax\n\t"
        "jnz 1f\n\t"
        "incl %eax\n\t"
        "1:\n\t"
        "movq 64(%rdi), %rsp\n\t"
        "jmpq *72(%rdi)\n\t");
}

#endif

void **recomp_sj_push(Ctx *c, uint64_t r0, uint64_t r1, uint64_t r2, uint64_t r3) {
    SjStack *const stack = stack_of(c, 1);
    if (!stack || stack->depth >= DEPTH) {
        /* (too deep: refused, not the outermost slots shifted out, which would move the live slots under the frames
           that hold them; a longjmp to this owner then misses as it would without the wrapper) */
        if (stack && !stack->overflow++) hos_log("setjmp: more than %d nested setjmp owners in one context: refused", DEPTH);
        return refused.buf;
    }
    SjSlot *s = &stack->slot[stack->depth++];
    s->rets[0] = r0, s->rets[1] = r1, s->rets[2] = r2, s->rets[3] = r3, s->target = 0, s->stack = stack;
    return s->buf;
}

uint64_t recomp_sj_target(void **buf) { return ((SjSlot *)((char *)buf - offsetof(SjSlot, buf)))->target; }

void recomp_sj_pop(void **buf) {
    SjSlot *const s = (SjSlot *)((char *)buf - offsetof(SjSlot, buf));
    if (s == &refused) return;   /* (a refused push's: nothing was pushed) */
    SjStack *const stack = s->stack;
    if (!stack) return;
    const int i = (int)(s - stack->slot);
    if (i >= 0 && i < DEPTH && i < stack->depth) stack->depth = i;
}

/* a64_call_miss's first question: a longjmp to a pushed setjmp owner? (does not return when it is) */
void hos_setjmp_return(Ctx *c, uint64_t target) {
    SjStack *const stack = target ? stack_of(c, 0) : NULL;
    if (!stack || !stack->depth) return;
    for (int i = stack->depth - 1; i >= 0; i--) {
        SjSlot *s = &stack->slot[i];
        if (s->rets[0] != target && s->rets[1] != target && s->rets[2] != target && s->rets[3] != target) continue;
        static volatile LONG logged;
        if (InterlockedIncrement(&logged) <= 4)
            hos_log("longjmp to %#llx (value %llu, %d slots above dropped)", (unsigned long long)target,
                    (unsigned long long)c->x[0], stack->depth - 1 - i);
        s->target = target;
        stack->depth = i + 1;
        recomp_sj_longjmp(s->buf, 1);
    }
}

/* ...and every indirect jump's (builder/rt/a64_rt.c a64_ijump_hook): the setjmp return addresses are entries too
   (the callers run them, recomp_sj_target), so the longjmp's BR finds a function there and never missed: it called
   the owner's code inside longjmp's own frame, which returned into longjmp's caller (0x8168215c, the UDF after OoT's
   BL longjmp: the text box thread's "branch to a non-function") */
void a64_ijump_hook(Ctx *c, uint64_t target) { hos_setjmp_return(c, target); }
