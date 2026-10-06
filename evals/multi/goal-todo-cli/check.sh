set -e
rm -f todo.json
run() { python3 todo.py "$@"; }
fail() { echo "FAIL: $*"; exit 1; }
[ "$(run add 'buy milk' | tr -d '[:space:]')" = "1" ] || fail "first add should print id 1"
[ "$(run add 'walk dog' | tr -d '[:space:]')" = "2" ] || fail "second add should print id 2"
run done 1 >/dev/null || fail "done 1 exited non-zero"
run rm 2 >/dev/null || fail "rm 2 exited non-zero"
[ "$(run add 'call mum' | tr -d '[:space:]')" = "3" ] || fail "add after rm 2 should print id 3 (ids are never reused)"
out=$(run list)
echo "$out" | grep -q "^1 \[x\] buy milk$" || fail "list should show '1 [x] buy milk', got: $out"
echo "$out" | grep -q "^3 \[ \] call mum$" || fail "list should show '3 [ ] call mum', got: $out"
! echo "$out" | grep -q "walk dog" || fail "removed item still listed: $out"
