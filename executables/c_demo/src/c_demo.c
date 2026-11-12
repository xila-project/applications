/* A linked application: startup capabilities are borrowed, opened files owned. */
#include "application.h"
#include <inttypes.h>
#include <stdio.h>
#include <string.h>

static const char *status_name(XilaStatus status) {
    switch (status) {
#define STATUS(name) case XILA_STATUS_##name: return #name
        STATUS(OK); STATUS(INVALID_ARGUMENT); STATUS(NOT_FOUND);
        STATUS(PERMISSION_DENIED); STATUS(ALREADY_EXISTS); STATUS(NOT_A_DIRECTORY);
        STATUS(IS_A_DIRECTORY); STATUS(NO_SPACE); STATUS(OUT_OF_MEMORY);
        STATUS(WOULD_BLOCK); STATUS(UNSUPPORTED); STATUS(OVERFLOW);
        STATUS(UNAVAILABLE); STATUS(INPUT_OUTPUT);
#undef STATUS
        default: return "UNKNOWN";
    }
}

/* Successful writes may be short. Zero progress must not spin forever. */
static XilaStatus write_all(XilaFile *file, const char *text, size_t length) {
    while (length != 0) {
        size_t count = 0;
        XilaStatus status = xila_file_write(file, text, length, &count);
        if (status != XILA_STATUS_OK) return status;
        if (count == 0) return XILA_STATUS_INPUT_OUTPUT;
        text += count;
        length -= count;
    }
    return XILA_STATUS_OK;
}

static int32_t failure(const XilaStartup *startup, const char *operation, XilaStatus status) {
    XilaFile *error = startup->standard_error;
    (void)write_all(error, operation, strlen(operation));
    (void)write_all(error, ": ", 2);
    (void)write_all(error, status_name(status), strlen(status_name(status)));
    (void)write_all(error, "\n", 1);
    return 1;
}

int32_t c_demo_main(const XilaStartup *startup) {
    if (startup == NULL || startup->size < sizeof(*startup)) return 1;
    XilaStatus status;
#define CHECK(operation) do { status = (operation); if (status != XILA_STATUS_OK) \
    return failure(startup, #operation, status); } while (0)
#define PRINT(text) CHECK(write_all(startup->standard_output, text, strlen(text)))
    PRINT("Hello from native C!\n");
    for (size_t i = 0; i < startup->argument_count; ++i) {
        PRINT("argument: ");
        CHECK(write_all(startup->standard_output, startup->arguments[i].data,
                        startup->arguments[i].length));
        PRINT("\n");
    }
    char line[128];
    uint32_t version = xila_abi_version();
    (void)snprintf(line, sizeof(line), "ABI: %" PRIu32 ".%" PRIu32 "\n",
                   version >> 16, version & UINT32_C(65535));
    PRINT(line);
    uint64_t monotonic = 0;
    int64_t realtime = 0;
    status = xila_time_monotonic(&monotonic);
    (void)snprintf(line, sizeof(line), "monotonic: %s (%" PRIu64 " ns)\n", status_name(status), monotonic);
    PRINT(line);
    status = xila_time_realtime(&realtime);
    (void)snprintf(line, sizeof(line), "realtime: %s (%" PRId64 " ns)\n", status_name(status), realtime);
    PRINT(line);

    XilaFile *file = NULL;
    CHECK(xila_file_open(startup->home, "c-demo.txt", 10,
        XILA_OPEN_READ | XILA_OPEN_WRITE | XILA_OPEN_CREATE | XILA_OPEN_TRUNCATE, &file));
    static const char contents[] = "A file written through Xila's C ABI.\n";
    status = write_all(file, contents, sizeof(contents) - 1);
    if (status == XILA_STATUS_OK) status = xila_file_flush(file);
    if (status == XILA_STATUS_OK) status = xila_file_seek(file, 0, XILA_SEEK_START, NULL);
    if (status == XILA_STATUS_OK) {
        for (;;) {
            size_t count = 0;
            status = xila_file_read(file, line, sizeof(line), &count);
            if (status != XILA_STATUS_OK || count == 0) break;
            status = write_all(startup->standard_output, line, count);
            if (status != XILA_STATUS_OK) break;
        }
    }
    XilaStatus closed = xila_file_close(file);
    if (status != XILA_STATUS_OK) return failure(startup, "home file", status);
    CHECK(closed);
    CHECK(xila_file_flush(startup->standard_output));
    return 0;
}
