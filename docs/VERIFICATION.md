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
