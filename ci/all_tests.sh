#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

# Get the roc commit pinned in Cargo.toml
ROC_COMMIT=$(python3 ci/get_roc_commit.py)
ROC_COMMIT_SHORT="${ROC_COMMIT:0:8}"
NEED_BUILD=false

echo "=== basic-cli CI ==="
echo ""

# Check if roc exists and matches pinned commit
if [ -d "roc-src" ] && [ -f "roc-src/zig-out/bin/roc" ]; then
    CACHED_VERSION=$(./roc-src/zig-out/bin/roc version 2>/dev/null || echo "unknown")
    if echo "$CACHED_VERSION" | grep -q "$ROC_COMMIT_SHORT"; then
        echo "roc already at correct version: $CACHED_VERSION"
    else
        echo "Cached roc ($CACHED_VERSION) doesn't match pinned commit ($ROC_COMMIT_SHORT)"
        echo "Removing stale roc-src..."
        rm -rf roc-src
        NEED_BUILD=true
    fi
else
    NEED_BUILD=true
fi

if [ "$NEED_BUILD" = true ]; then
    echo "Building roc from pinned commit $ROC_COMMIT..."

    rm -rf roc-src
    git init roc-src
    cd roc-src
    git remote add origin https://github.com/roc-lang/roc
    git fetch --depth 1 origin "$ROC_COMMIT"
    git checkout --detach "$ROC_COMMIT"

    zig build roc

    # Add to GITHUB_PATH if running in CI
    if [ -n "${GITHUB_PATH:-}" ]; then
        echo "$(pwd)/zig-out/bin" >> "$GITHUB_PATH"
    fi

    cd ..
fi

# Ensure roc is in PATH
export PATH="$(pwd)/roc-src/zig-out/bin:$PATH"

echo ""
echo "Using roc version: $(roc version)"

# Build the platform
if [ "${NO_BUILD:-}" != "1" ]; then
    echo ""
    echo "=== Building platform ==="
    ./build.sh
else
    echo ""
    echo "=== Skipping platform build (NO_BUILD=1) ==="
fi

# List of migrated examples that have expect tests
MIGRATED_EXAMPLES=(
    "command-line-args"
    "hello-world"
    "stdin-basic"
    "env-test"
    "file-test"
    "print-test"
    "dir-test"
    "path"
)

EXAMPLES_DIR="${ROOT_DIR}/examples/"
export EXAMPLES_DIR

# roc check migrated examples
echo ""
echo "=== Checking examples ==="
for example in "${MIGRATED_EXAMPLES[@]}"; do
    echo "Checking: ${example}.roc"
    roc check "examples/${example}.roc"
done

# roc build migrated examples
echo ""
echo "=== Building examples ==="
for example in "${MIGRATED_EXAMPLES[@]}"; do
    echo "Building: ${example}.roc"
    roc build "examples/${example}.roc"
    mv "./${example}" "examples/"
done

# Run expect tests
echo ""
echo "=== Running expect tests ==="
FAILED=0
for example in "${MIGRATED_EXAMPLES[@]}"; do
    echo ""
    echo "--- Testing: $example ---"
    set +e
    expect "ci/expect_scripts/${example}.exp"
    EXIT_CODE=$?
    set -e
    if [ $EXIT_CODE -eq 0 ]; then
        echo "PASS: $example"
    else
        echo "FAIL: $example (exit code: $EXIT_CODE)"
        FAILED=1
    fi
done

# Clean up built binaries
echo ""
echo "=== Cleaning up ==="
for example in "${MIGRATED_EXAMPLES[@]}"; do
    rm -f "examples/${example}"
done

echo ""
if [ $FAILED -eq 0 ]; then
    echo "=== All tests passed! ==="
else
    echo "=== Some tests failed ==="
    exit 1
fi
