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
![evals](https://img.shields.io/badge/evals-12%2F12-ec7a34)
![tests](https://img.shields.io/badge/tests-38%20passing-ec7a34)
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

Every clip below is a real session, recorded with
[`scripts/record.py`](scripts/record.py) against the toy store in
[`demo/shop`](demo/shop) (or against rusty's own source). The only edits are trimmed idle pauses and
faster playback. Run them yourself and you'll get the same kind of result.

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
| **Permission classifier** | `ls`, `git diff`, `cargo test`, `chmod +x ./script.sh` just run. `rm -r`, `git push`, `curl … \| sh` and writes outside the project ask first. Four modes plus your own allow and deny rules. |
| **`/loop`** | `/loop 5m check CI and fix failures`, or let rusty set its own pace. |
| **Subagents and swarms** | `/agents sub\|swarm\|auto`. Swarm workers can rotate models and spread their temperatures. Off by default, so you never pay for tokens you didn't ask for. |
| **Skills** | `/review` `/explore` `/feature` `/debug` `/test` `/commit`, plus your own markdown skills per project or per user. |
| **Terminal-native tools** | `search` (ripgrep), `glob`, `outline` (definitions with line numbers), ranged `read_file`, exact `edit_file`, `bash`. Find first, then read only what you need. |
| **Token tips** | After a turn that spent heavily, one specific tip based on what actually happened, never generic nagging. |
| **Instant Ctrl-C** | Stops a turn mid-stream in about 15 ms, and the next turn works. Type while it works and your message runs next. Press Ctrl-C twice to quit. |
| **Looks good** | Rust-and-steel themes, a starfield banner, today's sky and a coding horoscope, a star spinner with rotating verbs, grouped tool lines, rendered markdown. |

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
| `/context` · `/tokens` | where the window is going · spend by model and by tool |
| `/memory` · `/remember` · `/forget` | long-term memory |
| `/permissions [mode]` | `read-only` `ask` `auto` `yolo` · `allow` · `deny` · `check <cmd>` |
| `/view default\|verbose\|adhd` | how much you see |
| `/theme` · `/font` | `rust` `neon` `matrix` `amber` `ice` `mono` · `rust` `block` `thin` `classic` |
| `/review` `/explore` `/feature` `/debug` `/test` `/commit` | built-in skills · `/skills` lists yours too |
| `/model` · `/models` · `/plan` · `/tips` · `/settings` · `/clear` · `/exit` | |

## Execution modes

Choose how much reasoning and review rusty spends, independently of permissions:

| Mode | Inference on the default NVIDIA Super model | Completion checks | Default delegation |
|---|---|---|---|
| `careful` | high effort, 24,576 reasoning tokens within a 32,768-token output cap | two additional review passes with tools available before accepting completion | off |
| `standard` | provider defaults, existing 16,384-token output cap | relevant tests or direct verification | off |
| `vibe` | low effort, 2,048 reasoning tokens within an 8,192-token output cap | focused smoke checks, no forced extra review | auto, up to three read-only workers |

```bash
rusty --mode careful --goal "repair the migration and check data integrity"
rusty --mode standard "fix the parser regression"
rusty --mode vibe "prototype a settings screen"
```

Use `/mode careful`, `/mode standard` or `/mode vibe` in a session; `/mode`
shows the current mode. Interactive choices persist. `--mode` or `RUSTY_MODE`
overrides the saved choice for one invocation. Explicit `/agents` or `--agents`
choices take precedence over mode defaults.

Set `RUSTY_CAREFUL_MODEL`, `RUSTY_STANDARD_MODEL` and `RUSTY_VIBE_MODEL` to
route each mode to a different model. Explicit `--model`, `RUSTY_MODEL` or
`/model` choices take precedence. The same model is used when none is configured.
The NVIDIA reasoning fields are sent only for the documented default
endpoint and Super model; other models keep their provider's reasoning defaults
and receive the mode's output cap and workflow instructions.

Careful mode enforces two extra inference passes, including when the model
calls `goal_done` early. Reviews can inspect files, run relevant checks, and
repair failures. Blocked goals and interrupted or truncated turns do not
pretend to have completed the reviews. This is additional model review, not
an independent verifier or a guarantee of production safety. Vibe still needs
a relevant check; all modes preserve permission rules and explicit deny rules.

## Permissions

```
               read-only cmds    project edits,      rm -r, push, network,
               reads, search     build, test, run    leaving the project
read-only          run              denied               denied
ask                run              asks                 asks
auto               run              run                  asks        ← default
yolo               run              run                  run
```

Deny rules always win. At a prompt, `a` allows that kind of call for good, and
typing anything else sends it to the model as feedback.

## Verified end to end

Nothing here is a mock. The model is real, the terminal is real, and every
check is hidden from the agent.

| What | Result |
|---|---|
| `scripts/qa.sh`: fmt, clippy `-D warnings`, 24 unit tests, 10 end-to-end tests on the real binary | pass |
| Live end-to-end against the model: bug fix, goal mode, swarm, Ctrl-C exit code | 4/4 |
| Real pseudo-terminal: Ctrl-C during a streaming turn | stops in ~10-70 ms, next turn works, double Ctrl-C quits |
| Single-turn evals: Python, Rust, JS, code search, cross-file rename, CLI flag | 6/6 |
| Multi-turn evals: change of plan midway, memory across sessions, a rule surviving compaction, undo one step, a long `/goal`, `/compact` with a focus | 6/6 |
| Inside a FullStack-Bench task container under Harbor | installs, works the task with the environment's own CLI, closes its goal with evidence |

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

One static binary: `docker build --target bin -o out .` produces it for Linux.
[docs/CLOUD.md](docs/CLOUD.md) covers Daytona sandboxes
(`uv run cloud/daytona.py --repo … --goal …`), AWS, Devbox and Docker.
rusty also plugs into Harbor benchmarks with `--goal`, `--trajectory` and
`--stats`.

## Inside

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
  ui.rs           themes, banner, sky, spinner
```

Why it's built this way: [docs/DESIGN.md](docs/DESIGN.md) covers where coding
agents hold up on long tasks, where they fall apart, and what rusty does about
each.

## Contributing

Conventional Commits, and pull requests written by a person, for people. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
