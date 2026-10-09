# Fixed goal verification and repair experiments

The first repair-architecture increment gives each strategy the same acceptance
boundary. It is opt-in, local-only, and deliberately small:

```sh
rusty --goal 'repair the pagination boundaries' \
  --verify 'python3 -m pytest tests/test_pages.py' \
  --verify-timeout 120 --mode standard
```

`goal_done` proposes completion. The coordinator waits for every tool in the
reply, performs the existing careful review when required, and runs the fixed
user-selected command through the ordinary permission policy. A passing shell
exit plus unchanged source-input fingerprints can close the goal. Failed,
timed-out, interrupted, denied, or changing-input verification leaves it open.
Blocked goals do not run checks or spend a reviewer call. A headless goal with
`--verify` returns 2 when unfinished and 130 when interrupted.

The model cannot replace the pinned command with an evidence string or a
different command argument in `goal_done`. Checks from earlier tools and
reviewer prose are not used as acceptance receipts. Every proposal is checked
afresh. A completion proposal followed by an edit in the same reply is checked
against that edit, including in standard and vibe mode. Without `--verify`, the
legacy goal policy remains available for conversational and read-only tasks.

## Evidence and scope

The private `CheckRecord` fields are constructed from executor observations.
The record serializes for diagnosis but has no deserialization implementation.
The saved criterion is revalidated on resume, permissions apply again, and
historical records are ignored as authority. Session and trajectory records
include the fixed-command digest, before/after input digests, actual exit status,
outcome, elapsed check time, and bounded redacted output with a digest of that
captured output. The enclosing goal supplies the objective. Missing manifests
or failed reads do not create passing records.

The input digest covers the canonical workspace identity, inherited environment,
file contents, paths, and Unix permission modes. Environment values and file
contents are hashed rather than exported. It includes untracked files and
lockfiles. Symlinks and special files are rejected. Enumeration is bounded at
50,000 entries and 128 MiB of contents; exceeding either limit leaves the goal
unverified rather than silently dropping inputs.

`.git`, `target`, `__pycache__`, and `.pytest_cache` are excluded. These generated
artifacts often change during a check. This is a **source-input manifest**, not a
digest of a container image, dependencies, compiled outputs, or external
services. The chosen command must rebuild/invalidate caches when necessary.
For example, Python timestamp bytecode can reuse an old same-length module
after a same-second edit. The regression fixtures disable bytecode generation;
the repair eval uses inline source execution to avoid that confound.

Before/after hashes detect boundary changes, not transient mutation and restore,
nor an external writer after the final scan. Verification currently executes in
the local workspace, not an immutable OS-isolated clone. Native checks have the
permissions of their process; Rust field privacy and process groups do not
sandbox hostile code. Independently grading the final patch remains necessary.
An editable test can be weakened, and a successful command can have poor
coverage. Inline criteria or a protected checker reduce criterion tampering;
this release does not infer test quality or guarantee the whole objective.

Checks cannot route a mutating infrastructure action around the existing
dry-run/snapshot/health-check harness. Remote tool placement with `--verify` is
rejected explicitly; there is no silent local fallback or unqualified remote
completion receipt.

Shell commands have owned Unix process groups. Ordinary descendants are killed
when the shell exits or its deadline/interrupt fires, before collecting pipes.
Output is continuously drained while retaining bounded head/tail capture.
Background jobs in the same group therefore do not outlive a shell call.
Processes that deliberately escape their group need an OS sandbox; group
cleanup is not a containment proof.

## Evaluations

Offline Rust integration tests drive the real CLI with scripted SSE replies.
They cover correct repair, unchecked/failed/stale claims, a post-proposal edit
in one batch, verification that changes inputs, permission denial, deadline,
blocked completion, and careful-review ordering. Unit checks cover manifest
changes, symlink rejection, imported invalid criteria, output bounds and
descendant cleanup. These measure runtime contracts, not model coding quality.

The `verified-repair` development task uses `verify.txt` as its visible fixed
acceptance criterion. `cargo xtask eval` passes it as `--goal ... --verify ...`;
only `files/` is copied into the working project. The hidden grader remains
outside it and independently tests negative/zero/large integers and keyword
arguments. Oracle and three wrong-patch controls qualify the grader locally.
The host grading setup is evaluator separation, not an OS sandbox.

```sh
cargo xtask qa
EVAL_MODELS=nvidia/nemotron-3-super-120b-a12b EVAL_REPEATS=1 EVAL_JOBS=1 \
  EVAL_TIMEOUT=120 cargo xtask eval verified-repair
```

The second command starts a live provider run and needs its configured key.
Results go to `target/evals/`. A public one-function development task validates
the completion path; it does not establish repository-repair performance.
Existing eval scoring independently grades the patch regardless of the model's
claim. The new unfinished exit code is for direct CLI automation; grading
functional correctness and classifying goal reporting remain separate metrics.

## Combined architecture and next increments

[Agentless](https://arxiv.org/abs/2407.01489) motivates explicit localization,
repair, and validation; [MASAI](https://arxiv.org/abs/2406.11638) motivates bounded
specialized roles. Combine that repair path with structural navigation inspired
by [RepoGraph](https://arxiv.org/abs/2410.14684), alternative hypotheses inspired
by [SWE-Search](https://arxiv.org/abs/2410.20285), and outcome lessons inspired by
[Reflexion](https://arxiv.org/abs/2303.11366). This is a synthesis to evaluate,
not a reproduction of those systems or a claim of additive gains.

Build subsequent increments in this order:

1. Durable requirements, shared request/token reservations, registered wait
   handles, action intent/outcome recovery, and remote reconciliation.
2. A staged strategy around the existing lead loop: reproduce, localize,
   patch, verify, review. Avoid a workflow framework or mandatory extra calls.
3. Hash-versioned structural context and verified episode advice. Advice
   cannot authorize actions; isolate cold/warm episode cohorts and held-out data.
4. Two candidates from a common checkpoint with distinct hypotheses,
   isolated writable execution, independent grading, and final revalidation
   after applying the selected candidate. Git worktrees alone are not isolation.
5. Frozen sandbox verification and the same evidence protocol for remote tools.

For architecture evaluation, freeze one common source baseline and model/cohort.
Compare baseline, gate, staged, structural retrieval, episodes, search, and the
hybrid. Match total specialist/retry budgets; independently score actual patches
and count false completion separately. Four development tasks × five repeats ×
four initial variants is an 80-attempt pilot; seven variants gives 140 attempts.
Neither cohort is implemented or executed by this PR. Require artifact hashes,
sandbox qualification, and a spending envelope before admission, followed by a
held-out real-repository suite. Do not mix those results with scripted-runtime
tests or promote a tiny public fixture into a benchmark claim.

## Owned local waits

The next increment adds three lead-only tools: `bash_start`, `bash_wait`, and
`bash_cancel`. Use ordinary `bash` for short work. For a longer command, start
it once, inspect something useful, then wait on the returned handle. The handle
belongs to this coordinator; a PID, a saved receipt, model prose, or another
session's ID cannot construct one. Read-only workers cannot start activities.
Remote placement exposes no activity tools and refuses fabricated calls.

A start goes through the same permission policy as `bash`. Mutating infrastructure
commands are refused on this path so they still use the ordinary snapshot and
verification harness. The lifetime is fixed at start (1–600 seconds, default
120). `bash_wait` waits 0–30 seconds (default five) without extending that lifetime
or replaying the command. Four commands may be active, with at most 32 starts per
session, including cancelled commands. Nonzero waits avoid busy polling.

A registered command may return exactly the same `Running` result for several
polls. This is justified by a live executor handle and its deadline, not by a
changing timestamp. Unknown handles, completed handles, and ordinary repeated
calls still use the stall guard. Running output is withheld; terminal records
contain actual exit status, elapsed time, and bounded redacted stdout/stderr.
Each ordinary process group is terminated/reaped before its final result is
collected, even if its leader exited first.

Completion waits until no owned command is running, before spending a careful
review or executing the fixed acceptance criterion. Activity output alone is
not a passing acceptance receipt. Ending a turn with running commands cancels
them and reports unfinished work; a blocked goal cancels without a review. The
same cleanup runs on ordinary error, Ctrl-C, and destruction of the owner.
Cancellation does not undo effects that already happened. Starting the command
again requires an explicit proposal and authorization.

Sessions and trajectories export diagnostic activity records; loading a session
does not restore handles or replay commands. Live runtime state is carried in
the system prompt independently of compacted conversation text. This increment
is not a crash-recovery journal: abrupt process death, forced quit, and deliberately
escaped process groups need an external supervisor/reconciliation boundary.
The owned-wait increment by itself does not add model admission accounting;
the following increment supplies a process-scoped ledger. Durable recovery and
remote activity reconciliation remain required before writable candidate search.
No general shell isolation is claimed.

### Evaluation workflow

`verified-index` and `verified-invoice` add independently graded cross-file
repairs. The index task includes a slow public checker for exercising a useful
owned wait. The invoice task checks exact decimal rounding, negative amounts,
large values and tax applied to the integer subtotal. Graders remain outside
`files/`; they also reject edits to protected public checkers. Oracle, broken,
plausibly wrong and checker-tampering controls run offline.

```sh
EVAL_MODELS=nvidia/nemotron-3-super-120b-a12b EVAL_REPEATS=5 EVAL_JOBS=1 \
  EVAL_TIMEOUT=120 cargo xtask eval verified-repair,verified-index,verified-invoice
```

For an artifact comparison, `EVAL_BINARY=/absolute/path/to/qualified/rusty`
selects an already built binary. Record its hash, source revision, task hashes,
sampling settings and limits in a separate immutable experiment receipt.
Both variants must use the same frozen task source and grading runner. The
runner now uses unique per-process/nanosecond receipt names, exclusive creation,
and saved trajectories. Functional grading, goal reporting and false completion
are separate fields; a correct patch can still have an unfinished goal.
A known HTTP 402 evaluation-budget stop is scored as `budget`, rather than
excluded as infrastructure. Timed-out processes receive Ctrl-C and a three-second
cleanup grace before a hard kill; this is cooperative cleanup, not crash isolation.

The accompanying development experiment compares merged PR 28 with this activity
increment, five attempts per task and variant, using standard mode, memory and
delegation off. External provider admission counts transport attempts too,
reserves conservative input-byte-plus-output bounds, preserves unknown usage,
and paces requests. No automatic replacements or model substitutions occur.
Provider errors remain in the all-attempt ledger and are excluded from the
healthy functional cohort. Development fixtures and host graders do not establish
held-out repository performance or sandbox containment. A larger search/staging
experiment still needs matched aggregate budgets and qualified execution isolation.


## Shared model admission budget

The client now owns one synchronized ledger across the lead, read-only workers,
careful reviewer, compaction, all models/providers, key rotations and HTTP/stream
retries. Each HTTP POST reserves before dispatch. A role or new goal cannot create
another allowance. `/clear`, `/goal`, `/compact`, mode/model changes and loop turns
all retain the same process budget. Enable bounded execution with any of
`--max-requests`, `--max-budget-tokens`, `--budget-secs` or the matching
`RUSTY_MAX_REQUESTS`, `RUSTY_MAX_BUDGET_TOKENS`, `RUSTY_BUDGET_SECS` variables.
Unspecified limits in a bounded run default to 256 attempts, 4,000,000 admission
tokens and 3,600 seconds of provider time. Zero is rejected. Without any budget
option, legacy execution remains unbounded and its diagnostic snapshot says
`enabled=false`, `limits=null`. Do not present that snapshot as an enforced cap.

The token reservation is the serialized UTF-8 request size plus the actual
provider output cap (`max_tokens` or `max_completion_tokens`). This is deliberately
conservative admission accounting, not an exact tokenizer or a billed-cost cap.
Only a completed response with both valid nonnegative usage counters refunds the
unused reservation. Missing/partial usage, HTTP errors, stream failures and
interruption keep it charged. Known reported usage replaces its reservation;
unexpected excess is charged, marks an overrun and prevents further dispatch or
acceptance of that response. Anthropic accounting includes cached input and
requires final output usage; initial output counters are not treated as final.
A request is refunded only when the provider answers HTTP 429, since it did no
work; `attempts` still counts it. A required careful review that cannot finish within
the budget halts this root; resuming or replacing the goal cannot skip it.
Provider-internal processing/fallbacks are not
separate client HTTP attempts; exact billing remains provider evidence.

The provider deadline bounds retry waits and HTTP response time. The coordinator
also checks it while waiting for a silent stream and before consuming a reply.
An error returns through the normal owned-command cleanup path. This is not a
whole-task wall deadline: tools and the fixed verification command retain their
own deadlines, and verification consumes no model request. Model catalogue GETs
are outside the inference budget. Normal local execution or Daytona reasoning
shares this client ledger; a live Daytona journey has not qualified it.

Stats, trajectories and saved sessions include `model_budget`:

| Field | Meaning |
|---|---|
| `attempts` | every HTTP inference attempt sent, rate-limited ones included: what a proxy or gateway sees |
| `requests` | attempts counted against `--max-requests` (a 429 is given back, see `rate_limited`) |
| `charged_tokens` / `known_tokens` | admission tokens held / usage the provider reported |
| `unknown_usage_requests` | requests with no complete usage report, in flight included |
| `active_requests`, `denied_requests`, `by_role` | in flight now, refused admission, per role |
| `reported_cost_usd`, `cost_reported_requests` | summed from providers that put a price in their usage (OpenRouter's `usage.cost`); a lower bound unless every request reported, and absent prices are never invented |
| `elapsed_seconds`, `deadline_reached`, `overrun`, `halted` | provider time used, and why admission stopped |
| `ledger`, `runs` | the ledger file, if any, and how many processes have used it |

Conversation totals are logical successful reply metrics, not HTTP attempts.

Without a ledger, a new CLI process (including `--continue` or a multi-session
eval) starts a new budget. Set `RUSTY_BUDGET_LEDGER=<path>` to make one episode
budget span restarts: the state is rewritten (beside the file, then renamed)
after every admission, settlement and refusal, and loaded at start. Limits
then apply to the episode, not the process. An attempt that was in flight when
a process died stays charged with unknown usage. Elapsed time counts provider
time while some process was running, not the gaps between processes. The
ledger is locked (`<path>.lock`, released by the kernel if the process dies),
so a second rusty on the same ledger fails at startup. A ledger that can't be
read is an error, never a fresh start. The ledger is the client's own count;
a gateway in front of it remains the authority for what was billed.

Memory's only model request, reflection after a checked goal (`--memory
reflect` and `deep`), goes through the agent's client and this ledger with the
role `memory`; the memory daemon makes no model calls. Other already running
processes are outside this root's scope.

The evaluator now gives timeout, budget and infrastructure outcomes precedence
over functional passing. A nonzero process exit cannot be `pass` either. The
independent patch result stays in `functional_pass`, so a correct patch with a
fatal provider failure remains visible without becoming a healthy success.
Per-process budget snapshots are retained in eval rows, including multi-session
runs; they are not summed into a fictitious shared cross-process allowance.

Run the deterministic runtime and scoring counterexamples before live work:

```sh
cargo xtask qa
cargo test --test cli root_budget
RUSTY_MAX_REQUESTS=20 RUSTY_MAX_BUDGET_TOKENS=200000 RUSTY_BUDGET_SECS=120 \
  EVAL_REPEATS=1 EVAL_JOBS=1 EVAL_TIMEOUT=125 \
  cargo xtask eval verified-repair,verified-index,verified-invoice
```

The last command starts paid/provider work with per-process bounds. A whole
comparison needs a separately declared aggregate request/token admission cap,
frozen artifacts, randomized order and a ledger preserving every attempt. First
run small plumbing smokes and a deliberately tiny allowance that must stop with
an open goal. Require five healthy observations per task/variant before claiming
architectural benefits, with known and unknown usage reported separately.

## Harness contract

What a benchmark harness can rely on when it runs rusty unattended. The
FullStack-Bench adapter is one such harness. Anything not listed here is
display output and may change.

**Invocation.** Pin every setting that has a saved or default value:

```sh
RUSTY_NO_DOTENV=1 RUSTY_HOME=<fresh dir> RUSTY_BUDGET_LEDGER=<episode file> \
rusty --yolo --memory off --agents off --mode standard --model <id> \
  --max-requests N --max-budget-tokens T --budget-secs S \
  --stats --trajectory <path> \
  --goal "<task>" --verify "<check>" --verify-timeout <secs>
```

- **Keys** come from the environment only. `RUSTY_NO_DOTENV=1` stops a stray
  `.env` being read, and a fresh `RUSTY_HOME` keeps saved settings, sessions
  and memory out of the run.
- **Memory and delegation.** `--memory off` starts no memory daemon and reads
  no lessons. `--agents off` runs one agent. Any other value is a different
  experiment.
- **`--verify`** is optional. Without it, the goal closes on the model's word
  and exit status 2 can't happen.
- **MCP.** Servers come from `.mcp.json` in the project, or from
  `RUSTY_MCP_CONFIG`. `rusty --mcp-check` is a preflight: it needs no model
  key and exits 1 if a server marked `required`, or one of its
  `requiredTools`, is missing. A normal run with that config fails at startup
  instead of starting without the tools.

**Exit status.**

| Code | Meaning |
|---|---|
| 0 | finished; with `--verify`, the check passed |
| 2 | `--verify` was given and the goal did not close on a passing check |
| 130 | interrupted |
| 1 | error: bad flags, model budget exhausted, provider failure, required MCP server or tool missing, or an unreadable or locked budget ledger |

**Outputs.**

- **`--stats`** prints one JSON line on stderr at exit with these keys:
  - `requests`, `prompt` and `completion` (successful replies);
  - `model_budget` (the table above);
  - `secs`, `interrupted`;
  - `goal` (`done`, `blocked`, `open` or null);
  - `execution_mode`, `memory_mode`, `tools_location`;
  - `memory` (null when off).
- **`--trajectory`** writes the conversation and the same `model_budget`.
- Count requests against a gateway with `attempts`, and spending with
  `charged_tokens` and `unknown_usage_requests`. Billing evidence comes from
  the provider or gateway: `reported_cost_usd` is only what providers chose
  to report.

**Not claimed.**

- exact tokenization, or a billed-cost cap;
- a qualified live Daytona journey;
- remote (HTTP/SSE) MCP transports, which are skipped;
- that any memory level improves hard tasks.
