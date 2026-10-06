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
| `scripts/session_bench.py` | Actual interactive REPL, 48 approval decisions, 24 dynamic resizes at 24–120 columns, Ctrl-C during approvals/streaming, recovery; cwd/Unicode/symlink/permission cases; child-process deadlines; repeated Auto sessions | Scripted proposals; terminal regression and local overhead, not model task quality |
| `scripts/memory_stress.py` | Eight concurrent socket clients with 512 lessons, each doing begin/advice/event/finish; recall, errors, p95 and storage | Synthetic clients, not hosted or model-backed swarms |

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
python3 scripts/session_bench.py --trials 30 --report target/session-quality.json
python3 scripts/memory_stress.py --rounds 100 --report target/memory-stress.json
```

The recall gate requires every expected record and silence on unrelated cases;
the latency gate is p95 below 20 ms and the observed folder below 20 MiB. Neither
is a hard process-memory or disk quota. Tone checks inspect injected guidance;
a scripted final answer does not prove a live model follows user style.

Session latency includes process startup and three sequential responses from
a localhost scripted provider. It measures CLI and memory overhead, not NVIDIA
latency or coding ability. The concurrent memory p95 gate is 50 ms; independent
single-client advice retains its 20 ms gate. CI uses three session trials per
memory mode and 25 rounds per contention worker; longer runs are optional.

Bash starts a fresh shell for every call. `cd` and `export` affect that call
only. Its optional `cwd` selects a directory for one call (no shell expansion),
is canonicalized before permission classification, and never changes the
session's project identity. Pass it again for later Bash calls and use absolute
paths with file tools when working outside the starting project. Run `rusty -C
/path/to/project` to start a session with another project and its own memory.

Approvals use cooked terminal input rather than constructing a second line
editor; backspace remains available and Ctrl-C cancels. This avoids replacing
the REPL's process-wide resize handler. Timed-out Bash commands kill their
process group, including ordinary children that hold the output pipes. A child
that deliberately escapes into a new process group is outside this guarantee.
The password redactor excludes the shell's `PWD`/`OLDPWD` environment variables;
explicit password fields such as `DB_PWD` and `pwd=value` remain protected.

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
