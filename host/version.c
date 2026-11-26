/* Which Longshot this is (builder/crates/build-compile/src/generated/version.rs: the release and a fingerprint of the sources), for the log's first line, the
   control bridge, the Discord status and the crash record. builder/crates/build-compile gives LONGSHOT_VERSION to this file alone: the
   fingerprint changes with every source, and a new one then compiles this one small file, not hos_main.c. */
#ifndef LONGSHOT_VERSION
#define LONGSHOT_VERSION "unknown"
#endif

const char *hos_version(void) { return LONGSHOT_VERSION; }
