# The visible test must survive; hidden tests check the full contract.
grep -q 'fn keeps_the_newest_items_when_full' src/lib.rs || exit 1
mkdir -p tests && cp "$EVAL_DIR/hidden.rs" tests/zz_hidden.rs
cargo test -q 2>&1 | tail -15
test "${PIPESTATUS[0]}" -eq 0
