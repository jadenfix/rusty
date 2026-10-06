#!/usr/bin/env bash
# Runs eval tasks headless in fresh temp copies and scores each with its own
# check, which the agent never sees. Results go to target/evals/<stamp>.jsonl.
#
# Single-turn tasks (evals/<name>/prompt.txt) run as one prompt.
# Multi-turn tasks (evals/multi/<name>/turns.txt) are replayed line by line
# through the interactive REPL, one line per user message:
#   ## session      start a new process (memory must carry over)
#   ## env K=V      set an environment variable from here on
#   ## goal         the next line runs as an autonomous --goal
#
#   scripts/eval.sh                  # everything, EVAL_JOBS at a time (default 4)
#   scripts/eval.sh multi            # only multi-turn tasks
#   scripts/eval.sh rust-slugify     # one task
set -uo pipefail
cd "$(dirname "$0")/.."
root=$PWD
cargo build --release -q
bin=$root/target/release/rusty
limit=${EVAL_TIMEOUT:-600}
model=${RUSTY_MODEL:-nvidia/nemotron-3-super-120b-a12b}
jobs_max=${EVAL_JOBS:-4}
stamp=$(date +%Y%m%d-%H%M%S)
logs=$root/target/evals/$stamp
mkdir -p "$logs"
out=$root/target/evals/$stamp.jsonl

# Runs one rusty process with a hard time limit. Extra args pass through.
agent() {
  RUSTY_HOME=$home RUSTY_MODEL=$model NO_COLOR=1 \
    perl -e 'alarm shift; exec @ARGV' "$limit" env ${envs[@]+"${envs[@]}"} "$bin" --yolo --stats "$@" \
    >>"$home/stdout" 2>>"$home/stderr"
}

run_multi() {
  local buf=() goal=0 line code=0
  envs=()
  flush() {
    ((${#buf[@]})) || return 0
    if ((goal)); then agent --goal "${buf[0]}" </dev/null
    else printf '%s\n' "${buf[@]}" | agent; fi
    local c=$?; ((c == 142)) && code=142
    buf=(); goal=0
  }
  while IFS= read -r line || [[ -n $line ]]; do
    case "$line" in
      "## session") flush ;;
      "## goal") flush; goal=1 ;;
      "## env "*) envs+=("${line#\#\# env }") ;;
      "") ;;
      *) buf+=("$line") ;;
    esac
  done <"$dir/turns.txt"
  flush
  return $code
}

run_task() {
  name=$1 dir=$2
  local work start code verdict
  work=$(mktemp -d) home=$(mktemp -d)
  cp -R "$dir/files/." "$work/" 2>/dev/null
  start=$(date +%s)
  envs=()
  if [[ -f $dir/turns.txt ]]; then
    (cd "$work" && run_multi)
  else
    (cd "$work" && agent "$(cat "$dir/prompt.txt")" </dev/null)
  fi
  code=$?
  local secs=$(( $(date +%s) - start ))
  verdict=fail
  if [[ $code == 142 ]]; then verdict=timeout
  elif (cd "$work" && EVAL_DIR=$dir RUSTY_HOME=$home bash "$dir/check.sh" >"$home/check" 2>&1); then verdict=pass
  # The endpoint failing is an infrastructure error, not an agent failure.
  elif grep -qE "giving up after|HTTP 5[0-9][0-9]|request failed|connection to the model closed" "$home/stdout" "$home/stderr"; then verdict=infra
  fi
  mkdir -p "$logs/$name" && cp "$home/stdout" "$home/stderr" "$home/check" "$logs/$name/" 2>/dev/null
  python3 - "$name" "$model" "$verdict" "$secs" "$home" "$out" <<'PY'
import json, sys
name, model, verdict, secs, home, out = sys.argv[1:]
runs = [json.loads(l) for l in open(f"{home}/stderr") if l.startswith("{")]
tot = {k: sum(r.get(k, 0) or 0 for r in runs) for k in ("requests", "prompt", "completion")}
reason = open(f"{home}/check").read().strip().splitlines()[-1:] if verdict == "fail" else []
row = {"task": name, "model": model, "verdict": verdict, "secs": int(secs), "sessions": len(runs), **tot, "reason": reason}
open(out, "a").write(json.dumps(row) + "\n")
mark = {"pass": "✓", "fail": "✗", "timeout": "⧗", "infra": "⚠"}[verdict]
print(f"{mark} {name:<18} {verdict:<8} {int(secs):>4}s  {len(runs)} session(s)  {(tot['prompt'] + tot['completion']) / 1000:>5.0f}k tok  {' '.join(reason)[:80]}")
PY
  rm -rf "$work" "$home"
}

echo "model $model · timeout ${limit}s per session"
for dir in evals/*/ evals/multi/*/; do
  [[ -f $dir/prompt.txt || -f $dir/turns.txt ]] || continue
  name=$(basename "$dir")
  case "${1:-}" in
    "") ;;
    multi) [[ $dir == evals/multi/* ]] || continue ;;
    *) [[ $name == "$1" ]] || continue ;;
  esac
  while (( $(jobs -rp | wc -l) >= jobs_max )); do sleep 1; done
  run_task "$name" "$root/${dir%/}" &
done
wait

python3 - "$out" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
passed = sum(r["verdict"] == "pass" for r in rows)
infra = sum(r["verdict"] == "infra" for r in rows)
scored = len(rows) - infra
tok = sum(r["prompt"] + r["completion"] for r in rows)
note = f" · {infra} infrastructure errors (not scored; rerun them)" if infra else ""
print(f"\n{passed}/{scored} passed{note} · {tok/1000:.0f}k tokens · {sum(r['secs'] for r in rows)}s agent time")
print(f"logs: {sys.argv[1][:-6]}/")
sys.exit(0 if passed == scored else 1)
PY
