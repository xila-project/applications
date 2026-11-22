/* A linked application: startup capabilities are borrowed, opened files owned. */
#include "application.h"
#include <inttypes.h>
#include <stdio.h>
#include <string.h>

static const char *status_name(XilaStatus status, char hex[static 11]) {
    switch (status) {
#define STATUS(name) case XILA_STATUS_##name: return #name
        STATUS(OK); STATUS(INVALID_ARGUMENT); STATUS(NOT_FOUND);
        STATUS(PERMISSION_DENIED); STATUS(ALREADY_EXISTS); STATUS(NOT_A_DIRECTORY);
        STATUS(IS_A_DIRECTORY); STATUS(NO_SPACE); STATUS(OUT_OF_MEMORY);
        STATUS(WOULD_BLOCK); STATUS(UNSUPPORTED); STATUS(OVERFLOW);
        STATUS(UNAVAILABLE); STATUS(INPUT_OUTPUT); STATUS(BAD_DESCRIPTOR);
        STATUS(RESOURCE_BUSY); STATUS(FILE_TOO_LARGE); STATUS(INVALID_PATH);
        STATUS(NAME_TOO_LONG); STATUS(WRITE_ZERO);
#undef STATUS
        default:
            (void)snprintf(hex, 11, "0x%08" PRIx32, (uint32_t)status);
            return hex;
    }
}

/* Successful writes may be short. Zero progress must not spin forever. */
static XilaStatus write_all(const XilaApi *api, XilaFile *file, const char *text, size_t length) {
    while (length != 0) {
        size_t count = 0;
        XilaStatus status = api->file_write(file, text, length, &count);
        if (status != XILA_STATUS_OK) return status;
        if (count == 0) return XILA_STATUS_WRITE_ZERO;
        text += count;
        length -= count;
    }
    return XILA_STATUS_OK;
}

static int32_t failure(const XilaStartup *startup, const char *operation, XilaStatus status) {
    const XilaApi *api = startup->api;
    XilaFile *error = startup->standard_error;
    (void)write_all(api, error, operation, strlen(operation));
    (void)write_all(api, error, ": ", 2);
    char hex[11];
    const char *name = status_name(status, hex);
    (void)write_all(api, error, name, strlen(name));
    (void)write_all(api, error, "\n", 1);
    return 1;
}

/* Whether a host running `running` accepts an executable built for `declared`
   (packed as `(major << 16) | minor`): before 1.0 the minor versions must match;
   from 1.0 the majors must match and the running minor be at least `declared`. */
static bool xila_version_compatible(uint32_t declared, uint32_t running) {
    uint32_t major = declared >> 16;
    uint32_t minor = declared & UINT32_C(0xffff);
    if (major != (running >> 16)) return false;
    uint32_t running_minor = running & UINT32_C(0xffff);
    return major == 0 ? minor == running_minor : minor <= running_minor;
}

int32_t c_demo_main(const XilaStartup *startup) {
    if (startup == NULL || startup->size < sizeof(*startup)) return 1;
    const XilaApi *api = startup->api;
    uint32_t compiled_version =
        ((uint32_t)XILA_VERSION_MAJOR << 16) | (uint32_t)XILA_VERSION_MINOR;
    if (api == NULL || api->size < sizeof(*api) ||
        !xila_version_compatible(compiled_version, api->version)) return 1;
    XilaStatus status;
#define CHECK(operation) do { status = (operation); if (status != XILA_STATUS_OK) \
    return failure(startup, #operation, status); } while (0)
#define PRINT(text) CHECK(write_all(api, startup->standard_output, text, strlen(text)))
    PRINT("Hello from native C!\n");
    for (size_t i = 0; i < startup->argument_count; ++i) {
        PRINT("argument: ");
        CHECK(write_all(api, startup->standard_output, startup->arguments[i].data,
                        startup->arguments[i].length));
        PRINT("\n");
    }
    char line[128];
    char hex[11];
    uint32_t version = api->version;
    (void)snprintf(line, sizeof(line), "ABI: %" PRIu32 ".%" PRIu32 "\n",
                   version >> 16, version & UINT32_C(65535));
    PRINT(line);
    uint64_t monotonic = 0;
    int64_t realtime = 0;
    status = api->time_monotonic(&monotonic);
    (void)snprintf(line, sizeof(line), "monotonic: %s (%" PRIu64 " ns)\n", status_name(status, hex), monotonic);
    PRINT(line);
    status = api->time_realtime(&realtime);
    (void)snprintf(line, sizeof(line), "realtime: %s (%" PRId64 " ns)\n", status_name(status, hex), realtime);
    PRINT(line);

    XilaFile *file = NULL;
    CHECK(api->file_open(startup->home, "c-demo.txt", 10,
        XILA_OPEN_READ | XILA_OPEN_WRITE | XILA_OPEN_CREATE | XILA_OPEN_TRUNCATE, &file));
    static const char contents[] = "A file written through Xila's C ABI.\n";
    status = write_all(api, file, contents, sizeof(contents) - 1);
    if (status == XILA_STATUS_OK) status = api->file_flush(file);
    if (status == XILA_STATUS_OK) status = api->file_seek(file, 0, XILA_SEEK_START, NULL);
    if (status == XILA_STATUS_OK) {
        for (;;) {
            size_t count = 0;
            status = api->file_read(file, line, sizeof(line), &count);
            if (status != XILA_STATUS_OK || count == 0) break;
            status = write_all(api, startup->standard_output, line, count);
            if (status != XILA_STATUS_OK) break;
        }
    }
    XilaStatus closed = api->file_close(file);
    if (status != XILA_STATUS_OK) return failure(startup, "home file", status);
    CHECK(closed);
    CHECK(api->file_flush(startup->standard_output));
    return 0;
}
