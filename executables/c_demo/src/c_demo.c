/* A linked application: startup capabilities are borrowed, opened files owned.
   Freestanding C11: no host libc, only the headers application.h already needs. */
#include "application.h"
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

static size_t c_strlen(const char *text) {
    size_t length = 0;
    while (text[length] != '\0') ++length;
    return length;
}

/* Append a NUL-terminated string, stopping at `limit` (which leaves room for a
   final NUL). Returns the new end. */
static char *append_str(char *cursor, char *limit, const char *text) {
    while (*text != '\0' && cursor < limit) *cursor++ = *text++;
    return cursor;
}

/* Append the decimal digits of `value`. Returns the new end. */
static char *format_u64(char *cursor, char *limit, uint64_t value) {
    char digits[20];
    size_t count = 0;
    do {
        digits[count++] = (char)('0' + (value % UINT64_C(10)));
        value /= UINT64_C(10);
    } while (value != 0);
    while (count != 0 && cursor < limit) *cursor++ = digits[--count];
    return cursor;
}

/* Append the decimal digits of `value`, with a leading '-' when negative.
   Returns the new end. */
static char *format_i64(char *cursor, char *limit, int64_t value) {
    uint64_t magnitude;
    if (value < 0) {
        cursor = append_str(cursor, limit, "-");
        /* Negate in two steps so INT64_MIN does not overflow. */
        magnitude = UINT64_C(1) + (uint64_t)(-(value + 1));
    } else {
        magnitude = (uint64_t)value;
    }
    return format_u64(cursor, limit, magnitude);
}

/* Append `value` as exactly eight lowercase hexadecimal digits, like "%08x".
   Returns the new end. */
static char *format_hex32(char *cursor, char *limit, uint32_t value) {
    static const char digits[] = "0123456789abcdef";
    for (int shift = 28; shift >= 0; shift -= 4) {
        if (cursor < limit) *cursor++ = digits[(value >> (unsigned)shift) & UINT32_C(0xf)];
    }
    return cursor;
}

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
        default: {
            char *end = append_str(hex, hex + 11, "0x");
            end = format_hex32(end, hex + 11, (uint32_t)status);
            *end = '\0';
            return hex;
        }
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
    (void)write_all(api, error, operation, c_strlen(operation));
    (void)write_all(api, error, ": ", 2);
    char hex[11];
    const char *name = status_name(status, hex);
    (void)write_all(api, error, name, c_strlen(name));
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
#define PRINT(text) CHECK(write_all(api, startup->standard_output, text, c_strlen(text)))
#define PRINT_LINE(buffer, end) CHECK(write_all(api, startup->standard_output, buffer, \
    (size_t)((end) - (buffer))))
    PRINT("Hello from native C!\n");
    for (size_t i = 0; i < startup->argument_count; ++i) {
        PRINT("argument: ");
        CHECK(write_all(api, startup->standard_output, startup->arguments[i].data,
                        startup->arguments[i].length));
        PRINT("\n");
    }
    char line[128];
    char hex[11];
    char *limit = line + sizeof(line) - 1;
    char *cursor;
    uint32_t version = api->version;
    cursor = append_str(line, limit, "ABI: ");
    cursor = format_u64(cursor, limit, version >> 16);
    cursor = append_str(cursor, limit, ".");
    cursor = format_u64(cursor, limit, version & UINT32_C(65535));
    cursor = append_str(cursor, limit, "\n");
    PRINT_LINE(line, cursor);
    uint64_t monotonic = 0;
    int64_t realtime = 0;
    status = api->time_monotonic(&monotonic);
    cursor = append_str(line, limit, "monotonic: ");
    cursor = append_str(cursor, limit, status_name(status, hex));
    cursor = append_str(cursor, limit, " (");
    cursor = format_u64(cursor, limit, monotonic);
    cursor = append_str(cursor, limit, " ns)\n");
    PRINT_LINE(line, cursor);
    status = api->time_realtime(&realtime);
    cursor = append_str(line, limit, "realtime: ");
    cursor = append_str(cursor, limit, status_name(status, hex));
    cursor = append_str(cursor, limit, " (");
    cursor = format_i64(cursor, limit, realtime);
    cursor = append_str(cursor, limit, " ns)\n");
    PRINT_LINE(line, cursor);

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
