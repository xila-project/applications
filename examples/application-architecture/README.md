# Application cores and frontends

A **user-facing application** provides a feature to a person. Its **application
core** is the reusable state and behavior of that feature. A **frontend** adapts
user input and presents results using a particular interface. Xila's
`ExecutableTrait`, by contrast, is the operating-system interface for starting
and running an executable: it is an entry point a frontend may use, not an
application architecture. A Wasm guest can have its own entry point and adapt
the same kind of core to the guest API.

This is a recommendation for applications that benefit from shared behavior,
not a required abstraction. A short command such as `date` or `copy` can use a
function and existing I/O directly; it need not have messages, a view model, or
another crate. A small single-frontend application may start with private modules
and extract a core crate only when a second frontend or independent tests justify
it.

## Choosing a structure

Model–View–Update (MVU) fits stateful Xila interfaces: the frontend translates an
event into a message, the core updates private state, and the frontend renders
view data. This makes transitions testable without LVGL or an executor. It does
not require immutable state, a message queue, a heap, or a framework. For simple
operations, direct core methods may be clearer than a message enum. MVC can also
separate responsibilities, but a controller coupled to a particular widget tree
is harder to reuse in a command-line frontend. Either approach works when the
dependency boundary and ownership are clear.

When reuse warrants separate crates, the dependency direction is:

```text
GUI frontend ──┐
               ├──> application core
CLI frontend ──┘
```

Keep the core free of LVGL, terminal, host runtime, guest runtime and platform
drivers. It can use `core`, and `alloc` or domain libraries if actually needed.
The frontend owns event loops, formatting, localization, storage, clocks and
startup. For asynchronous I/O, a core can accept data supplied by the frontend
and return a small typed request or result; the frontend performs effects and
feeds their results back. Do not hold a graphics lock across I/O. Define error,
cancellation and persistence behavior for the application rather than assuming
that an update is always synchronous or infallible.

Cargo prevents a core from importing a frontend crate it does not depend on;
private fields prevent external callers from mutating state directly; a public
message enum and update method define which transitions clients can request.
These do not prevent someone adding a dependency in the future or duplicating
business logic in a frontend: dependency review, tests and code review remain
necessary. Avoid dependency cycles. A core shared by native and Wasm must itself
compile for each target; sharing a Rust API does not imply a stable Wasm or C ABI.

The optional `xila-application` library provides `Application` for typed
updates, `Frontend` for input translation and rendering, and `dispatch` to run
those steps in order. It uses static dispatch, has no dependencies or allocator,
and leaves scheduling and platform effects to the application. An input can be
ignored without repainting. Update errors stop rendering; a rendering error
does not undo an already successful update. Use `render` for the initial view.
For batched event loops, an application may use `Application` without
`dispatch` and render at its own cadence, as Clock's stopwatch does.
Stateless or single-interface programs can continue using ordinary functions.

## Example

The independent workspace in this directory has three example crates and uses
the local `xila-application` library:

- `core`: a `no_std`, fixed-size counter with a private value, typed messages,
  a bounded update and a read-only value; it implements `Application`.
- `gui`: a `no_std` **adapter skeleton** mapping buttons to messages and returning
  view data through the shared dispatcher. It intentionally does not create an LVGL window: a real graphical
  frontend must map LVGL events, render the returned value, and own window
  lifetime and error handling. Its tests exercise that mapping.
- `cli`: a `no_std` command adapter with a small native `std` executable. It
  maps command words to the same messages, rejects unknown commands without
  mutation, and displays the result. A Xila shell frontend would adapt the same
  library to shell arguments and output instead of using `std::env`.

Run from the repository root:

```sh
cargo test --locked --manifest-path examples/application-architecture/Cargo.toml --workspace
cargo run --locked --manifest-path examples/application-architecture/Cargo.toml -p example-counter-cli -- add add subtract
# prints 1
cargo check --locked --manifest-path examples/application-architecture/Cargo.toml --workspace --lib --target wasm32-wasip1
```

The checks cover the shared framework, core and both adapter libraries for WASI
and browser-style WebAssembly (`wasm32-unknown-unknown`). CI also checks the
CLI executable for WASI and exercises both adapters against the same core on a
native host. Platform adapters remain responsible for supplying input and
presenting output: the example GUI does not create a window, and the example
CLI is a standalone process rather than a Xila shell command. Clock provides a
real Wasm graphical frontend using the same application update contract.
Neither the framework nor the core depends on the target, an allocator, a
particular executor, or a GUI or CLI runtime. Native graphical and CLI programs
can use the same contracts with their existing Xila event and I/O adapters.

## Existing applications and migration cost

- **Calculator:** `executables/calculator/src/engine.rs` evaluates expressions,
  and `interface.rs` maps its graphical controls and maintains expression/UI
  state. Its engine is currently a private module of the binary, and localizes
  errors itself. A shared CLI would first move domain evaluation and typed
  errors into an independent core crate and let each frontend localize/present
  errors. It must preserve expression semantics and localization IDs. Until
  then, tests of the module do not make it a separate Cargo boundary.
- **Clock:** `executables/clock/src/lib.rs` contains bounded stopwatch, timer
  and alarm behavior used by the Wasm graphical interface. Its library is a
  useful starting point, though the package still has application metadata and
  translation dependencies, and storage is gated to tests or Wasm. A native or
  CLI frontend can reuse the model with explicit time inputs; storage and time
  adapters need separate treatment before claiming full runtime-independent
  reuse.
- **Command line:** shell commands such as `word_count` separate counting from
  file I/O within a module. If a graphical word-count application is needed, the
  counting state and byte-processing logic could become a core and each frontend
  handle its own stream and presentation. A standalone shell command does not
  need that extraction merely to conform.

The shared library adds no external dependency, allocation, background task or runtime
indirection. Its core is a single `u8`; each adapter uses static calls and
returns a value. Real applications still pay for their chosen data, effects,
rendering, code size and any core dependencies. Separate crates can increase
build and maintenance work, and generics may increase code size through
monomorphization. Measure on representative MCU and Wasm builds before claiming
savings; this example is a structure demonstration, not a benchmark or a broad
migration mandate.
