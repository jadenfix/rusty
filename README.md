<div align="center">

```
   ⋆        ·      ✦                 ·    ⋆               ✧        ·
          ▀▀▀▀▀▀▀▀▀▀▀   ▀▀▀      ▀▀▀   ▀▀▀▀▀▀▀▀▀▀▀  ▀▀▀▀▀▀▀▀▀▀▀▀  ▀▀▀      ▀▀▀
         ▀▀▀▀▀▀▀▀▀▀▀▀  ▀▀▀      ▀▀▀  ▀▀▀▀▀▀▀▀▀▀▀▀  ▀▀▀▀▀▀▀▀▀▀▀▀   ▀▀▀    ▀▀▀
        ▀▀▀      ▀▀▀  ▀▀▀      ▀▀▀  ▀▀▀               ▀▀▀▀        ▀▀▀  ▀▀▀
       ▀▀▀      ▀▀▀  ▀▀▀      ▀▀▀  ▀▀▀▀▀▀▀▀▀▀▀       ▀▀▀▀         ▀▀▀▀▀▀
      ▀▀▀▀▀▀▀▀▀▀▀   ▀▀▀      ▀▀▀   ▀▀▀▀▀▀▀▀▀▀▀      ▀▀▀▀          ▀▀▀▀
     ▀▀▀▀▀▀▀▀▀▀    ▀▀▀      ▀▀▀           ▀▀▀      ▀▀▀▀          ▀▀▀▀
    ▀▀▀    ▀▀▀    ▀▀▀      ▀▀▀           ▀▀▀      ▀▀▀▀          ▀▀▀▀
   ▀▀▀     ▀▀▀   ▀▀▀▀▀▀▀▀▀▀▀▀  ▀▀▀▀▀▀▀▀▀▀▀▀      ▀▀▀▀          ▀▀▀▀
  ▀▀▀      ▀▀▀   ▀▀▀▀▀▀▀▀▀▀   ▀▀▀▀▀▀▀▀▀▀▀       ▀▀▀▀          ▀▀▀▀
     ·         ✦ ⋆        ·  ✦        ⋆ ·              ⋆          ✧
```

**A coding agent for your terminal. Written in Rust.**

*All the good parts of modern coding agents. None of the stuff that wears you down.*

![rust](https://img.shields.io/badge/rust-2021-b7410e?logo=rust)
![binary](https://img.shields.io/badge/one%20static%20binary-no%20runtime-a0a8b2)
![evals](https://img.shields.io/badge/evals-10%E2%80%9312%20of%2012%20by%20mode-ec7a34)
![tests](https://img.shields.io/badge/tests-111%20passing-ec7a34)
![models](https://img.shields.io/badge/models-any%20OpenAI--compatible-6c757d)
![license](https://img.shields.io/badge/license-MIT-6c757d)

<img src="docs/demo/hero.gif" alt="rusty finds and fixes two bugs in a small shop app, then runs the tests" width="100%">

<sub>A real session (idle pauses trimmed, played back a little faster). rusty finds two real bugs in a discount module, fixes both, and proves it by running the tests.</sub>

</div>

---

## Why rusty

Coding agents are great for the first twenty minutes of a task and shaky for
the next three hours. They forget what you said once the context fills up.
They announce "done" without running anything. They ask before `ls`, then wave
through `rm -rf`. Their memory turns into a junk drawer pasted into every
prompt. And you find out what they cost when the bill arrives.

rusty is built around fixing exactly those problems. It's one static binary
with no telemetry, it works with any OpenAI-compatible endpoint, and by default
it runs on open models hosted by NVIDIA.

## See it work

The model-backed clips in the following sections are real sessions, recorded with
[`scripts/record.py`](scripts/record.py) against the toy store in
[`demo/shop`](demo/shop) (or against rusty's own source). The only edits are trimmed idle pauses and
faster playback. Run them yourself and you'll get the same kind of result.

### The complete terminal UI session

[![Editable drafts, workers and careful review](docs/demo/cli-session.gif)](docs/demo/cli-session.mp4)

**[Watch the full 24-second video](docs/demo/cli-session.mp4)** ·
[Replay the terminal recording](docs/demo/cli-session.cast)

One continuous terminal session: read a Rust file, observe two failing tests,
edit the helper, pass both tests, keep an unsent draft, run a read-only worker
and a three-worker swarm, perform the single careful checker, then interrupt
and send a new request. The geometric panel shows each stage and partial worker
failure while the input stays editable beneath it.

This UI recording uses scripted responses from a local SSE provider. File
editing, Bash, worker execution, review and keyboard input run through the actual
Rusty binary; it demonstrates terminal behavior, not live model quality or hosted
Daytona connectivity. Its driver is [`tests/tty/demo.py`](tests/tty/demo.py).

### A swarm, when one pair of eyes isn't enough

<img src="docs/demo/swarm.gif" alt="four parallel workers audit four modules and return a ranked bug list" width="100%">

`/agents auto`, then one sentence. rusty starts four read-only workers, one per
module, each with its own context. It shows each one finishing, then merges
their reports into a ranked list. The two real bugs come out on top.
<sub>(Sped up 2×.)</sub>

### `/goal`: keep going until it's actually done

<img src="docs/demo/goal.gif" alt="goal mode plans, writes a CLI and its tests, runs them, and closes with evidence" width="100%">

Goal mode plans, writes `shop/cli.py` and its tests, and runs them. It also
checks the edge cases by hand. It can only finish by calling `goal_done` with
evidence, so "I think it works" doesn't count.

### `/compact`: a summary that doesn't forget

<img src="docs/demo/compact.gif" alt="compaction shrinks 12.6k tokens to 864 and the next answer still knows the details" width="100%">

rusty reads two of its own source files (12.6k tokens). `/compact keep how the
working set is built` shrinks that to **864 tokens** and saves three lasting
facts to memory. The follow-up question is answered correctly **without
reading a single file again**. Your messages and a list of files changed and
commands run are carried over verbatim, so the summary can't lose them.

### Focus mode, memory, permissions and token honesty

<img src="docs/demo/focus.gif" alt="adhd view gives a three-bullet answer; memory, permission check and token breakdown" width="100%">

`/view adhd` gets you one status line and an answer in three bullets.
`/remember` saves a lesson for future sessions. `/permissions check` shows the
classifier at work: `rm -rf` asks first while `cargo test` just runs.
`/tokens` shows where every token went.

## Everything else

| | |
|---|---|
| **High-recall context** | Fixed token budgets. Old tool output gets trimmed first, because it can be re-read, and only then are older turns summarised. Your messages, the plan and the goal survive every compaction. |
| **Real memory** | Typed records (`preference`, `fact`, `decision`, `gotcha`), ranked with BM25 against each request and injected within a budget. Not a file pasted into every prompt. |
| **Permission classifier** | Builds, installs, feature-branch pushes and read-only cloud commands just run. Pushes to main, deploys and anything unclear ask. Force pushes, cloud and database deletes and `terraform destroy` need a typed yes every time, even in yolo. |
| **Execution modes** | `careful` has a read-only checker review finished work, `vibe` moves fast with a few read-only workers, `standard` sits in between. Separate from permissions. |
| **Infrastructure harness** | Knows where it's pointed: the banner and `/target` show the kube context, cloud account, Terraform workspace and branch, and a production target makes auto pick careful. In careful mode there's no apply without a diff or plan from the same session, a snapshot of what a change touches is taken before it runs, and the rollout is checked after. Every infra command goes into an audit log, and `/changes` shows what the session changed. See [docs/INFRA.md](docs/INFRA.md). |
| **Secrets stay out of the transcript** | Tool output is redacted before the model, the screen or a session file sees it. |
| **`/loop`** | `/loop 5m check CI and fix failures`, or let rusty set its own pace. |
| **Subagents and swarms** | `/agents sub\|swarm\|auto`. Swarm workers can rotate models and spread their temperatures. Off by default, so you never pay for tokens you didn't ask for. |
| **Skills** | `/review` `/explore` `/feature` `/debug` `/test` `/commit`, plus your own markdown skills per project or per user. |
| **Terminal-native tools** | `search` (ripgrep), `glob`, `outline` (definitions with line numbers), ranged `read_file`, exact `edit_file`, `bash`. Find first, then read only what you need. |
| **Token tips** | After a turn that spent heavily, one specific tip based on what actually happened, never generic nagging. |
| **Instant Ctrl-C** | Stops a turn mid-stream in about 15 ms, and the next turn works. Type while it works; Enter stops and sends, Esc keeps your draft. Press Ctrl-C twice to quit. |
| **Looks good** | Rust-and-steel themes, a starfield banner with today's mission line, a steady rotating Braille knot with event pulses, grouped tool lines, rendered markdown. |

## Quickstart

```bash
git clone https://github.com/jadenfix/rusty && cd rusty
cp .env.example .env        # add NVIDIA_API_KEY (free at build.nvidia.com)
cargo install --path .
```

```bash
rusty                                         # interactive
rusty "why does the build fail?"              # one shot
rusty --goal "get cargo test green" --yolo    # unattended until verifiably done
rusty -c                                      # resume the last session here
```

Keys come from your shell, then `~/.config/rusty/.env`, then the `.env` next
to rusty's `Cargo.toml`. With several keys set (`NVIDIA_API_KEY`,
`NVIDIA_API_KEY_2`, …), rusty rotates through them and backs off politely
when rate limited.

## Commands

| | |
|---|---|
| `/goal <objective>` | autonomous until done and verified · `status` · `resume` · `clear` |
| `/loop [5m] [x10] <prompt>` | repeat on an interval, or self-paced |
| `/agents off\|sub\|swarm\|auto` | delegation · `/agents model <id>` sets the subagent model |
| `/swarm size\|models\|spread` | worker count (up to 64), model rotation, temperature spread |
| `/compact [focus]` | summarise now, keeping what you name in detail |
| `/target` · `/audit [n]` · `/changes` | where infra commands will land · the infra audit log · what this session changed |
| `/triage` `/change` `/postmortem` | infra skills: incident triage, a change plan with rollback, a blameless write-up |
| `/context` · `/tokens` | where the window is going · spend by model and by tool |
| `/memory` · `/remember` · `/forget` | long-term memory |
| `/permissions [mode]` | `read-only` `ask` `auto` `yolo` · `allow` · `deny` · `check <cmd>` |
| `/mode auto\|careful\|standard\|vibe` | how hard rusty thinks and checks its work |
| `/tools [local\|daytona]` | show tool placement; save the default for the next session |
| `/view default\|verbose\|adhd` | how much you see |
| `/theme` · `/font` | `calm` `rust` `neon` `matrix` `amber` `ice` `mono` · banners: `minimal` `rust` `block` `thin` `classic` |
| `/review` `/explore` `/feature` `/debug` `/test` `/commit` | built-in skills · `/skills` lists yours too |
| `/model` · `/models` · `/plan` · `/tips` · `/settings` · `/clear` · `/exit` | |

## Execution modes

How hard should rusty think, and how many times should it check its work?
That's separate from what it's allowed to do.

| | |
|---|---|
| `auto` | The default. rusty picks one of the three below for each request and says why on the first line: careful when it mentions production, a migration, a deploy, data, money, security, infrastructure or an incident; vibe for a quick prototype or sketch; standard otherwise, including whenever it's unsure. During a turn it only ever moves up to careful, when a consequential command comes up or three commands fail in a row. It never changes permissions or the model. |
| `careful` | Thinks longer. When it says it's done, a separate read-only checker looks at the real files and your instructions and reports what's wrong. rusty fixes that and verifies it, with no second review. A plain question isn't checked, and a goal gets one checker attempt, including after saving and resuming. |
| `standard` | Normal effort, and it checks what it changed. |
| `vibe` | Quick passes and small checks. It can send out up to three read-only workers to look things up in parallel. It still has to run a relevant check before it calls something done. |

```bash
rusty --mode careful --goal "move the sessions table to the new schema without losing rows"
rusty --mode vibe "sketch a settings page"
```

Switch with `/mode auto|careful|standard|vibe`; the choice is saved, and an
explicit mode always beats auto. `--mode` or `RUSTY_MODE` sets it for one
run. To use a different model per mode, set
`RUSTY_CAREFUL_MODEL`, `RUSTY_STANDARD_MODEL` or `RUSTY_VIBE_MODEL`
(`--model` and `/model` still win). If you choose `/agents` settings
yourself, they stick; `/agents default` hands delegation back to the mode.

On the default NVIDIA model, careful and vibe also set the model's
reasoning budget. In a quick test, the low budget clearly cut reasoning,
but asking for "high" made no difference on easy questions. Whether careful
mode beats standard on real tasks is still being measured; see the evals
below.

All profiles stop a turn after four consecutive identical tool calls return
unchanged results. Remaining tools in that batch are skipped and the goal stays
open. A different call or changed output resets the counter; JSON formatting
changes do not. This bounds exact-call loops, not every possible lack of progress.
It adds no model request, checker or dependency.

## Permissions

Built for letting an agent run on its own, with a hard stop for anything
you can't take back.

```
                 reads, searches,   project edits,     pushes to main,    force push, cloud or
                 cloud reads        builds, installs,  deploys, PRs,      database deletes,
                                    feature pushes     anything unclear   rm -rf outside, disks
read-only            run               refused            refused            refused
ask                  run               asks               asks               needs you, every time
auto                 run               run                asks               needs you, every time  ← default
yolo                 run               run                run                needs you, every time
```

- **Runs on its own in auto:**
  - building, testing, linting and installing project dependencies;
  - deleting build folders (`rm -rf node_modules dist target`);
  - everyday git (commit, branch, rebase, pull, push a feature branch);
  - calls to your local dev server, and plain downloads;
  - read-only cloud commands: `aws … describe/list/get`, `kubectl get/logs/describe`, `terraform plan`, `gcloud … list`, `gh pr view`, `docker ps`, `SELECT` queries.
- **Asks first:**
  - pushing to `main` or another shared branch;
  - `git reset --hard` and anything else that throws away uncommitted work;
  - deploys, `terraform apply`, `kubectl apply`;
  - opening or merging pull requests;
  - writes outside the project;
  - reading `~/.aws/credentials` and other keys;
  - sending local data off the machine;
  - commands rusty doesn't recognise.
- **Needs a typed `yes` every time, even in yolo:**
  - force pushes and deleting remote branches or history;
  - `terraform destroy`;
  - cloud deletes (`aws s3 rm`, `kubectl delete`, `gcloud … delete`, `fly apps destroy`);
  - `DROP`, `TRUNCATE`, and `DELETE` without `WHERE`;
  - `prisma migrate reset`;
  - `rm -rf` outside the project;
  - disk writes.

  An allow rule can't skip this. With nobody at the keyboard (a script, CI, a benchmark), it's refused, and the model is told to hand the command to a person.

rusty looks at what a command will really run, not just its first word:
- the scripts it calls, `package.json` scripts, `make` targets;
- heredocs piped into a shell, and `$(…)` substitutions;
- commands sent through `ssh`, `docker exec` and `kubectl exec`;
- code passed to `python -c`.

So `npm run deploy` is judged by what `deploy` does.

`/permissions check <command>` shows the verdict and why. Deny rules always
win. At a prompt, `a` allows that kind of call from then on, and typing
an empty answer declines. Other text goes back to the model as feedback.

## Verified end to end

The live rows use the real model and a real terminal, and every eval check
is hidden from the agent. The offline tests drive the real binary against a
scripted stand-in for the model, so they're free and repeatable.

| What | Result |
|---|---|
| `scripts/qa.sh`: fmt, clippy `-D warnings`, 93 offline Rust tests (including the real binary and memory service), 6 memory smoke cases, 7 cloud launcher tests | pass |
| Live end-to-end against the model: bug fix, goal mode, swarm, Ctrl-C exit code | 4/4 |
| Real pseudo-terminal: Ctrl-C during a streaming turn | stops in ~10-70 ms, next turn works, double Ctrl-C quits |
| 12 evals in each mode: Python, Rust and JS fixes, code search, a cross-file rename, a CLI flag, a change of plan midway, memory across sessions, a rule surviving compaction, undo one step, a long `/goal`, `/compact` with a focus | careful 12/12 · standard 11/12 · vibe 11/12 · auto 10/12 |
| Inside a FullStack-Bench task container under Harbor | installs, works the task with the environment's own CLI, closes its goal with evidence |

These are single runs of each task on the same model, so a difference of
one or two tasks between modes isn't meaningful yet. Runs that the API's
rate limit cut off were rerun, and every miss was read from its logs:
- standard and auto each went in circles on one task until the time limit;
- vibe closed one goal that failed the hidden checks;
- auto ran out of time on another goal even after switching itself up to careful.

Careful used about 2.5 times the tokens of standard on the tasks both passed.

```bash
scripts/qa.sh            # offline checks
scripts/qa.sh --live     # plus live tests, the terminal check and all 12 evals
scripts/eval.sh multi    # just the multi-turn evals; logs land in target/evals/
```

## Models

Anything your endpoint serves, chosen with `--model`, `RUSTY_MODEL` or `/model`.

| Model on NVIDIA's API | |
|---|---|
| `nvidia/nemotron-3-super-120b-a12b` | **default**, fast and dependable with tools (every demo above) |
| `nvidia/nemotron-3-ultra-550b-a55b` | bigger, slower, thinks harder |
| `z-ai/glm-5.3` | strong coder, slow to first token |
| `openai/gpt-oss-20b` | small and quick; a good swarm worker |

To use another endpoint, set `RUSTY_BASE_URL`, for example `http://localhost:11434/v1`.

## Run it anywhere

Keep reasoning and memory local while executing the entire swarm's tools in
Daytona. `hybrid` uses the pinned Daytona SDK and attaches to an already
started sandbox; it never creates or wakes one. The snapshot must contain
this version of rusty and the remote workspace must already exist.

```bash
cargo build --bins
uv run cloud/daytona.py hybrid --sandbox <id> --workspace /home/daytona/work \
    --mode standard --agents swarm --swarm-max 3 --memory-mode on \
    --prompt "Use three workers to inspect the project, then fix and verify the bug."
```

`--tools local|daytona` (or `RUSTY_TOOLS`) overrides the saved default.
`/tools` and `/settings` show the current location; `/tools local|daytona`
changes the next session, so a goal never changes workspaces halfway through.
The hybrid launcher supplies the ephemeral local SDK connection for
`--tools daytona`. Swarm workers remain read-only and share the lead's remote
checkout; only the lead edits. No model credentials are copied to the sandbox.
Cloud compute and model usage still consume credits. Full setup, lifecycle,
placement choices and evidence are in [docs/CLOUD.md](docs/CLOUD.md).

The core CLI is one static binary. `docker build --target bin -o out .` produces
`rusty` and the optional `rusty-memoryd` companion for Linux.
[docs/MEMORY.md](docs/MEMORY.md) covers opt-in `--memory off|on|deep`, the
RAM-only L1, packed L2, learning from feedback, live checks and cloud transfer.
[docs/ARCHITECTURE_AUDIT.md](docs/ARCHITECTURE_AUDIT.md) records the storage,
dependency and privacy tradeoffs. Redistributed binaries include `LICENSE` and
`THIRD_PARTY_NOTICES.txt`.
[docs/CLOUD.md](docs/CLOUD.md) covers Daytona sandboxes
(`uv run cloud/daytona.py --repo … --goal …`), AWS, Devbox and Docker.
rusty also plugs into Harbor benchmarks with `--goal`, `--trajectory` and
`--stats`.

## Inside

```
  prompt · /goal · /loop
            │
  ┌─────────▼──────────── agent loop ───────────┐      ┌──────────────┐
  │ mode     auto ▸ careful · standard · vibe   │ ◀──▶ │ model, SSE   │
  │ context  budgets · elision · /compact       │      │ key rotation │
  │ memory   BM25 recall                        │      └──────────────┘
  └─────┬────────────────────────────────┬──────┘
        ▼                                ▼
  permissions ▸ infra ▸ tools        read-only workers
  runs · asks   target   bash        subagent · swarm
  · needs you   audit    files       careful checker
```

```
src/
  main.rs         CLI, REPL, slash commands, settings
  agent.rs        the loop: model ⇄ tools, goal, loop, subagents and swarms, compaction
  display.rs      grouped tool lines, status line, recaps, views
  llm.rs          streaming client on a background thread, tool-call assembly, key rotation
  tools.rs        read, write, edit, list, search, glob, outline, bash
  permissions.rs  the command classifier
  memory.rs       memory store and BM25 retrieval
  context.rs      budgets, elision, compaction, the working set
  skills.rs       built-in and user skills
  tips.rs         token tips from real usage
  markdown.rs     terminal markdown
  ui.rs           themes, banner, mission line
  footer.rs       steady activity strip, defined rules and rotating Braille knot
  execution.rs    careful, standard and vibe profiles, and auto's pick
  infra.rs        live target, dry-run gate, snapshots, audit log, redaction
```

Why it's built this way: [docs/DESIGN.md](docs/DESIGN.md) covers where coding
agents hold up on long tasks, where they fall apart, and what rusty does about
each.

## Contributing

Conventional Commits, and pull requests written by a person, for people. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## Terminal appearance

The status panel stays beneath the transcript on the normal terminal screen;
completed output enters ordinary scrollback. A small three-dimensional Braille
trefoil rotates between crisp horizontal rules; the labels stay in fixed cells. Terminals
narrower than 31 columns or shorter than eight rows use one compact line. `RUSTY_NO_ANIM=1`
keeps the sculpture still and disables pulses; `NO_COLOR=1` and pipes use plain output. No animation
thread or full-screen terminal framework is involved. Unicode cell measurement
uses the same `unicode-width` version already used by rustyline.

New installations use the warm `calm` theme and a small `minimal` banner.
Existing saved choices stay in effect. Switch appearance and save it with:

```text
/theme calm
/font minimal
```

The strip uses an 88-cell maximum width, separate status and hint colors, and a
six-row knot rendered on a fixed 32×24-dot canvas. Below 60 columns or 20 rows
it uses a smaller three-row strip. Long status text ends in an ellipsis.
Tool starts briefly expand the sculpture; successful results give it an ivory
pulse, and failures give it an accent-colored ripple. Test commands and explicit
verification status show `verifying`; replies show `responding`. Shell commands
reuse their existing wait loop to animate, with no added thread or completion
delay. File tools pulse before and after their synchronous execution.

The mode label stays visible, including automatic escalation to `careful`.
Careful uses a slower knot inside a dotted shield. A delegated worker adds an
orbiting satellite; a swarm uses several. The strip shows the actual number of
returned reports, running workers, failures and tool calls, plus numbered worker
markers (`●` running, `✓` returned, `×` failed). Delegation is read-only.
The single careful checker has its own `checker` stage with a scanning satellite;
this changes presentation and does not add reviews. Reduced motion retains the
same mode, stage and worker information.
`/theme` lists the other palettes; `/font` selects banner lettering only.

Normal text uses your terminal's installed monospace font. CSS and web fonts do
not apply to a CLI. In macOS Terminal, choose Settings → Profiles → Text → Font;
in iTerm2, choose Settings → Profiles → Text. Menlo at 14–16 points is an available
macOS starting point; choose any installed monospace font and size you prefer.
Rusty does not change the terminal's global settings or install fonts.

The interactive prompt supports local slash-command and argument hints, recent
history hints, and filename completion (including `@path`). Tab accepts a hint or
completes a path; Up/Down and Ctrl-R use the line editor's history. Suggestions do
not call a model. `/suggestions off` disables ghost hints and `RUSTY_NO_SUGGEST=1`
disables them for a run; explicit Tab completion remains available.

While a turn works, an editable draft sits beneath the activity strip. Enter
stops the current turn and sends the draft through the normal command dispatcher;
Esc or Ctrl-C stops without sending it. A draft left unsent returns to the normal
editor when the turn ends. It stays in RAM until you submit it. Approval prompts
own input separately and cannot consume that draft as an approval answer.
The busy editor supports grapheme-aware arrows/backspace, Home/End, Ctrl-A/E,
Ctrl-U/K/W, yank (Ctrl-Y), and one undo step (Ctrl-_). Idle input retains rustyline's
full editing and history search. Busy drafts have a 64 KiB limit; their one-row
viewport keeps the caret visible and displays newlines as `↵`.

Use Ctrl-J or Alt-Enter for a newline, or backslash + Enter to continue a prompt.
Bracketed multiline paste waits for Enter. Canceling a multiline prompt clears it
without sending any partial message. At an empty idle prompt, Ctrl-C twice quits.
With `NO_COLOR`, the busy composer is disabled: interrupt first, then edit at the
ordinary prompt. The line editor and command completion still work.

To capture and inspect the real CLI without model credentials:

```bash
python3 tests/tty/capture.py --binary target/debug/rusty --output /tmp/rusty-tty
python3 tests/tty/inspect.py /tmp/rusty-tty
python3 tests/tty/input.py --binary target/debug/rusty --output /tmp/rusty-input
python3 tests/tty/input_screen.py /tmp/rusty-input
python3 tests/tty/demo.py --binary target/debug/rusty --output /tmp/rusty-demo
python3 tests/tty/modes.py --binary target/debug/rusty --output /tmp/rusty-modes
python3 tests/tty/modes.py --binary target/debug/rusty --reduced-only --output /tmp/rusty-modes-static
```

The captures exercise real file writes, Bash results, streamed replies, and
interactive Ctrl-C recovery through an `expect` PTY at 24/40/80/120 columns.

## License

MIT
