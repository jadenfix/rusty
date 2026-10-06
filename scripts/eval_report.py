#!/usr/bin/env python3
"""Summarises eval runs: one row per model, then one row per task across
models. Reads one or more target/evals/*.jsonl files (rows from several runs
are pooled), prints the tables, writes them next to the first file as .md,
and appends them to $GITHUB_STEP_SUMMARY when set. Exits non-zero unless
every scored run passed.

    python3 scripts/eval_report.py target/evals/20261006-190000.jsonl [more.jsonl ...]
"""
import json
import os
import sys
from collections import defaultdict

MARK = {"pass": "✓", "fail": "✗", "timeout": "⧗", "infra": "⚠"}


def load(paths):
    rows = []
    for p in paths:
        with open(p) as f:
            rows += [json.loads(line) for line in f if line.strip()]
    return rows


def report(rows):
    models = list(dict.fromkeys(r["model"] for r in rows))
    tasks = sorted({r["task"] for r in rows})
    by = defaultdict(list)
    for r in rows:
        by[(r["model"], r["task"])].append(r)

    lines = ["| model | passed | pass rate | timeouts | infra (not scored) | tokens | agent time |",
             "|---|---|---|---|---|---|---|"]
    for m in models:
        mine = [r for r in rows if r["model"] == m]
        scored = [r for r in mine if r["verdict"] != "infra"]
        passed = sum(r["verdict"] == "pass" for r in scored)
        tok = sum(r["prompt"] + r["completion"] for r in mine)
        rate = f"{100 * passed / len(scored):.0f}%" if scored else "–"
        lines.append(f"| `{m}` | {passed}/{len(scored)} | {rate} | "
                     f"{sum(r['verdict'] == 'timeout' for r in mine)} | {sum(r['verdict'] == 'infra' for r in mine)} | "
                     f"{tok / 1000:.0f}k | {sum(r['secs'] for r in mine) / 60:.1f} min |")

    lines += ["", "| task | " + " | ".join(f"`{m}`" for m in models) + " |", "|---|" + "---|" * len(models)]
    for t in tasks:
        cells = []
        for m in models:
            runs = by[(m, t)]
            scored = [r for r in runs if r["verdict"] != "infra"]
            if not runs:
                cells.append("")
                continue
            marks = "".join(MARK[r["verdict"]] for r in sorted(runs, key=lambda r: r.get("rep", 1)))
            cells.append(f"{sum(r['verdict'] == 'pass' for r in scored)}/{len(scored)} {marks}")
        lines.append(f"| {t} | " + " | ".join(cells) + " |")

    misses = [r for r in rows if r["verdict"] in ("fail", "timeout")]
    if misses:
        lines += ["", "Misses:"]
        for r in sorted(misses, key=lambda r: (r["model"], r["task"], r.get("rep", 1))):
            why = " ".join(r.get("reason") or []) or r["verdict"]
            lines.append(f"- `{r['model']}` {r['task']} #{r.get('rep', 1)}: {why[:160]}")
    return "\n".join(lines)


def main():
    paths = sys.argv[1:]
    if not paths:
        sys.exit(__doc__)
    rows = load(paths)
    if not rows:
        sys.exit("no eval rows")
    text = report(rows)
    print("\n" + text)
    with open(paths[0][: -len(".jsonl")] + ".md" if paths[0].endswith(".jsonl") else paths[0] + ".md", "w") as f:
        f.write(text + "\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write("## rusty evals\n\n" + text + "\n")
    scored = [r for r in rows if r["verdict"] != "infra"]
    sys.exit(0 if scored and all(r["verdict"] == "pass" for r in scored) else 1)


if __name__ == "__main__":
    main()
