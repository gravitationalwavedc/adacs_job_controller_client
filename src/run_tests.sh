#!/bin/bash
#
# Test runner script for ADACS Job Controller Client (Rust port)
#
# Usage:
#   ./run_tests.sh                              # Run all tests sequentially
#   ./run_tests.sh --verbose                    # Run with verbose output
#   ./run_tests.sh tests::job_tests             # Run specific test suite
#   ./run_tests.sh -- --nocapture               # Pass through to cargo test
#   ./run_tests.sh --coverage                   # Generate coverage report
#   ./run_tests.sh --coverage --open            # Generate and open coverage report
#
# IMPORTANT: Tests must run sequentially (--test-threads=1) to avoid
# race conditions with shared global state (BundleManager, websocket mocks).

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Default: sequential execution for test isolation
DEFAULT_TEST_ARGS="--test-threads=1"

# Check for --coverage flag
COVERAGE=false
OPEN_COVERAGE=false
COVERAGE_ARGS=()

for arg in "$@"; do
    if [[ "$arg" == "--coverage" ]]; then
        COVERAGE=true
    elif [[ "$arg" == "--open" ]]; then
        OPEN_COVERAGE=true
    fi
done

# Remove coverage-related flags from arguments passed to cargo
FILTERED_ARGS=()
for arg in "$@"; do
    if [[ "$arg" != "--coverage" && "$arg" != "--open" ]]; then
        FILTERED_ARGS+=("$arg")
    fi
done

if [[ "$COVERAGE" == true ]]; then
    # Generate coverage report using cargo-llvm-cov
    echo "Running tests with coverage..."

    # Build base command with coverage options first, then test args
    COVERAGE_CMD="cargo llvm-cov --html -- --test-threads=1"

    # Add any additional filtered args (excluding --test-threads=1)
    for arg in "${FILTERED_ARGS[@]}"; do
        if [[ "$arg" != "--test-threads=1" ]]; then
            COVERAGE_CMD="$COVERAGE_CMD $arg"
        fi
    done

    if [[ "$OPEN_COVERAGE" == true ]]; then
        COVERAGE_CMD="$COVERAGE_CMD --open"
    fi

    exec $COVERAGE_CMD
elif [[ "$*" == *"--test-threads"* ]]; then
    # User knows what they're doing, pass through
    exec cargo test "$@"
elif [[ $# -eq 0 ]]; then
    # No arguments - run all tests sequentially
    exec cargo test -- $DEFAULT_TEST_ARGS
else
    # Arguments provided - separate cargo args from test args
    # Everything before '--' goes to cargo, everything after goes to test binary
    CARGO_ARGS=()
    TEST_ARGS=()
    PASSED_THROUGH=false

    for arg in "${FILTERED_ARGS[@]}"; do
        if [[ "$arg" == "--" ]]; then
            PASSED_THROUGH=true
            continue
        fi

        if [[ "$PASSED_THROUGH" == true ]]; then
            TEST_ARGS+=("$arg")
        else
            CARGO_ARGS+=("$arg")
        fi
    done

    # Run cargo test with our default test args
    if [[ ${#TEST_ARGS[@]} -eq 0 ]]; then
        exec cargo test "${CARGO_ARGS[@]}" -- $DEFAULT_TEST_ARGS
    else
        exec cargo test "${CARGO_ARGS[@]}" -- "${TEST_ARGS[@]}" $DEFAULT_TEST_ARGS
    fi
fi
