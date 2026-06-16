# ADACS Job Controller Client — Domain Map

> Rust daemon managing computational jobs on distributed clusters via WebSocket.
> All DB operations are remote (WebSocket proxy), not direct SQLite.

## Module Layering & Dependency Direction

```
config → main → daemon → python_interface → bundle_interface → bundle_manager
                 ↓                                           ↓
            logging.rs                                   bundle_db
                 ↓                                   bundle_logging
            update_check
                 ↓
            websocket → messaging → jobs → db (remote proxy)
                                      ↓
                                  files
                                      ↓
                              thread_bundle_map
                                      ↓
                              db_bridge (async dispatch)
```

## Domain Map

| Domain | Module | Owns | Depends On |
|--------|--------|------|------------|
| **Python FFI** | `python_interface` | C-API bindings, GIL mutex, subinterpreter lifecycle | libpython (dynamic) |
| **Bundle Runtime** | `bundle_interface`, `bundle_manager` | Subinterpreter cache, bundle function dispatch, ThreadScope | python_interface |
| **C Extensions** | `bundle_db`, `bundle_logging` | Python-callable DB/logging functions (PyInit_*) | python_interface, websocket |
| **WebSocket** | `websocket` | Persistent connection, priority queue, ping/pong, reconnect | messaging |
| **Messaging** | `messaging` | Binary protocol (LittleEndian), message type constants | — |
| **DB Proxy** | `db`, `db_bridge` | Remote DB operations over WebSocket, async dispatch queue | websocket, messaging |
| **Job Lifecycle** | `jobs` | Submit/status/cancel/delete/archive | bundle_manager, db, websocket |
| **File I/O** | `files` | List/download/upload with path traversal protection | bundle_manager, db, websocket |
| **Lifecycle** | `main`, `daemon` | CLI, daemonization, signal handling, event loop | everything |
| **Config** | `config` | config.json reading (ltk, endpoint, pythonLibrary) | — |
| **Logging** | `logging` | Rotating file + stderr via tracing-appender | — |
| **Auto-Update** | `update_check` | GitHub release check, download, binary replacement | semver, ureq |
| **Thread Tracking** | `thread_bundle_map` | ThreadId → bundle_hash mapping (RAII guard) | — |

## Key Integration Points

```
WebSocket message → jobs::handle_job_submit()
  → BundleManager::singleton().run_bundle_*()    [PYTHON_MUTEX + ThreadScope]
  → db::save_job() → DbBridge::send() → WebSocket DB response
  → websocket::queue_message() → server
```

## Data Flow

```
Server → WebSocket (binary msg) → handle_message() dispatches by msg.id
  → jobs/ or files/ handler → BundleManager → Python subinterpreter
  → db:: (remote) → WebSocket → server DB
  → Result → WebSocket → server
```

## Infrastructure Topology

```
[Job Controller Server] ← WebSocket → [adacs_job_client daemon]
                                         ↕ (via db_bridge)
                                      [Remote DB]
                                         ↓
                                   Python bundles (subinterpreters)

Deployment: RHEL 7 / glibc 2.17, single binary, no runtime deps beyond libc
```

## Safety Invariants

1. **`PYTHON_MUTEX` + `ThreadScope`** — required for ALL Python FFI calls
2. **Path traversal protection** — all file I/O validates against canonical working directory
3. **Binary protocol bounds-checked** — `pop_bytes` validates buffer length
4. **Reconnectable vs one-shot** — LTK in config enables reconnect; without it, disconnect = exit
