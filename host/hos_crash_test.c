/* SPDX-License-Identifier: GPL-3.0-or-later */
/* (longshot) HOS_CRASH_TEST: a crash on purpose, to see what a crash leaves (the crash record, host/hos_crash_record.c) and
   the crash window the builder shows after it (`longshot-builder play`, builder/crates/crash). =start crashes just after the
   crash handlers are set up, =<seconds> that long after. From the environment only (not longshot_env.txt: hos_main.c's
   env_line_allowed). */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <errno.h>
#include <pthread.h>
#include <time.h>
#endif

void hos_log(const char *fmt, ...);
void hos_log_flush(void);

static void crash_now(void) {
    hos_log("crash test: crashing now (HOS_CRASH_TEST)");
    hos_log_flush();
    volatile uintptr_t where = 16;
    *(volatile int *)where = 1;
}
#ifdef _WIN32
static DWORD WINAPI crash_later(void *p) {
    Sleep((DWORD)(uintptr_t)p * 1000);
    crash_now();
    return 0;
}
#else
static void *crash_later(void *p) {
    struct timespec t = {(time_t)(uintptr_t)p, 0};
    while (nanosleep(&t, &t) && errno == EINTR) {}
    crash_now();
    return NULL;
}
#endif
/* After the crash handlers are set up (hos_main.c reserve_guest): start, at once; a number, that many seconds later */
void hos_crash_test_arm(void) {
    const char *e = getenv("HOS_CRASH_TEST");
    if (!e || !*e) return;
    if (!strcmp(e, "start")) crash_now();
    const int seconds = atoi(e);
    if (seconds <= 0) return;
#ifdef _WIN32
    HANDLE t = CreateThread(NULL, 0, crash_later, (void *)(uintptr_t)seconds, 0, NULL);
    if (t) CloseHandle(t);
#else
    pthread_t t;
    if (!pthread_create(&t, NULL, crash_later, (void *)(uintptr_t)seconds)) pthread_detach(t);
#endif
}
