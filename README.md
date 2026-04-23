# ADACS Job Controller Client (Rust)

Rust port of the ADACS Job Controller Client. The original C++ implementation is in `legacy/`.

## First-Time Setup

1. **Install Rust**:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Install Zig** (required for release builds):
   ```bash
   curl -sSf https://webinstall.dev/zig | bash
   source ~/.config/envman/PATH.env
   ```

3. **Install cargo-zigbuild**:
   ```bash
   cargo install cargo-zigbuild
   ```

4. **Clone and initialize submodules**:
   ```bash
   git clone https://gitlab.com/CAS-eResearch/GWDC/adacs_job_controller_client.git
   cd adacs_job_controller_client
   git submodule update --init --recursive
   ```

## Project Structure

All Rust code is in the `src/` directory. All commands below should be run from `src/`:

```bash
cd src
```

## Development

### Running Tests

The test suite (127 tests) uses in-memory SQLite and must run sequentially to avoid race conditions with shared global state.

```bash
# Run all tests
./run_tests.sh

# Run specific test module
./run_tests.sh job_tests
./run_tests.sh file_tests
./run_tests.sh bundle_db_tests

# Run with verbose output
./run_tests.sh --verbose

# Run specific test function
./run_tests.sh test_check_status_job_running -- --nocapture

# Pass any arguments to cargo test
./run_tests.sh -- --test-threads=1 --nocapture
```

### Coverage

```bash
# Generate HTML coverage report
./run_tests.sh --coverage

# Generate and open in browser
./run_tests.sh --coverage --open
```

Report location: `target/llvm-cov/html/index.html`

### Debug Build

```bash
cargo build
```

Output: `target/debug/adacs_job_client`

### Run the Binary

```bash
cargo run
```

## Release Build (glibc 2.17 / RHEL 7 Compatible)

Builds a binary compatible with RHEL 7 / CentOS 7 and newer. Only depends on core glibc libraries:
- `libc.so.6` - C library
- `libm.so.6` - Math library
- `libpthread.so.0` - POSIX threads
- `libdl.so.2` - Dynamic linking

No external dependencies (OpenSSL, libcurl, etc.) are dynamically linked.

```bash
./build_release.sh
```

Output: `target/x86_64-unknown-linux-gnu/release/adacs_job_client`

### Manual build:
```bash
cargo zigbuild --target x86_64-unknown-linux-gnu.2.17 --release
```

### Verify the binary:
```bash
# Check dependencies (should only show libc, libm, libpthread, libdl)
ldd target/x86_64-unknown-linux-gnu/release/adacs_job_client

# Check glibc version requirement (should be 2.17)
objdump -T target/x86_64-unknown-linux-gnu/release/adacs_job_client | grep GLIBC_ | cut -d@ -f2 | sed 's/.*GLIBC_\([0-9.]*\).*/\1/' | sort -V | tail -1
```

## Troubleshooting

### Zig not found
```bash
source ~/.config/envman/PATH.env
# Or reinstall: curl -sSf https://webinstall.dev/zig | bash
```

### cargo-zigbuild not found
```bash
cargo install cargo-zigbuild
```

### Submodule errors
```bash
git submodule update --init --recursive
```

### Build fails with missing headers
Ensure submodules are initialized, especially `mempatch/subhook`:
```bash
git submodule update --init --recursive
```

### Clean build
```bash
cargo clean
cargo build
```

## Requirements

- **Build**: Rust stable, Zig 0.13+
- **Runtime**: glibc 2.17+ (RHEL 7 / CentOS 7 or newer)
