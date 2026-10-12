// Exercise real socket forwarding and the narrowly scoped retirement fault.
#include <assert.h>
#include <errno.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#ifdef BIFROST_E2E_DARWIN_FORWARD_TEST
// A Darwin forwarder must never resolve through dlsym: dyld already supplies
// original bindings in the interposing image. Fail deterministically if it does.
#include <dlfcn.h>
void *dlsym(void *handle, const char *name) {
    (void)handle;
    (void)name;
    abort();
}
#include "admin_response_reset_fault.c"
#define probe_recv fault_recv
#define probe_setsockopt fault_setsockopt
#else
#define probe_recv recv
#define probe_setsockopt setsockopt
#endif

int main(void) {
    int sockets[2];
    char byte;
    struct timeval zero = {0, 0}, nonzero = {1, 0};
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    assert(probe_setsockopt(sockets[0], SOL_SOCKET, SO_RCVTIMEO, &zero, sizeof(zero)) == 0);
    assert(write(sockets[1], "x", 1) == 1);
    assert(probe_recv(sockets[0], &byte, 1, 0) == 1 && byte == 'x');
    assert(probe_setsockopt(sockets[0], SOL_SOCKET, SO_RCVTIMEO, &nonzero, sizeof(nonzero)) == 0);
    assert(setenv("BIFROST_E2E_SOCKET_RESET_FAULT", "1", 1) == 0);
    errno = 0;
    assert(probe_setsockopt(sockets[0], SOL_SOCKET, SO_RCVTIMEO, &zero, sizeof(zero)) == -1);
    assert(errno == EINVAL);
    assert(unsetenv("BIFROST_E2E_SOCKET_RESET_FAULT") == 0);
    assert(probe_setsockopt(sockets[0], SOL_SOCKET, SO_RCVTIMEO, &zero, sizeof(zero)) == 0);
    assert(close(sockets[0]) == 0 && close(sockets[1]) == 0);
    return 0;
}
