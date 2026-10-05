set -e
rm -f todo.json
run() { python3 todo.py "$@"; }
[ "$(run add 'buy milk' | tr -d '[:space:]')" = "1" ]
[ "$(run add 'walk dog' | tr -d '[:space:]')" = "2" ]
run done 1 >/dev/null
run rm 2 >/dev/null
[ "$(run add 'call mum' | tr -d '[:space:]')" = "3" ]
out=$(run list)
echo "$out" | grep -q "^1 \[x\] buy milk$"
echo "$out" | grep -q "^3 \[ \] call mum$"
! echo "$out" | grep -q "walk dog"
