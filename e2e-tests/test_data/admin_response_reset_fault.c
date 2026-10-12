// Reject only post-receive socket timeout retirement in the opted-in test child.
// This models an OS error after a valid fragmented HTTP response, not a read retry.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <stdio.h>
#include <sys/socket.h>
#include <sys/time.h>

static _Atomic int received[65536];
#if !defined(__APPLE__) && !defined(BIFROST_E2E_DARWIN_FORWARD_TEST)
static pthread_once_t initialized = PTHREAD_ONCE_INIT;
static ssize_t (*real_recv)(int, void *, size_t, int);
static int (*real_setsockopt)(int, int, int, const void *, socklen_t);
#else
// dyld exempts the interposing image from its own replacements. Direct bindings
// call the original implementation; RTLD_NEXT can return our replacement.
#define real_recv recv
#define real_setsockopt setsockopt
#endif
__attribute__((constructor)) static void mark_loaded(void) {
    const char *path = getenv("BIFROST_E2E_SOCKET_RESET_LIBRARY_LOADED");
    if (path) {
        FILE *file = fopen(path, "w");
        if (file) { fputs("loaded\n", file); fclose(file); }
    }
}
#if !defined(__APPLE__) && !defined(BIFROST_E2E_DARWIN_FORWARD_TEST)
static void initialize(void) {
    real_recv = dlsym(RTLD_NEXT, "recv");
    real_setsockopt = dlsym(RTLD_NEXT, "setsockopt");
}
#endif
static ssize_t fault_recv(int fd, void *buffer, size_t length, int flags) {
#if !defined(__APPLE__) && !defined(BIFROST_E2E_DARWIN_FORWARD_TEST)
    pthread_once(&initialized, initialize);
#endif
    ssize_t n = real_recv(fd, buffer, length, flags);
    if (n > 0 && fd >= 0 && fd < 65536) atomic_store(&received[fd], 1);
    return n;
}
static int fault_setsockopt(int fd, int level, int option, const void *value, socklen_t length) {
#if !defined(__APPLE__) && !defined(BIFROST_E2E_DARWIN_FORWARD_TEST)
    pthread_once(&initialized, initialize);
#endif
    if (getenv("BIFROST_E2E_SOCKET_RESET_FAULT") && level == SOL_SOCKET && option == SO_RCVTIMEO &&
        length == sizeof(struct timeval) && fd >= 0 && fd < 65536 && atomic_load(&received[fd])) {
        const struct timeval *timeout = value;
        if (timeout->tv_sec == 0 && timeout->tv_usec == 0) { errno = EINVAL; return -1; }
    }
    return real_setsockopt(fd, level, option, value, length);
}
#if defined(__APPLE__) && !defined(BIFROST_E2E_DARWIN_FORWARD_TEST)
__attribute__((used)) static const struct { const void *replacement; const void *original; }
interposes[] __attribute__((section("__DATA,__interpose"))) = {
    { (const void *)fault_recv, (const void *)recv },
    { (const void *)fault_setsockopt, (const void *)setsockopt }
};
#elif !defined(BIFROST_E2E_DARWIN_FORWARD_TEST)
ssize_t recv(int fd, void *buffer, size_t length, int flags) { return fault_recv(fd, buffer, length, flags); }
int setsockopt(int fd, int level, int option, const void *value, socklen_t length) {
    return fault_setsockopt(fd, level, option, value, length);
}
#endif
