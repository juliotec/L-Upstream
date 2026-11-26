/* host/guest_profile.c's writes from two threads at once (the timer thread's and the one at exit): one at a time, each
   file whole, none failing, and none after the last (at exit). Includes the file as it is (its functions are static);
   tests/test_guest_profile_write.py builds and runs it.

       guest_profile_test <path>     exit 0: every write whole; 1: a write failed or a file was not whole */
#include "../host/guest_profile.c"
#include <pthread.h>

/* (host/posix's: the timer thread is never started here, HOS_GUEST_PROFILE unset) */
void Sleep(DWORD ms) { (void)ms; }
HANDLE CreateThread(void *attr, SIZE_T stack, LPTHREAD_START_ROUTINE fn, void *arg, DWORD flags, DWORD *id) {
    (void)attr, (void)stack, (void)fn, (void)arg, (void)flags, (void)id;
    return NULL;
}

static uint64_t counts[4096];
static const char *out;

static void *writer(void *arg) {
    (void)arg;
    for (int i = 0; i < 200; i++) lsp_write(out, 0);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    out = argv[1];
    for (int i = 0; i < 4096; i++) counts[i] = (uint64_t)i + 1;
    lsp_register(counts, 4096, "unit_a");
    pthread_t t[2];
    for (int i = 0; i < 2; i++) pthread_create(&t[i], NULL, writer, NULL);
    for (int i = 0; i < 2; i++) pthread_join(t[i], NULL);
    lsp_write(out, 1);
    counts[0] = 999999;   /* (written after the last: never) */
    lsp_write(out, 0);
    FILE *f = fopen(out, "rb");
    if (!f) return 1;
    char line[128];
    int lines = 0, bad = 0;
    while (fgets(line, sizeof line, f)) {
        unsigned long long i, c;
        if (lines++ == 0) bad |= strcmp(line, "unit unit_a\n") != 0;
        else bad |= sscanf(line, "%llu %llu", &i, &c) != 2 || c != i + 1;
    }
    fclose(f);
    if (bad || lines != 4097) {
        printf("FAIL: the file is not whole (%d lines)\n", lines);
        return 1;
    }
    printf("ok\n");
    return 0;
}
