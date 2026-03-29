# ADACS Job Controller Client: Rust Port State

## 1. Project Objective
The goal is a 1:1 functional port of the C++ ADACS Job Controller Client to Rust. This client is responsible for managing "bundles" (Python-based job plugins), executing them in isolated sub-interpreters, and communicating with a central server via a custom binary protocol over WebSockets.

The port must maintain exact parity with the original C++ test suite (67 test cases) and preserve the "unhinged" architectural requirements: dynamic loading of `libpython`, binary patching/FFI manipulation of the GIL, and deep sub-interpreter integration.

---

## 2. Completed Implementations

### A. Python FFI & Runtime Bridge (`python_interface.rs`)
- **Dynamic Loading:** Successfully implemented dynamic loading of `libpython3.11.so` using the `libloading` crate. This avoids linking against Python at compile time, allowing the binary to run on systems with different Python minor versions.
- **Symbol Wrapping:** A custom `py_wrap!` macro proxies nearly 40 Python C-API functions (e.g., `Py_Initialize`, `Py_NewInterpreter`, `PyObject_CallObject`) through the dynamically loaded library.
- **Thread Safety:** Implemented `ThreadStatePtr` wrappers and `OnceLock` to safely store the Python `Library` and the main `PyThreadState` across threads.
- **GIL Management:** Ported the logic to acquire/release the Global Interpreter Lock (GIL) and manage thread states, which is significantly more complex in Rust due to Tokio's multi-threaded executor stealing tasks across OS threads.

### B. Bundle Management & Isolation (`bundle_manager.rs`, `bundle_interface.rs`)
- **Isolation:** Implemented `BundleInterface` which creates a dedicated `PyInterpreterState` for every plugin via `Py_NewInterpreter`. This ensures that global variables in one plugin do not leak into another.
- **JSON Serialization Bridge:** Built a bridge that converts Rust `serde_json::Value` objects into Python `dict` objects (and vice-versa) using the Python `json` module inside the sub-interpreter. This is used for passing "job details" into plugins.
- **Registry:** `BundleManager` maintains a registry of loaded bundles, caching the sub-interpreters to avoid the high overhead of recreation.

### C. The Logging Interceptor (`bundle_logging.rs`)
- **Stdout/Stderr Redirection:** Successfully implemented a custom Python module `_bundlelogging` written in Rust.
- **Interception:** When a Python bundle calls `print()`, the output is routed through our Rust `write` implementation, which captures the data into thread-local buffers.
- **Log Levels:** Implemented logic to prefix lines with `[STDOUT]` or `[STDERR]` and route them to the main application's logging framework.

### D. Messaging & Protocol (`messaging.rs`)
- **Binary Protocol:** Fully ported the Little-Endian binary protocol.
- **Serialization:** Implemented `push/pop` methods for `uint`, `ulong`, `string`, `bool`, and `bytes`.
- **Command Constants:** Mirrored all 100+ command IDs (e.g., `SUBMIT_JOB`, `FILE_LIST`) from the C++ source.

### E. WebSocket Infrastructure (`websocket.rs`)
- **Async Client:** Implemented using `tokio-tungstenite`.
- **Priority Queuing:** Ported the 20-level priority queuing system. Messages with higher priority (e.g., `Highest`) are always sent before lower-priority ones.
- **Sync-Async Bridge:** Implemented a complex bridge allowing synchronous Python code to trigger asynchronous WebSocket requests (like database lookups) and block until the result arrives.

### F. Database Layer (`db/mod.rs`)
- **ORM:** Used `SeaORM` with `Sqlite` to mirror the job schema.
- **Entities:** Ported `Job` and `JobStatus` models.

---

## 3. Current Problems and Blockers

### A. The "GIL is released" / Segfault Nightmare
The most significant blocker is the interaction between **Python Sub-interpreters** and **Tokio's Multi-threading**.
- **Problem:** Python's C-API relies heavily on thread-local storage (TLS) to find the current `PyThreadState`.
- **Symptom:** When a Tokio task moves from Thread A to Thread B, or when we call a Python function from a spawned thread, the Python C-API finds a `NULL` thread state and triggers a `Fatal Python error: PyThreadState_Get: the function must be called with the GIL held`.
- **Attempted Fix:** We are currently using a global `PYTHON_MUTEX` to serialize all Python access and attempting to manually manage `PyEval_RestoreThread` and `PyEval_SaveThread` inside the `BundleInterface`. However, `Py_NewInterpreter` and `Py_EndInterpreter` behave inconsistently when called from threads that weren't the one that initialized Python.

### B. Mocking & Singleton Inconsistency
- **Problem:** Tests require a `MockWebsocketClient` to verify outgoing messages. Because `get_websocket_client()` returns a singleton, tests running in sequence often inherit "dirty" state or missing expectations from previous tests.
- **Attempted Fix:** Refactored `websocket.rs` to use an `Arc<RwLock<Option<Arc<dyn WebsocketClient>>>>` to allow tests to "inject" a fresh mock.

### C. Protocol Indexing Desync
- **Problem:** Several tests were failing because they created a `Message`, pushed data, and then passed it to a handler. The handler would try to "pop" data, but the message's internal read-index was already at the end.
- **Fix:** We are refactoring tests to use `Message::from_data(msg.get_data().clone())` to reset the read-pointer.

---

## 4. Immediate Next Steps
1. **Stabilize Python Lifecycle:** We must find the definitive way to make `Py_NewInterpreter` work across Tokio worker threads. This likely involves ensuring every entry point into Python from Rust explicitly acquires the GIL and sets up a `PyThreadState` if one doesn't exist for that OS thread.
2. **Sequential Test Execution:** Python's global initialization is NOT thread-safe. We are strictly using `--test-threads=1` to prevent concurrent `Py_Initialize` calls from different tests.
3. **Full Parity Verification:** Once the GIL crashes are resolved, we need to verify the remaining 28 test cases (mostly involving large file streaming and complex job cancellation flows).

Current status: **39/67 tests implemented.** The core logic is ported, but the FFI "glue" is currently brittle under the stress of the full test suite.
