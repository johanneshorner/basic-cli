#!/usr/bin/env bash

# https://vaneyckt.io/posts/safer_bash_scripts_with_set_euxo_pipefail/
set -exo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

EXAMPLES_DIR="${ROOT_DIR}/examples/"
export EXAMPLES_DIR

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

# Build the platform using build.sh
if [ "$NO_BUILD" != "1" ]; then
    echo "Building platform..."
    ./build.sh
fi

# roc check migrated examples
echo "Checking migrated examples..."
for example in "${MIGRATED_EXAMPLES[@]}"; do
    roc check "examples/${example}.roc"
done

# roc build migrated examples
echo "Building migrated examples..."
for example in "${MIGRATED_EXAMPLES[@]}"; do
    roc build "examples/${example}.roc"
    mv "./${example}" "examples/"
done

# Run expect tests
echo "Running expect tests..."
for example in "${MIGRATED_EXAMPLES[@]}"; do
    echo "=== Testing $example ==="
    expect "ci/expect_scripts/${example}.exp"
    echo "=== $example passed ==="
done

# Clean up built binaries
echo "Cleaning up..."
for example in "${MIGRATED_EXAMPLES[@]}"; do
    rm -f "examples/${example}"
done

echo ""
echo "All tests passed!"
