# ADACS Job Controller Client - Architecture Documentation

## Overview

The ADACS Job Controller Client is a Rust port of the original C++ implementation that manages the lifecycle of computational jobs submitted to distributed computing clusters. It handles job submission, status tracking, cancellation, file operations, and execution of bundle functions (Python code isolated in separate interpreter instances).

**Key Responsibilities:**
- Receive job submissions via WebSocket from external clients
- Execute bundle functions (Python code) for job operations (submit, status, cancel, delete)
- Manage job state in SQLite database
- Handle file I/O operations for job working directories
- Maintain persistent WebSocket connection for bidirectional communication

**Why Rust?**
- Memory safety without garbage collection (zero-cost abstractions)
- Fearless concurrency (type system prevents data races)
- FFI safety (careful control over unsafe boundaries)
- Strong ecosystem (tokio, sea-orm, serde, parking_lot)

---

## Core Module Architecture

### `main.rs`
Entry point and daemon mode implementation. Handles:
- CLI argument parsing
- Daemon mode (`--daemonize`, `--log-dir`)
- UNIX double-fork daemonization (process lifecycle management)
- Connection initialization (Python, database, WebSocket)
- Event loop setup (tokio runtime, async task spawning)

**Key Functions:**
- `daemonize()`, `daemonize_with_log_redirect()` - UNIX daemon pattern

### `bundle_manager.rs` - Singleton Bundle Cache
Manages loading and caching of `BundleInterface` instances (one per unique bundle hash).

**Design:**
- **Production**: `OnceLock<BundleManager>` (immutable singleton, set once at startup)
- **Tests**: `AtomicPtr<BundleManager>` (resettable between test suites, sequential test execution only)
- **Cache**: `HashMap<String, BundleInterface>` inside `RwLock` for thread-safe double-checked locking

**Key Methods:**
- `initialize(bundle_path_root)` - Set up singleton
- `singleton() -> &'static BundleManager` - Get cached instance
- `load_bundle(bundle_hash)` - Lazy-load or return cached BundleInterface
- `run_bundle_string/uint64/bool/json(function_name, bundle_hash, details, job_data)` - Execute bundle functions with result conversion

**Safety:**
- `load_bundle()` uses double-checked locking under `RwLock`:
  1. Read lock: check if bundle already cached, return if found
  2. Write lock: re-check, create if not found (prevents thundering herd)
- All `run_bundle_*` methods acquire `PYTHON_MUTEX` before calling bundle functions

**Why Singleton?**
- Bundle initialization is expensive (Python interpreter setup, bytecode compilation)
- Cache prevents redundant reinitialization
- Shared across application lifetime

### `bundle_interface.rs` - Python Interpreter Wrapper
Wraps a single Python subinterpreter instance, providing safe methods for bundle execution.

**Design:**
- `BundleInterface` - Clone-able wrapper containing:
  - `inner: Arc<BundleInterfaceInner>` - Actual Python state
  - Stores subinterpreter pointer, module references
- All methods marked `pub unsafe fn` (must be called with `PYTHON_MUTEX` held)

**Key Methods:**
- `new(bundle_hash, bundle_path_root)` - Create new subinterpreter for bundle
- `thread_scope()` - RAII guard setting up Python thread state (see ThreadScope section)
- `run(function_name, details, job_data)` - Call bundle function, return PyObject result
- `to_string_py/to_uint64/to_bool/json_dumps/json_loads` - Type conversions
- `print_last_python_exception()` - Debug helper
- `dispose_object(obj)` - Decrease Python refcount

**Thread Safety:**
- `unsafe impl Send + Sync for BundleInterfaceInner`
- Justification: Only accessed under `PYTHON_MUTEX` (GIL equivalent); subinterpreter pointers are thread-safe when GIL held

### `python_interface.rs` - Python C-API Bindings
Low-level FFI bindings to the Python C-API. Handles:
- Dynamic library loading (`libloading` crate)
- Function pointer setup via `py_wrap!` macro
- GIL hook installation (binary patching via subhook)
- Subinterpreter creation/destruction

**Key Components:**

**PYTHON_MUTEX**
```rust
static PYTHON_MUTEX: parking_lot::RwLock<()> = parking_lot::RwLock::new(());
```
- Global mutual exclusion for all Python state access
- `parking_lot::RwLock` chosen for better performance than `std::sync::Mutex`
- Must be held when:
  - Calling any PyObject* function
  - Modifying subinterpreter state
  - Creating ThreadScope (thread state setup)

**SubInterpreter**
```rust
pub struct SubInterpreter {
    interp: *mut PyInterpreterState,
    // ... internal state
}
```
- Isolated Python namespace (separate sys.modules, globals, builtins)
- Created via `Py_NewInterpreter()` (requires GIL)
- Each bundle gets its own subinterpreter
- No global state bleed between bundles

**ThreadScope**
```rust
pub struct ThreadScope {
    _guard: PyGILState_EnsureGuard,
}
```
- RAII guard for OS thread state setup
- On construction: calls `PyGILState_Ensure()` → initializes Python thread state
- On destruction: calls `PyGILState_Release()` → cleans up thread state
- **Must** be created once per OS thread per Python execution
- Prevents crashes when thread hasn't been initialized with Python

**Extension Modules**
- `PyInit_bundledb()` - C-API extension for job database operations
- `PyInit_bundlelogging()` - C-API extension for logging
- Registered at startup via `PyImport_AppendInittab()`

**Safety Invariants:**
1. GIL must be held (PYTHON_MUTEX locked) when accessing any PyObject*
2. ThreadScope must exist before calling any Python code
3. Each OS thread needs its own ThreadState (ThreadScope provides this)
4. Refcounts must be managed carefully (Py_INCREF/Py_DECREF)

### `bundle_db.rs` - Database Extension Module
Python C-API extension module providing database operations to bundle functions.

**C Callables (invoked from Python):**
- `create_or_update_job(bundle_hash, job_id, ...)` - Insert/update job in DB
- `get_job_by_id(job_id)` - Query job from DB, return as JSON
- `delete_job(job_id)` - Delete job record

**Async Bridge:**
Uses `tokio::runtime::Handle::current().block_on()` to call async database functions from synchronous Python code.

**Safety:**
- C function signatures must match Python C-API calling convention
- Uses `#[unsafe(no_mangle)]` to ensure C calling convention
- All FFI operations wrapped in safety comments explaining invariants

### `bundle_logging.rs` - Logging Extension Module
Python C-API extension module providing logging to bundle functions.

**C Callable:**
- `write_log(message, level)` - Write to Rust logger via `tracing` crate

### `daemon.rs` - UNIX Daemonization
Implements UNIX double-fork daemonization pattern.

**Pattern:**
1. **First fork**: Parent exits immediately (init adopts child)
2. **setsid()**: Create new session (child becomes session leader)
3. **chdir("/")**: Change to root (prevents directory from being unmountable)
4. **umask(0)**: Clear umask (give full control over file permissions)
5. **Second fork**: Prevent child from acquiring controlling terminal
6. **Redirect stdio**: Connect stdin/stdout/stderr to /dev/null or log files

**Why Double-Fork?**
- First fork: Decouple from parent (parent exits, init adopts child)
- Second fork: Prevent daemon from acquiring controlling terminal (ensures daemon survives terminal closure)

### `messaging.rs` - WebSocket Message Format
Defines binary message format for WebSocket communication.

**Message Structure:**
```
[4 bytes] message_type (u32)
[4 bytes] priority (u32)
[4 bytes] body_length (u32)
[N bytes] body (variable length)
[body_length bytes] payload data
```

**Key Types:**
- `Message` struct - Encapsulates type, priority, source, payload
- Serialization/deserialization with `serde_json`
- Type constants: `JOB_SUBMIT`, `JOB_STATUS`, `DB_RESPONSE`, `FILE_WRITE`, etc.

**Why Binary?**
- Efficient (fixed 12-byte header + variable payload)
- Fast deserialization (no parsing needed for numeric fields)
- Extensible (priority routing, message types)

### `websocket.rs` - WebSocket Client
Async WebSocket client for persistent bidirectional communication.

**Key Components:**
- `WebsocketClient` - Manages connection, message queues, authentication
- `GAuthToken` - HMAC-based authentication (prevents unauthorized access)
- Keep-alive mechanism (periodic PING messages)
- Reconnection logic (exponential backoff)

**Design:**
- Async/await using tokio
- Spawned as background task at startup
- Separate send/receive channels for decoupling
- Thread-safe: can be cloned and shared across tasks

**Safety:**
- Public key stored in config (not executable code)
- Tokens are short-lived (time-limited via HMAC)
- No SQL injection (messages are JSON, not queries)

### `jobs/mod.rs` - Job Lifecycle Management
Handles job submission, status tracking, cancellation, deletion.

**Key Functions:**
- `submit_job(job_id, bundle_hash, ...)` - Calls bundle "submit" function, stores in DB
- `update_job(job_id, ...)` - Calls bundle "status" function, updates DB record
- `cancel_job(job_id, ...)` - Calls bundle "cancel" function, marks as deleted in DB
- `delete_job(job_id, ...)` - Calls bundle "delete" function, removes from DB and filesystem

**Lifecycle:**
```
SUBMIT MESSAGE
    ↓ (calls bundle "submit")
    ↓ (inserts into jobs table)
JOB_SUBMITTED ACK
    ↓
STATUS MESSAGE (polling)
    ↓ (calls bundle "status")
JOB_STATUS RESPONSE
    ↓ (if running, repeat)
    ↓ (if complete)
CANCEL or DELETE MESSAGE
    ↓ (calls bundle "cancel" or "delete")
    ↓ (removes from DB, cleans working directory)
ACK
```

**Working Directory:**
- Created per-job in submission
- Passed to bundle functions (working_directory field)
- Deleted when job is deleted or cancelled
- Contains job-specific files, outputs

### `files/mod.rs` - File Operations
Handles reading/writing/deleting files in job working directories.

**Key Functions:**
- `file_write(job_id, path, content)` - Write file to job directory
- `file_read(job_id, path)` - Read file from job directory
- `file_delete(job_id, path)` - Delete file from job directory

**Safety:**
- No symlink following (prevents directory traversal attacks)
- Paths validated against job directory (prevents escape)
- Permissions inherited from daemon process

### `db/mod.rs` - SQLite Database Access
Async database interface via SeaORM.

**Schema:**
```
jobclient_job (
  id INTEGER PRIMARY KEY,
  job_id INTEGER,
  scheduler_id INTEGER,
  submitting BOOLEAN,
  submitting_count INTEGER,
  bundle_hash TEXT,
  working_directory TEXT,
  running BOOLEAN,
  deleted BOOLEAN,
  deleting BOOLEAN
)

jobclient_jobstatus (
  id INTEGER PRIMARY KEY,
  what TEXT,
  state INTEGER,
  job_id INTEGER FOREIGN KEY
)
```

**Design:**
- **Production**: `OnceLock<DatabaseConnection>` (immutable singleton)
- **Tests**: `AtomicPtr<DatabaseConnection>` (resettable via `reset_for_test()`)
- Single connection shared across application (SeaORM manages pooling internally)
- Async/await (non-blocking, plays well with tokio)

**Key Functions:**
- `initialize(url)` - Connect to database
- `get_db()` - Get reference to connection
- `save_job()`, `get_job_by_id()`, `delete_job()` - Job operations
- `save_status()`, `get_job_status_by_job_id()` - Status operations

---

## Global Interpreter Lock (GIL) Architecture

### What is the GIL?

The Python Global Interpreter Lock is a mutex that protects Python's internal state. In CPython, only one OS thread can execute Python bytecode or modify Python objects at a time.

**Why it exists:**
- CPython's memory management (reference counting) is not thread-safe
- Simpler implementation (no need for atomic operations on every refcount change)
- Fine for most workloads (I/O-bound Python code releases GIL during system calls)

### How We Use It

**PYTHON_MUTEX Pattern:**
```rust
let _guard = PYTHON_MUTEX.lock();  // Acquire "GIL equivalent"
let _scope = bundle.thread_scope(); // Set up thread state
// ... call Python functions ...
// scope dropped (thread state cleaned up)
// guard dropped (PYTHON_MUTEX released)
```

This ensures:
1. No concurrent Python execution (mutex prevents interleaving)
2. Current OS thread is initialized with Python (ThreadScope ensures this)
3. Resource cleanup (RAII guards handle teardown)

**Why not actual Python GIL?**
- We're calling from Rust, not from within Python
- PYTHON_MUTEX is simpler, more explicit
- Prevents deadlocks (we control the lock, not Python)

### SubInterpreter Isolation

Each bundle gets its own Python subinterpreter:
```rust
// Separate sys.modules
bundle1.modules ≠ bundle2.modules

// Separate globals
bundle1.__builtins__ ≠ bundle2.__builtins__

// No global state bleed
import sys in bundle1 → doesn't affect bundle2
```

**Benefits:**
- Bundles are isolated (one malicious bundle can't crash others)
- No namespace pollution (bundles don't need to cooperate on naming)
- Easier to debug (state is localized)

### ThreadScope and PyGILState_EnsureGuard

Each OS thread needs its own Python thread state before executing Python code:

```rust
pub struct ThreadScope {
    _guard: PyGILState_EnsureGuard,
}

impl Drop for ThreadScope {
    fn drop(&mut self) {
        // PyGILState_Release() called here
    }
}
```

**Why?**
- Python tracks per-thread state (current frame, exception handlers, etc.)
- First call to `PyGILState_Ensure()` initializes thread state
- Must call `PyGILState_Release()` when done (ThreadScope Drop ensures this)

**Example:**
```rust
// Main thread (from Python's perspective, already has thread state)
let _guard = PYTHON_MUTEX.lock();
// Can call PyObject functions directly

// Worker thread (first time using Python)
let _guard = PYTHON_MUTEX.lock();
let _scope = bundle.thread_scope();  // Initializes thread state
// Now can call PyObject functions
```

---

## Job Execution Flow

### From WebSocket to Bundle Execution

```
1. WebSocket message received: { job_id, bundle_hash, function, details, params }
   ↓
2. Route to handler (jobs::submit_job, files::file_write, etc.)
   ↓
3. Get BundleManager singleton
   ↓
4. Load bundle (or get from cache)
   bundle_manager.load_bundle(bundle_hash)
   ↓
5. Acquire PYTHON_MUTEX (prevents other threads from using Python)
   ↓
6. Create ThreadScope (set up OS thread's Python state)
   ↓
7. Call bundle function (e.g., bundle.run("submit", details, params))
   - Convert Rust Value to Python object
   - Call Python function
   - Get result as Python object
   ↓
8. Convert Python result to Rust type (String, u64, bool, JSON)
   ↓
9. Dispose Python object (decrement refcount)
   ↓
10. Release ThreadScope (clean up thread state)
    ↓
11. Release PYTHON_MUTEX
    ↓
12. Send response message via WebSocket
```

### Why This Order?

1. **Singleton before mutex**: Singleton is thread-safe (OnceLock), no need to hold mutex
2. **Mutex before scope**: PYTHON_MUTEX guards all Python state, including thread state initialization
3. **Scope before bundle call**: Thread state must be initialized before PyObject calls
4. **Convert before dispose**: Must extract data before decrementing refcount
5. **Scope then mutex**: Drop order reverses acquisition order (RAII best practice)

---

## Database Transaction Flow

### Job Submission
```
WebSocket submit message
  ↓ jobs::submit_job()
  ↓ bundle.run("submit", ...) → returns job-specific data
  ↓ db::save_job() → INSERT into jobclient_job
  ↓ send JOB_SUBMITTED response
```

### Job Status Query
```
WebSocket status message
  ↓ jobs::update_job()
  ↓ db::get_job_by_id() → query jobclient_job
  ↓ bundle.run("status", ...) → returns job state
  ↓ db::save_status() → INSERT/UPDATE jobclient_jobstatus
  ↓ send JOB_STATUS response
```

### Job Deletion
```
WebSocket delete message
  ↓ jobs::delete_job()
  ↓ bundle.run("delete", ...)
  ↓ db::delete_job() → DELETE from jobclient_job
  ↓ fs::remove_dir_all(working_directory)
  ↓ send DELETE_OK response
```

---

## Safety Properties & Guarantees

### Memory Safety
- **No buffer overflows**: Rust bounds checking
- **No use-after-free**: RAII resource management (ThreadScope, File, Arc)
- **No data races**: Type system prevents shared mutable state
- **Controlled unsafety**: FFI boundary isolated to `python_interface.rs`, `bundle_db.rs`, `bundle_logging.rs`

### Thread Safety

**GIL Equivalent (PYTHON_MUTEX):**
- Single PYTHON_MUTEX guards all Python state access
- Any thread holding mutex is sole modifier of Python objects
- No thread can call PyObject function without mutex

**Per-Thread State (ThreadScope):**
- Each OS thread has its own Python thread state (managed by ThreadScope)
- Cannot use Python thread state from different thread (crashes CPython)
- ThreadScope ensures thread state exists before Python calls

**Subinterpreter Safety:**
- `unsafe impl Send + Sync` justified by:
  - Only accessible under PYTHON_MUTEX
  - Mutex prevents concurrent access
  - SubInterpreter pointers are stable (never moved/freed while referenced)

**Database Connection:**
- `Arc<DatabaseConnection>` wrapped in `OnceLock` (production) or `AtomicPtr` (tests)
- SeaORM connection is thread-safe (internal connection pool behind Arc/Mutex)
- Multiple threads can safely call `get_db()` → get shared reference

**BundleManager Cache:**
- `RwLock<HashMap>` allows concurrent reads (many threads can query cache)
- Double-checked locking prevents redundant initialization
- Bundle cloning (Arc) is atomic

### Resource Cleanup

**RAII Pattern:**
- `ThreadScope` Drop → `PyGILState_Release()` guaranteed
- `File` Drop → OS file handle closed guaranteed
- `DatabaseConnection` Drop → connection returned to pool / closed guaranteed
- No manual cleanup needed (no memory leaks from exceptions)

**Exception Handling:**
```rust
match bundle.run("submit", ...) {
    Ok(result) => { /* success */ }
    Err(NoneException) => { /* Python exception, still cleaned up */ }
}
// ThreadScope and PYTHON_MUTEX dropped regardless of path
```

---

## Design Decisions & Rationale

### SubInterpreters vs. Separate Processes

**Why SubInterpreters?**
- ✅ Shared memory (no IPC overhead)
- ✅ Direct Rust ↔ Python function calls (no marshaling)
- ✅ Simpler resource management (single process, single DB connection)
- ✅ Faster (no fork overhead, no message serialization)

**Trade-off:**
- ❌ All bundles share same Python version/C extensions
- ❌ One malicious bundle can crash the daemon (but isolation on filesystem helps)
- ❌ Harder to debug (mixed process state)

### parking_lot RwLock vs. std::sync::Mutex

**Why parking_lot?**
- ✅ Faster (lock-free fast path when uncontended)
- ✅ No poisoning (no panic recovery overhead)
- ✅ RwLock allows concurrent readers (PYTHON_MUTEX doesn't, but fast_local is fine)

**Trade-off:**
- ❌ External dependency
- ❌ Less standardized (harder for new maintainers)

### SeaORM vs. Raw sqlite3 Bindings

**Why SeaORM?**
- ✅ Type safety (compile-time SQL validation)
- ✅ Migration support (schema versioning)
- ✅ Async-friendly (plays well with tokio)
- ✅ ORM abstractions (less boilerplate)

**Trade-off:**
- ❌ Less control over exact SQL
- ❌ Performance overhead (query builder)

### Rust vs. C++

**Why Rust (over legacy C++)?**
- ✅ Memory safety (prevent buffer overflows, use-after-free)
- ✅ Fearless concurrency (type system prevents data races)
- ✅ Modern ecosystem (tokio, serde, sea-orm)
- ✅ Better error handling (Result<T, E> vs. exceptions/return codes)

**Trade-off:**
- ❌ Borrow checker learning curve
- ❌ Build times (monomorphization)
- ❌ Smaller community than C++

---

## Testing Architecture

### Test Isolation

Tests run in parallel (after OnceLock refactor):

**BundleManager Isolation:**
- Tests: `#[cfg(test)] AtomicPtr<BundleManager>`
- Each test module resets BundleManager before first test
- Sequential test execution ensures no race conditions

**Database Isolation:**
- Tests: `#[cfg(test)] reset_for_test(db_url)` - clear and recreate schema
- Each test suite gets fresh in-memory SQLite database
- Schema created fresh, no cross-test contamination

**WebSocket Isolation:**
- `MockWebsocketClient` in fixtures
- No real network access
- Predictable message sequences

### Test Modules

```
tests/
  ├─ mod.rs - Shared setup (Python initialization, fixtures)
  ├─ fixtures/ - Reusable test data (bundles, messages)
  ├─ bundle_db_tests.rs - Database extension module + job lifecycle
  ├─ bundle_logging_tests.rs - Logging extension module
  ├─ file_tests.rs - File I/O operations
  ├─ job_tests.rs - Job submission, status, updates
  ├─ job_cancel_tests.rs - Job cancellation
  └─ job_delete_tests.rs - Job deletion
```

### Running Tests

```bash
# All tests (parallel)
./run_tests.sh

# Specific module
./run_tests.sh tests::job_tests

# With output
./run_tests.sh -- --nocapture

# Coverage
./run_tests.sh --coverage
```

---

## Future Improvements

### 1. Connection Pooling
Currently: Single database connection shared globally.

Future: Multiple connections for concurrent job executions (SeaORM supports this).

```rust
sea_orm::Database::connect(&config)  // Built-in pooling
```

### 2. Async Bundle Execution
Currently: Bundle functions are synchronous, holding PYTHON_MUTEX while executing.

Future: Async Python functions (requires `asyncio` integration or green threads).

Trade-off: Complexity vs. throughput.

### 3. Telemetry & Observability
- Instrument bundle execution time (tracing spans)
- Database query metrics (slow queries)
- WebSocket connection stats
- CPU/memory usage per bundle

### 4. Graceful Shutdown
Currently: Daemon exits immediately on signal.

Future:
- Drain in-flight jobs
- Close database connection cleanly
- Shutdown tokio runtime gracefully
- Signal clients of upcoming maintenance

### 5. Hot Reloading of Bundles
Currently: Bundle changes require daemon restart.

Future:
- File system watcher on bundle directories
- Reload changed bundles on-demand
- Invalidate cache selectively

---

## References & Further Reading

- **Python C-API**: https://docs.python.org/3/c-api/
- **Subinterpreters**: https://docs.python.org/3/c-api/init.html#sub-interpreter-support
- **GIL Design**: https://www.python.org/dev/peps/pep-0703/
- **tokio Runtime**: https://tokio.rs/
- **SeaORM**: https://www.sea-ql.org/SeaORM/
- **parking_lot**: https://docs.rs/parking_lot/
