# Repeatable memory, mode, terminal and full-stack checks

Run `scripts/qa.sh` with Rust, Python 3, Node.js and `expect` installed. All of
these checks are offline. They never create a Daytona resource or call a paid
model. CI runs them for pushes to main and PRs targeting main, in one workflow.
Live memory evaluation remains an explicit workflow dispatch.

## What the checks prove

| Check | Executed behavior | Limit |
|---|---|---|
| `scripts/behavior_check.py` | Real Unix-socket daemon, 512 lessons, 17 labeled recall queries, 5 unrelated queries, response style selection, L2 restart, L1 clearing, p95 and storage | Small authored coding vocabulary; not universal semantic recall |
| `tests/advisor.rs` | Stalled hook returns within the 50 ms caller budget plus scheduling tolerance; 350 ms management reply succeeds; tone survives tools and forget removes it | Host scheduling can add delay; no latency SLA claim |
| `scripts/mode_smoke.py` | 42 labeled requests through the real CLI, request-level profiles, explicit overrides, Careful → Standard → Vibe → Standard with Auto still saved | Deterministic classifier, not an independent NLP benchmark |
| `tests/cli.rs` | Risky tools escalate before dispatch, permissions stay separate, failures/repeated calls are bounded, careful goals get one checker including resume | Offline provider supplies the tool proposals |
| `tests/tty/capture.py` + `inspect.py` | Real expect/PTY at 24/40/80/120 columns, complete frames, scrollback, fixed bottom panel, no wrap-column use, clean exit, approval and interruption | Fixed viewport checks; not every terminal emulator or live resize |
| `scripts/fullstack_smoke.py` | Three reproduced bugs repaired with real reads/edits/Bash; actual local HTTP API, blank validation, persistence restart, assets and shipped JS rendering; Off/On/Deep | Scripted repair plan; real-browser QA is separate, not part of CI |

The new full-stack fixture intentionally starts broken. It is a local demo,
not a hardened internet service. Hidden acceptance checks live in the harness,
outside the project copied into the coding agent's workspace. The fixture's
public API tests are available to the agent, as they would be in a normal repo.

## Run individual checks

```sh
cargo build --bins
python3 scripts/behavior_check.py --report target/memory-quality.json
python3 scripts/memory_bench.py --binary target/debug/rusty-memoryd --report target/memory-bench.json
python3 scripts/mode_smoke.py --report target/mode-quality.json
python3 scripts/fullstack_smoke.py --report target/fullstack-quality.json
python3 tests/tty/capture.py --binary target/debug/rusty --output target/tty-quality
python3 tests/tty/inspect.py target/tty-quality
```

The recall gate requires every expected record and silence on unrelated cases;
the latency gate is p95 below 20 ms and the observed folder below 20 MiB. Neither
is a hard process-memory or disk quota. Tone checks inspect injected guidance;
a scripted final answer does not prove a live model follows user style.

## Record the new full-stack GIF

Requires `agg` in addition to the test dependencies:

```sh
python3 scripts/fullstack_smoke.py --modes on --cast target/fullstack.cast \
  --workspace target/repaired-taskboard --report target/fullstack-recording.json
agg --theme monokai --font-size 15 --speed 1.2 --fps-cap 15 \
  target/fullstack.cast docs/demo/fullstack.gif
python3 target/repaired-taskboard/app.py --port 8765
```

The workspace destination must not exist; the harness refuses to replace it.
Open `http://127.0.0.1:8765`, submit a task and reload to check persistence. For
an independent live-model attempt, copy `demo/fullstack/files` to a fresh
workspace and ask Rusty to fix its API/UI and run the tests, using your chosen
provider. Provider costs and failures must be recorded separately from offline
qualification. Do not report the new scripted GIF as that live attempt.
