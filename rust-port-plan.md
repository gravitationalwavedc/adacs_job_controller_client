# ADACS Job Controller Client - Rust Port Plan

This document outlines the architecture and execution plan for porting the C++ ADACS Job Controller Client to Rust, while preserving its unique dynamic library loading, binary patching, and sub-interpreter "bundle" architecture.

## Phase 1: Core Scaffolding & FFI (The "Crapola")
**Objective:** Replicate the C++ `PythonInterface` dynamic linking, `subhook` patching, and thread state management.
*   **Initialize Cargo Project:** Create `rust/Cargo.toml` with dependencies (`libloading`, `libc`, `cc`, `bindgen`, etc.).
*   **Compile Subhook:** Create a `build.rs` script to compile the `src/third_party/mempatch/subhook` C files into the Rust binary.
*   **Dynamic Loading (`python_interface.rs`):** Replicate the `PYWRAP_*` macros using `libloading` to fetch Python C-API function pointers from the loaded `.so` at runtime.
*   **GIL Hooking:** Define the dummy `myPyGILState_Ensure` in Rust (`#[no_mangle] extern "C"`) and apply the subhook during `initPython`.
*   **Sub-Interpreters & RAII:** Create `ThreadState` and `SwapThreadStateScope` structs that implement `Drop` to ensure safe cleanup of Python thread states.

## Phase 2: Bundle Management & Global State Fixes
**Objective:** Port the plugin architecture while fixing the C++ data races.
*   **Logging Data Race Fix (`bundle_logging.rs`):** Implement `_bundlelogging` Python module in Rust. Replace the global `std::vector<std::string> lineParts` with a `thread_local!` `RefCell<Vec<String>>` to prevent concurrent modification panics.
*   **Bundle Manager (`bundle_manager.rs`):** Replicate the thread-safe loading of bundle hashes using a `RwLock<HashMap>`. Implement the mutex to prevent concurrent threads from executing the *same* bundle hash simultaneously.
*   **Stdout Redirection:** Translate the embedded Python redirection script to call into the Rust-backed `_bundlelogging` module.

## Phase 3: Domain Models & Database (SeaORM)
**Objective:** Port the domain models (`sJob`, `sStatus`) using SeaORM.
*   **Entity Generation:** Define `Job` and `JobStatus` entities mapping to the `jobclient_job` and `jobclient_jobstatus` schema.
*   **SQLite Setup:** Create a `Database` connection pool and initialization routine to automatically run migrations or generate the schema.
*   **Bundle DB Module (`bundle_db.rs`):** Replicate the `_bundledb` Python module exposed to the bundles for querying/updating jobs. Use `serde_json` to translate `PyObject` to/from Rust structs.

## Phase 4: Job Handling & Core Business Logic
**Objective:** Port the business operations (Submit, Status, Cancel, Delete, Archive).
*   **Trait-Based DI:** Define traits for the WebSocket client and Bundle Manager to enable mocking during tests (using `mockall`).
*   **Job Routines:** Translate `checkAllJobsStatus`, `submitJob`, etc. into Rust `async` functions powered by Tokio.
*   **Files & Archive:** Translate the file system operations (tarring archives, file lists).

## Phase 5: Testing
**Objective:** Replicate and enhance the C++ testing suite.
*   **Test Fixtures:** Port the `BundleFixture` to write temporary python scripts.
*   **Logging Tests:** Translate `bundle_logging_tests.cpp` to verify thread-safe stdout/stderr captures.
*   **Job Tests:** Translate the status, submit, and cancel tests using the mocked WebSockets and real Bundle DB logic.

## Execution Strategy
Due to the sheer size of the application, I will execute this plan sequentially, starting with the most complex and high-risk part (Phase 1 & 2), followed by the database integration, and then the async job handlers.