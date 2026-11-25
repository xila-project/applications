# Wasmi runtime

`wasm` is the Wasmi-based runtime and includes the guest graphics wrappers.
It replaces the original WAMR implementation. Xila's shared `EventCode` enum
defines guest event values independently of LVGL. The host translates native
event variants into that contract. Graphics call IDs and state/style definitions
still follow the bundled LVGL API, so changes to those interfaces may require
rebuilding the host and guest applications together.

Wasmi 2 runs with default features disabled for `no_std` hosts. The runtime and
the translation test harness enable validation and automatic dispatch, which
falls back to portable dispatch when required by the target or optimization
level. Wasmi's memory64 support is only built for the `memory_64` configuration
(`memory_64 = ["wasmi?/memory64"]`); the translation test harness enables it
for both widths because it exercises both.

## Guests and `sudo`

Guests are untrusted code, so they never run with an identity granted by the
Shell's `sudo`. `sudo` launches its command with `Launch::with_identity`, and
Core marks that task and every task it starts as running with a granted
identity (`task::Manager::is_identity_granted`). The mark is set with the
identity and nothing clears it, whatever the environment holds and even once
the command has exited and its descendants have been reparented. The runtime
refuses to start a guest, with exit status 1, in a marked task. This covers the
mounted runner given to `sudo` directly (`sudo wasm GUEST`) and any native
program run by `sudo` that starts the runner. Core separately refuses to launch
a registered file format with another identity.

## WebAssembly features

`src/host/engine.rs` writes the validated feature set out explicitly, so a
Wasmi upgrade cannot change what guests may use. `rustc` (1.82+) and
`clang` (`--target=wasm32-wasip1`, default CPU) both emit exactly bulk memory,
multi-value, mutable globals, non-trapping float-to-int, reference types and
sign extension for this target; the guests of this repository add no
`target-feature` flags.

| Proposal | State | Reason |
|---|---|---|
| Mutable globals, sign extension, non-trapping float-to-int, multi-value, bulk memory, reference types | enabled | Default output of the Rust and C toolchains |
| Tail calls, extended constants | enabled | No cost; opt-in for guest toolchains (`+tail-call`, `-mcpu=lime1`) |
| memory64 | `memory_64` builds only | The host ABI has one address width; other builds reject memory64 modules at validation |
| Multi-memory | disabled | The host ABI and pointer translation address the single exported `memory` |
| SIMD, relaxed SIMD | disabled (not built) | No guest needs it; enabling Wasmi's `simd` feature grows the host by about 420 KiB (+29 %) |
| Custom page sizes | enabled | Bundled guests use 1-byte pages to reduce initial linear memory |
| Wide arithmetic | disabled | No bundled guest uses it |
| Threads and atomics | unsupported by Wasmi | |

Disabling Wasmi's memory64 support in `memory_32` builds saves about 2 KiB.

## Guest linear memory

Every guest built by this repository (`mise run build:applications`, the
`release-wasm` profile, host tests and the release packages all use the same
build script, `executables/wasm_guest_build.rs`) is linked with 1-byte linear
memory pages (`--page-size=1`, the custom page sizes proposal) and a 32 KiB
stack (`-zstack-size=32768`). The initial memory is then exactly the stack
plus the data the module needs, instead of a multiple of 64 KiB.

The stack is placed first in memory, so an overflow stops the guest with an
out-of-bounds memory access trap instead of corrupting its data. The deepest
call chain of the bundled guests is the calculator's: an expression nested to
the parser's limit of 32 levels needs between 16 and 20 KiB of stack, and a
host test evaluates one. The other guests used at most 11.3 KiB in the host
tests (the synthesizer, then 10.3 KiB for the text editor). A guest with a
deeper call chain can set a larger `-zstack-size`.

Third-party guests opt in the same way. For Rust 1.97.0 targeting
`wasm32-wasip1`, pass these linker arguments, for example from a build script
with `cargo:rustc-link-arg-bins`, or with `-C link-arg=...` in the Cargo target
configuration:

```text
--page-size=1
-zstack-size=32768
```

The proposal allows only 1-byte and 65,536-byte pages. Run Binaryen with
`--enable-custom-page-sizes` when optimizing these modules, as
`mise run build:applications` does. WebAssembly's `memory.size` and
`memory.grow` count in the module's page size, and the bundled WASI libc
allocator follows the linked page size. Custom allocators and direct callers of
the memory intrinsics must use the same unit. A runtime that does not implement
the proposal rejects such a module; link with `--page-size=65536` for it.
Validators must enable the proposal too, for example
`wasm-tools validate --features=wasm2,custom-page-sizes` or Wasmparser's
`WasmFeatures::CUSTOM_PAGE_SIZES`.

## WASI Preview 1

All 46 `wasi_snapshot_preview1` functions are provided (plus the Xila-specific
`fd_control`), so any guest that links against the Preview 1 libc loads. Every
path operation resolves beneath its preopened directory using Core's
`*_beneath` VFS operations: `..` and absolute symlink targets are
`ENOTCAPABLE`, and a directory opened by a guest keeps its preopen as its
confinement root.

| Import | Behavior |
|---|---|
| `fd_pread`, `fd_pwrite` | Read or write at an offset without using or moving the cursor; guest buffers are used directly (no allocation) and validated before any byte moves; a failure after progress reports the partial count. Standard streams are `ESPIPE`, directories `EISDIR`, wrong access mode `EBADF` |
| `fd_tell` | Current cursor; `ESPIPE` for standard streams |
| `fd_sync`, `fd_datasync` | Flush the file (a directory has nothing to flush) |
| `fd_advise` | Validates the advice (`EINVAL` outside 0–5) and succeeds: there is no page cache |
| `fd_allocate` | Like `posix_fallocate`: grows to `offset + len`, never shrinks, `len == 0` is `EINVAL`, overflow `EFBIG`, read-only `EBADF` |
| `fd_filestat_get`, `path_filestat_get` | `ctim` reports the last status change time (`Statistics::status`), not the creation time. `dev` is 0: the Core `Statistics` carries no device or mount identifier yet (gap G14 in the review on xila-project/core#835), so `(dev, ino)` does not identify a file across mounts until a Core change adds the identifier |
| `fd_filestat_set_size` | Truncates or zero-extends a writable file |
| `fd_filestat_set_times`, `path_filestat_set_times` | `ATIM`/`MTIM`/`*_NOW` with the Preview 1 invalid combinations as `EINVAL`; `lookupflags` `SYMLINK_FOLLOW`; Xila stores whole seconds, so nanoseconds are truncated; `*_NOW` uses the realtime clock |
| `fd_fdstat_set_flags` | Replaces the status flags: append and nonblocking are honored, `DSYNC`/`RSYNC`/`SYNC` are `ENOTSUP`, unknown bits `EINVAL` |
| `fd_fdstat_set_rights` | See Rights below |
| `fd_renumber` | Atomically moves `fd` onto `to` and closes the old `to`. Both must be open (`EBADF`): the environment offers no `dup2`. Rights, the preopen name and the confinement root move with the descriptor |
| `path_rename` | `rename_beneath` across two directory descriptors; renaming the base directory is `EBUSY`; a directory beneath itself is `EINVAL`; across mounts `EXDEV` |
| `path_remove_directory` | Only empty directories (`ENOTEMPTY`, `ENOTDIR` for files and links, `EINVAL` for `.`) |
| `path_link` | Hard links: neither the VFS backends nor LittleFS have them, so after confinement and existence checks a valid request is `ENOTSUP` (a directory source is `EPERM`, an existing target `EEXIST`, other mounts `EXDEV`) |
| `proc_raise` | Default dispositions, since Preview 1 cannot install handlers: `chld`, `cont`, `urg`, `winch` return; `stop`/`tstp`/`ttin`/`ttou` are `ENOTSUP` (no job control); every other signal ends the guest like `proc_exit(128 + signal)`; numbers outside 1–30 are `EINVAL` |
| `sock_accept`, `sock_recv`, `sock_send`, `sock_shutdown` | Preview 1 only operates on pre-opened sockets and Xila pre-opens none: `EBADF` for a closed descriptor, `ENOTSOCK` for any open one. Wiring a socket variant to Core's network subsystem is future work |

### Rights

Descriptors hold every right unless a reduction was made, either by the guest
(`fd_fdstat_set_rights`, which can only remove rights) or by `path_open`
through a directory whose inheriting rights are reduced (the requested rights
must then be a subset, else `ENOTCAPABLE`). Reductions are recorded per
descriptor only when made, reported by `fd_fdstat_get` and enforced by every
descriptor and path function. Rights requested through an unreduced directory
are not recorded and only select the access mode. Rights are not the
confinement mechanism: the preopen and Xila's permissions are.

`--instruction-limit` supplies the fuel budget for each cooperative execution
slice. Wasmi 2 meters input Wasm operators instead of its internal bytecode, so
the same budget may yield at different intervals than with Wasmi 1. A bulk
operation can require more fuel than one normal slice; the runtime replenishes
at least the amount required to resume it.

### Translation

`--translation eager` (the default) translates every function to Wasmi's
internal code while the module loads; `--translation lazy` translates each
function at its first call, which lowers the load peak and the resident size
of guests that never call most of their code, at the cost of a pause at each
first call. Building with the `lazy_translation` feature makes lazy the
default, so a target where the peak matters most can opt in. Either way the
whole module is validated before it is instantiated.

Wasmi charges lazy translation to the running guest's fuel, and in Wasmi 2.0
running out of fuel while translating a callee is a plain error, not a
resumable pause ([Wasmi PR #2055](https://github.com/wasmi-labs/wasmi/pull/2055)).
The engine therefore charges no fuel for translation or validation, so a slice
can never end inside one. Translating one function does not yield to the
executor, but it is bounded: a function body is never larger than the module,
which is read whole into memory, and each function is translated at most once.
All lazy pauses together never exceed the work eager translation does in one
go while loading, so a guest cannot hold the executor any longer than eager
loading already allows.

Wasmi 2 has no streaming module constructor (`Module::new_streaming` was
removed in Wasmi 1.0), so the source is still read whole and released as soon
as the module is built.

## Test Calculator

From the Core repository root on Linux, set `XILA_APPLICATIONS_MANIFEST` to the pinned Applications checkout’s `Cargo.toml`:

```sh
rustup target add --toolchain 1.97.0 wasm32-wasip1
export XILA_APPLICATIONS_MANIFEST=/absolute/path/to/applications/Cargo.toml

# Headless acceptance: builds the real guest, enters 1 + 2 = through LVGL
# button events, checks the displayed result, requests close, checks cleanup.
cargo +1.97.0 test -p xila-wasm --features graphics \
  --target x86_64-unknown-linux-gnu --test calculator

# Interactive native display. Click calculator buttons; press Enter in the
# launching terminal to request close and let the test finish successfully.
cd /absolute/path/to/applications
cargo +1.97.0 test --manifest-path executables/calculator/Cargo.toml \
  --target x86_64-unknown-linux-gnu --test calculator_test \
  -- --ignored --nocapture
```

The interactive test requires a working native graphical session. Its terminal
close path sends Xila's window-close event, rather than abruptly killing the VM.
The guest handles that event as a numeric code, not an invalid Rust enum variant.

Other focused tests:

```sh
cargo +1.97.0 test -p xila-graphics-bindgen --target x86_64-unknown-linux-gnu
cargo +1.97.0 test -p xila-wasm --features graphics --target x86_64-unknown-linux-gnu
cargo +1.97.0 test -p xila-wasm --no-default-features \
  --features host,graphics,memory_64 --target x86_64-unknown-linux-gnu
```

Calculator is a wasm32 guest, so its acceptance test is not run in the memory64
configuration. Core/adapter tests cover both guest-word widths. Portable memory
tests, 90% line and function coverage commands, and Miri commands are documented in
[`tests/translation/README.md`](tests/translation/README.md).

## Graphics architecture

Pixel Painter uses the reusable indexed-canvas helpers. `indexed_canvas_create`
allocates a host-owned image; `indexed_canvas_set_pixels` validates and copies
palette indexes and RGB888 palette entries from guest memory. The per-VM image
budget is 65,536 pixels. Buffers remain alive through native object deletion and
are reclaimed afterward or at VM teardown.

`window_pop_pointer_event` extends event polling with event-time x/y coordinates
relative to the target content area and its width/height. For events without
pointer data it writes `i32::MIN` to x/y and zero to width/height. The original
`window_pop_event` contract is unchanged. Textarea copying, keyboard attachment,
padding, and flag removal also have checked adapters. New Xila helpers have
append-only IDs starting at `0xf000`, preserving existing call numbering.

Run the headless Wasm app acceptance and ABI boundary tests with:

```sh
cargo test -p xila-wasm --features graphics,xila/graphics_rendering_xrgb8888 \
  --test graphics_canvas --test pixel_painter
```

- `subsystems/graphics/bindings_generator` discovers and orders one shared LVGL ABI
  catalog; unsupported calls keep their IDs. The supported-call declaration
  (`graphics_bindgen::abi`) checks full C signatures, not just argument counts,
  and generates both the guest bindings and Core's host call table. The catalog
  is compatible with guest bindings built from the same LVGL revision/configuration,
  not a stable cross-version ABI.
- Core's graphics executor (`graphics::call`, in `subsystems/graphics`) runs every
  graphics call: it owns the graphics lock, each VM's object table, the event
  queue and the extension calls. `src/host/graphics/mod.rs` is the Wasm transport
  over it: it presents the guest's linear memory as a `LinearMemory` with the
  guest's 32- or 64-bit word. It uses no Wasmi types and no global
  graphics-manager lookups; the thin `graphics/wasmi.rs` adapter alone handles
  imports, exports, memory/store borrowing, and context capability checks.
- Object handles (Core's `graphics::call::State`) are 32-bit `lvgl-rust` generational handles (16-bit slot +
  16-bit generation, 65,536 reuse generations per slot) tracked in a per-VM
  sorted `Vec<u32>` handle table. Every guest handle is verified by binary
  search in the calling VM's table before resolving it in `lvgl-rust`'s
  registry, rejecting forged, stale, or cross-VM handles with
  `GraphicsError::InvalidHandle`. There is no 15-VM cap; live VMs are
  registered dynamically in a growable host table of `Side` pointers with no
  static reservation. Guest handle typedefs (`ObjectHandle`, `LvObject`) and
  `object_delete(u32)` are widened to 32 bits while preserving all call IDs.
- One global `set_free_observer` removes freed handles from the owning VM's
  table and reclaims retained side-table entries and quota counters immediately
  when LVGL finishes object destruction (including cascading and external
  deletes), without per-object `DELETE` callbacks or `Rc` wrappers. VM teardown
  deletes the VM's still-live objects under the graphics manager lock (skipping
  already-dead children and ignoring refused structural deletions) and
  unregisters its `Side` before freeing it, including on traps. Registration
  clears matching pre-existing queued targets to null before issuing a new
  handle, so native address reuse cannot grant an old unknown event a new
  capability. Event codes are preserved.
- Text setters validate NUL termination (and UTF-8 where required) directly in
  guest memory and pass pointers to LVGL's synchronous copying setters with
  zero host allocations. Button maps (one packed allocation for pointer table
  plus strings), grid tracks, calendar highlights, line points, chart series,
  indexed canvases (one packed allocation), and alias kinds live in a sparse
  per-VM side table keyed by raw handle; plain widgets add zero host
  side-table entries. A map or grid replacement switches LVGL to the new
  buffer before releasing the old one.

Enable the optional `graphics` feature for host bindings. Select the Xila
rendering feature appropriate to the platform (e.g.
`xila/graphics_rendering_xrgb8888` for a standalone host library check). Normal
non-graphical runtime use does not enable graphics.

Guest-only consumers must disable defaults to avoid enabling the host runtime.
Set `default-features = false` on the workspace dependency before inheriting
it in guest members:

```toml
wasm = { workspace = true, default-features = false, features = ["guest", "graphics", "memory_32"] }
```

`default_guest` additionally enables C wrappers and emits `xila_graphics.h` in
the build output directory. No build step rewrites the guest source files.
Weather's guest and host-test wiring compile against this crate, but graphical
execution still requires APIs beyond the Calculator subset listed below.

### Supported calls and limits

This first milestone supports windows (creation, icon, event pop), object
creation/deletion, position/size and scalar geometry queries, alignment,
layout update, built-in default/flex/grid layout selection, flex flow/alignment/
grow, width/height, flags, validated Grid tracks/placement/alignment, local
background/text/border/radius/opacity/text-alignment/min-max styling, semantic
theme surfaces and host font roles, labels (creation/text), button matrices
(creation/map/selection), range/value/mode/orientation for Arc/Bar/Slider,
Switch orientation/state, Spinner animation parameters/duration/sweep, Dropdown
and Roller with bounded copied options, selection and selected-text queries, and
percentage conversion. Calendar supports validated today/shown dates, bounded
copied highlights, and shown-date output. Spinbox supports bounded range/value/
step/digit-format operations, rollover, increment, and decrement. Most other
widget-specific query/configuration APIs and advanced style properties remain
unsupported. Chart supports bounded series slots and copied values; Line accepts
bounded points copied into host-owned memory because LVGL retains the point
array pointer. List supports copied text sections/buttons with safe child
handles, and Tileview supports bounded tile creation and active-tile navigation.
List text is UTF-8 validated in guest memory and passed to LVGL's copying
calls without a host allocation; the guest never passes icon, tile, or series
pointers to LVGL.
The shadcn-themed slice adds Badge and Card helpers, Button variants, and Input
error states by calling the shared `lvgl_theme_shadcn` theme APIs.

- No fixed concurrent graphical VM limit (dynamic per-VM registry and sorted
  handle tables); object slots are dynamically grown in non-moving doubling
  chunks up to 65,535 live objects and reused across 65,536 generations after
  deletion.
- 64 map entries including the sentinel; 1024 bytes per NUL-terminated string.
- 8192 retained map-string bytes per widget, including the native sentinel.
- 1024 option-string bytes/64 entries per Dropdown or Roller, with an 8192-byte
  per-VM retained high-water budget shared with Calendar highlights, Line points,
  and List text. Calendar accepts at most 32 dates; Roller infinite mode has a
  512-byte input cap to bound LVGL's repeated internal option storage.
- Spinbox values are limited to ±1,000,000, step to 1..=1,000,000, and digit
  count to 1..=10. Calendar dates use Gregorian validation.
- Chart allows up to 8 series and 128 values per series; Line allows up to 128
  points. List allows 64 child items and Tileview 32 live tiles per view; tile
  coordinates must be unique within a view.
- Theme variants are 0..=4, surfaces are 0..=3, and error flags are boolean;
  palette updates remain owned by the existing display theme integration.
- Null and empty-string guest map sentinels both become LVGL's empty-string
  terminator. Row separators are preserved.

Return status zero means success. Errors are nonzero: invalid count 1, invalid
memory 2, unavailable capability 3, unsupported call 4, invalid handle 5,
resource limit 6, invalid value 7, native failure 8.

Calls requiring arbitrary native structure layouts, guest callbacks, retained
guest pointers, raw font/image/chart handles, or unsupported LVGL APIs return unsupported.
No typed host reference is formed from guest memory.

## Scheduling and remaining limitations

WASI `poll_oneoff` supports up to 64 realtime/startup-clock subscriptions,
relative and absolute deadlines. It validates explicit little-endian records
and outputs, suspends the VM, asynchronously waits, then reacquires memory and
resumes with ready events. FD polling and unsupported clocks return errors.
The startup-clock implementation follows the existing Xila time manager; it is
not an independent hardware monotonic-clock source.

Fuel exhaustion and `sched_yield` yield to the executor. Clock polling uses
resumable Wasmi host calls; suspension is VM-local. WebAssembly start-section
suspension is not supported; WASI command guests should use exported `_start`.
The former `--install` option and the guest `__install` export are removed:
the host rejects `--install` and never runs an `__install` export, so installing
or discovering an application never executes guest code. Applications supply
their launcher metadata in a bundle-local `shortcut.json`.
The command-line stack-size option remains unimplemented.

The current graphics ABI audit and staged delivery roadmap are tracked in
[`GRAPHICS_ABI.md`](GRAPHICS_ABI.md). In particular,
the generator currently emits more guest wrappers than the host implements;
unsupported calls return status 4 and should not be treated as available APIs.

Static audit of the current LVGL configuration: 28 surveyed widget families
are enabled, while the guest generator emits 1,074 LVGL/Xila wrappers and the
host supports 92 calls (including Xila helpers). Seven enabled widget families
have creation adapters, but several remain partial. This is not a runtime or
performance benchmark. The next ABI tranches are a shared host/guest support
manifest and inventory, semantic style/font APIs, then the BTreeMap handle
registry and typed bounded events; see issue #483 for scope and acceptance.

The existing stdin fixture passes with a line of input. Running it with closed
stdin (`/dev/null`) exposes an unrelated `drivers_std::StandardInDevice` bug:
it discards the actual read count and reports the buffer length even at EOF.

Host tests and Miri do not establish embedded hardware correctness or firmware
RAM/flash costs. Native LVGL integration tests are separate from the portable
translation coverage gate; no full LVGL/WASI compatibility is claimed.

## Cooperative execution control

Host callers can retain an `Arc<ExecutionControl>` and pass a clone to
`execute_with_control`. `request_stop()` stops only that execution and cannot be
cleared. Join the execution future before replacing its module or releasing its
storage/graphics. Fuel slices, yield and clock-poll waits observe the token and
termination or interrupt signals sent to the guest's task, which is how the OS
lifecycle stops guests during a shutdown. Guests cannot request a shutdown: the
lifecycle request API is host-only. The legacy process-wide signal broadcast is
retained for native runners; it is not the per-guest control API. Host calls that
block synchronously cannot be preempted, and a timeout must retain their resources.

## Executable integrity

An executable inside `/applications/bundles/<package>/executables/` is a
**managed** package executable. Before compiling it, the runner reads the
bundle's installed `.xila/manifest.json` (written and sealed by the
installer after verifying the package) and hashes the executable while
reading it into the buffer that is compiled, using
`xila-package-manifest` and `xila-package-verification::ContentVerifier`. It
refuses the executable, with a `wasm: refusing to run <path>: <reason>`
message, when the manifest is missing, invalid, describes another package or
does not declare the file; when the size or SHA-256 differs; or when a bundle
directory, the manifest or the executable is a link or is writable by a
non-root user. Paths with empty, `.` or `..` components are refused.

Any other path is an **unmanaged** development executable with no
authenticated digest. It runs only with the explicit `--developer-mode`
option (off by default), unverified and with the caller's permissions:

```text
wasm --developer-mode /temporary/application.wasm
```

`--developer-mode` never relaxes the checks on managed executables. This is
load-time validation, once per load; it is not a runtime integrity guarantee
such as fs-verity.
