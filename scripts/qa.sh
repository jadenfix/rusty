#!/usr/bin/env bash
# Full QA pass. Offline by default; --live adds the model-backed tests,
# the real-terminal Ctrl-C check, and the eval suite.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n\033[1m▸ %s\033[0m\n' "$1"; }

step "format"
cargo fmt --check
step "lint"
cargo clippy --all-targets -- -D warnings
step "unit + offline end-to-end tests"
cargo test

if [[ "${1:-}" == "--live" ]]; then
  cargo build --release
  step "live end-to-end tests"
  cargo test --test cli -- --ignored --test-threads 4
  if command -v expect >/dev/null; then
    step "real-terminal ctrl-c"
    work=$(mktemp -d)
    (cd "$work" && expect "$OLDPWD/tests/tty/ctrl_c.exp" "$OLDPWD/target/release/rusty")
    rm -rf "$work"
  else
    echo "expect not installed; skipping the terminal check"
  fi
  step "evals"
  scripts/eval.sh
fi
printf '\n\033[32m✓ qa passed\033[0m\n'
