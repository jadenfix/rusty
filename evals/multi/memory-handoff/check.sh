python3 -c "
from stats import median
assert median([3, 1, 2]) == 2 and median([4, 1, 3, 2]) == 2.5
try:
    median([]); raise SystemExit('no ValueError')
except ValueError:
    pass
" && grep -rqs "median" tests/ && cat "$RUSTY_HOME"/projects/*/memory.jsonl | grep -qiE "make check|gen_fixtures"
