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
![binary](https://img.shields.io/badge/one%20static%20binary-~5k%20lines-a0a8b2)
![evals](https://img.shields.io/badge/evals-12%2F12-ec7a34)
![models](https://img.shields.io/badge/models-any%20OpenAI--compatible-6c757d)
![license](https://img.shields.io/badge/license-MIT-6c757d)

</div>

```text
♎ libra season · ☾ waning crescent  “saturn asks for a regression test.”

› the date parser drops timezones. fix it and prove it.

◆ explored · searched /parse_date/ · outlined time.rs · read time.rs
▸ edit src/time.rs
  └ edited src/time.rs (1 replacement)
▸ $ cargo test time::
  └ exit 0 · 14 lines

Fixed: parse_date now keeps the offset instead of forcing UTC (src/time.rs:52).
Added a test for +05:30 input. cargo test time:: passes (6 tests).

✓ 1 read · 2 lookups · 1 edit · 1 command · 19s · ↑18.2k ↓1.1k · ctx 9%
```

## Why rusty

Coding agents are great for the first twenty minutes of a task and shaky for
the next three hours. They forget what you said once the context fills up.
They announce "done" without running anything. They ask before `ls` and then
wave through `rm -rf`. Their memory turns into a junk drawer that gets pasted
into every prompt. And you only find out what they cost when the bill shows
up.

rusty is a small agent built around fixing exactly those problems. It's one
static binary with no telemetry, and it works with any OpenAI-compatible
endpoint. By default it runs on open models hosted by NVIDIA.

## What you get

| | |
|---|---|
| **High-recall context** | Fixed token budgets. Old tool output gets trimmed first, because it can be re-read, and only then are older turns summarised. Your messages, the plan and the goal survive every compaction **word for word**. |
| **`/compact [focus]`** | Summarise on demand, steered by what you care about: `/compact keep the auth bug details`. A working set of files changed, files read, recent commands with exit codes and the last error is rebuilt from the tool log, so the summary can't lose it. |
| **Real memory** | Typed records (`preference`, `fact`, `decision`, `gotcha`), ranked with BM25 against each request and injected within a budget. Not a markdown file pasted into every prompt. |
| **Permission classifier** | `ls`, `git diff` and `cargo test` just run. `rm -r`, `git push`, `curl \| sh` and writes outside the project ask first. Four modes plus your own allow and deny rules. |
| **`/goal`** | Works across as many turns as it takes, and only stops by calling `goal_done` with evidence of verification. |
| **`/loop`** | `/loop 5m check CI and fix failures`, or let rusty set its own pace. |
| **Subagents and swarms** | `/agents sub` for one read-only researcher, `/agents swarm` for many in parallel (each with its own model and temperature), `/agents auto` to let rusty choose. Off by default, so you never pay for tokens you didn't ask for. |
| **Skills** | `/review`, `/explore`, `/feature`, `/debug`, `/test`, `/commit`, plus your own markdown skills per project or per user. |
| **Terminal-native tools** | `search` (ripgrep), `glob`, `outline` (definitions with line numbers), ranged `read_file`, exact `edit_file`, and `bash`. Locate, then read only what you need. |
| **Token honesty** | A recap after every turn, `/tokens` broken down by model and by tool, per-request traces on disk, and tips based on what each turn actually spent. |
| **Instant Ctrl-C** | Stops a turn mid-stream in milliseconds. The transcript stays valid and the next turn works. Press Ctrl-C twice at the prompt to quit. |
| **Looks good** | Rust-and-steel themes, a starfield banner, today's sky and a coding horoscope, a star spinner with rotating verbs, live grouped tool lines, rendered markdown. `default`, `verbose` and `adhd` views. |

## Quickstart

```bash
git clone https://github.com/jadenfix/rusty && cd rusty
cp .env.example .env        # add NVIDIA_API_KEY (free at build.nvidia.com)
cargo install --path .
```

```bash
rusty                                         # interactive
rusty "why does the build fail?"              # one shot
rusty --goal "get cargo test green" --yolo    # unattended until it's verifiably done
rusty -c                                      # resume the last session here
```

Keys come from your shell, then `~/.config/rusty/.env`, then the `.env` next
to rusty's `Cargo.toml`. With several keys set (`NVIDIA_API_KEY`,
`NVIDIA_API_KEY_2`, …), rusty rotates to the next one when it hits a rate
limit or a server error.

## Commands

| | |
|---|---|
| `/goal <objective>` | autonomous until done and verified · `status` · `resume` · `clear` |
| `/loop [5m] [x10] <prompt>` | repeat on an interval, or self-paced |
| `/agents off\|sub\|swarm\|auto` | delegation · `/agents model <id>` sets the subagent model |
| `/swarm size\|models\|spread` | worker count (up to 64), model rotation, temperature spread |
| `/compact [focus]` | summarise now, keeping what you name in detail |
| `/context` · `/tokens` | where the window is going · spend by model and by tool |
| `/memory` · `/remember` · `/forget` | list, search and edit long-term memory |
| `/permissions [mode]` | `read-only` `ask` `auto` `yolo` · `allow` · `deny` · `check <cmd>` |
| `/view default\|verbose\|adhd` | how much you see; `adhd` is one status line and short answers |
| `/theme` · `/font` | `rust` `neon` `matrix` `amber` `ice` `mono` · `rust` `block` `thin` `classic` |
| `/review` `/explore` `/feature` `/debug` `/test` `/commit` | built-in skills; `/skills` lists yours too |
| `/model` · `/models` · `/plan` · `/tips` · `/settings` · `/clear` · `/exit` | |

## Permissions

```
               read-only cmds    project edits,      rm -r, push, network,
               reads, search     build and test      leaving the project
read-only          run              denied               denied
ask                run              asks                 asks
auto               run              run                  asks        ← default
yolo               run              run                  run
```

Deny rules always win. At a prompt, `a` allows that kind of call for good, and
typing anything else sends it to the model as feedback.
`/permissions check <command>` shows how any command is classified.

## Models

Anything your endpoint serves, chosen with `--model`, `RUSTY_MODEL` or `/model`.
On NVIDIA's API:

| Model | |
|---|---|
| `nvidia/nemotron-3-super-120b-a12b` | **default**, fast and dependable with tools |
| `nvidia/nemotron-3-ultra-550b-a55b` | bigger, slower, thinks harder |
| `z-ai/glm-5.3` | strong coder, slow to first token |
| `openai/gpt-oss-20b` | small and quick; good as a swarm worker |

To use a different endpoint, set `RUSTY_BASE_URL`, for example `http://localhost:11434/v1`.

## Quality

```bash
scripts/qa.sh            # fmt, clippy -D warnings, 24 unit + 10 end-to-end tests
scripts/qa.sh --live     # plus live model tests, a real-terminal ctrl-c check, and the evals
scripts/eval.sh          # 12 held-out tasks; hidden checks the agent never sees
```

The latest run on the default model:

| Suite | Result |
|---|---|
| single-turn tasks: Python, Rust, JS, code search, a cross-file rename, a CLI flag | 6/6 |
| multi-turn: mid-task pivot, memory across sessions, a rule surviving compaction, undo one step, a long `/goal`, `/compact <focus>` | 6/6 |
| live end-to-end: bug fix, goal mode, swarm, Ctrl-C | 4/4 |
| real terminal: Ctrl-C stops a streaming turn | ~1 ms, then the next turn works |

## Run it anywhere

One static binary. `docker build --target bin -o out .` produces it for Linux.
[docs/CLOUD.md](docs/CLOUD.md) covers Daytona sandboxes
(`uv run cloud/daytona.py --repo … --goal …`), AWS, Devbox and Docker.
rusty also plugs into Harbor-based benchmarks with `--goal`, `--trajectory` and
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

Why it's built this way: [docs/DESIGN.md](docs/DESIGN.md) covers where
coding agents hold up on long tasks, where they fall apart, and what rusty
does about each.

## Contributing

Conventional Commits, and pull requests written by a person for people. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
