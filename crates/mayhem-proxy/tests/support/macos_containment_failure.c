/* Fixed test-only fault injection. Never linked into the bundled worker. */
#include <sandbox.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int issued = 0;
static const char diagnostic[] = "synthetic sandbox refusal";

static int reject_named_profile(const char *profile, uint64_t flags, char **error) {
    if (profile != kSBXProfilePureComputation || flags != SANDBOX_NAMED ||
        error == NULL || issued != 0) {
        _exit(75);
    }
    *error = strdup(diagnostic);
    if (*error == NULL) {
        _exit(76);
    }
    issued = 1;
    const char marker[] = "forced sandbox refusal\n";
    if (write(STDERR_FILENO, marker, sizeof(marker) - 1) != sizeof(marker) - 1) {
        _exit(77);
    }
    return -1;
}

static void release_synthetic_error(char *error) {
    if (issued != 1 || error == NULL || strcmp(error, diagnostic) != 0) {
        _exit(78);
    }
    issued = 2;
    free(error);
    const char marker[] = "error buffer released\n";
    if (write(STDERR_FILENO, marker, sizeof(marker) - 1) != sizeof(marker) - 1) {
        _exit(79);
    }
}

/* Apple's dyld __interpose pair ABI; only the two named test substitutions. */
__attribute__((used, section("__DATA,__interpose")))
static const struct {
    const void *replacement;
    const void *original;
} substitutions[] = {
    {(const void *)(uintptr_t)&reject_named_profile, (const void *)(uintptr_t)&sandbox_init},
    {(const void *)(uintptr_t)&release_synthetic_error, (const void *)(uintptr_t)&sandbox_free_error},
};
