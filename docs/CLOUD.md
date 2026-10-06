# Running rusty in the cloud

rusty is a single static binary with no runtime dependencies, so it runs
anywhere Linux does. Model keys always travel as environment variables, never
as arguments.

## Daytona

### Choose what runs where

| Entry point | Reasoning and workers | Memory | Files, search, Bash, infra probes |
|---|---|---|---|
| `rusty --tools local` | local | local | local |
| `uv run cloud/daytona.py hybrid ...` | local | local | one Daytona workspace |
| `uv run cloud/daytona.py run ...` | Daytona | Daytona | Daytona |

`--memory off|on|deep` on the CLI, or `--memory-mode` on the launcher,
chooses the memory behavior at the agent's location. `deep` may make extra
model calls; `on` uses local hooks without background inference. The three
reasoning modes and permission modes work in each arrangement.

### Local agent, remote swarm tools

Use this when the model endpoint is reachable from your computer but blocked
by sandbox egress policy. NVIDIA requests originate locally. The sandbox
needs access only to the services its code/build tools actually use; this
does not grant it general internet access.

```text
Local Rusty lead + read-only workers ─── model endpoint
             │
      local memory hooks
             │
  authenticated localhost SDK bridge
             │
     Daytona process API
             │
  Rust tool endpoint in one shared checkout
```

Requirements: Python 3.10+, [uv](https://docs.astral.sh/uv/getting-started/installation/),
a local Rusty binary and its optional memory companion, `DAYTONA_API_KEY`
and model credentials in your local environment, and an **already started**
sandbox with a project checkout and this build of the Linux Rusty binary on
PATH. The launcher automatically installs its pinned Daytona SDK 0.220.0
through uv; the core Rust binary adds no SDK crate or Python dependency.

```bash
cargo build --bins
# Configure credentials in your shell or private .env; never paste them into flags.
uv run cloud/daytona.py hybrid \
    --sandbox <existing-id> --workspace /home/daytona/work \
    --local-binary target/debug/rusty \
    --mode standard --agents swarm --swarm-max 3 --memory-mode on
# Omit --prompt/--goal for the interactive CLI.
# Headless run with a local receipt:
uv run cloud/daytona.py hybrid \
    --sandbox <existing-id> --workspace /home/daytona/work \
    --agents swarm --swarm-max 3 --memory-mode off \
    --prompt "Audit the modules with three read-only workers, then fix and verify." \
    --stats --trajectory hybrid.json
```

If you need a fresh environment, `prepare --source` below builds the current
**committed HEAD**. Create/start a sandbox from the printed snapshot with
Daytona's dashboard or SDK, then clone the project into it. Those operations
consume cloud credits; `hybrid` performs none of them implicitly. Older Rusty
snapshots lack tool protocol v1 and must be rebuilt. The bridge handshake
checks compatibility and resolves the real remote workspace before any model
request. `DAYTONA_TARGET=eu` selects EU as with the full-cloud launcher.

Placement controls:

- CLI `--tools local|daytona` and `RUSTY_TOOLS` override the saved default.
  The SDK launcher supplies the localhost connection required by `daytona`.
- `/tools` and `/settings` show the current backend and remote workspace.
  `/tools local|daytona` saves the default for the **next** session, without
  moving active work. Missing bridge settings fail clearly, with no local fallback.
- `--agents off|sub|swarm|auto` controls delegation; `--swarm-max 1..8` limits
  concurrency. Hybrid defaults to auto and three workers. Workers share one
  checkout and cannot edit; their model calls stay local. Creating one sandbox
  per worker is deliberately omitted: it adds cost and reconciliation work.
- `--mode` and `--permissions` stay independent. Hybrid defaults to `auto`
  permissions; it does not inherit the full-cloud launcher's `--yolo` flag.

The bridge binds only to 127.0.0.1 with a random per-session token, bounded
requests/responses, bounded concurrent RPCs, no HTTP logs, no redirects, and
no retries of uncertain writes. It reuses one SDK client and the existing
Rust HTTP client crate. Each RPC starts a small Rust tool process in the
sandbox, reusing the same filesystem/shell implementations. Permission checks
inspect **remote** scripts and branch state, and are checked again before
execution. Infrastructure target detection, dry-run gates, pre-change capture
and rollout checks use the remote environment; snapshots are retained privately
on the controller and staged privately outside the sandbox checkout for
rollback, so ordinary Git commits cannot accidentally include them.
Repo instructions come from the remote checkout; installed slash-command skills
stay on the controller. Sessions/memory are scoped by sandbox ID and workspace.
Model keys and host environment are never exported by hybrid. Use
`--project-id <stable-name>` to retain on/deep advisor lessons across replacement
sandboxes, and `--continue` to resume the latest local session for the attached
sandbox/workspace. These settings contain no credentials.

Hybrid owns only its local bridge. It does not stop, delete, reset or change
the lifecycle policy of an attached sandbox. Changes stay in that sandbox;
commit/export them with your normal Git or Daytona workflow. Ctrl-C stops
waiting locally, but a remote command may continue until its timeout; inspect
its outcome before retrying. There is no automatic replay or cancellation
claim. The sandbox can continue consuming compute credits after the CLI exits.

### Hybrid checks without cloud spending

```bash
cargo xtask qa
# Or only the hybrid journey after building both binaries:
python3 -m unittest cloud/test_hybrid.py
# Optional: repeat the same journeys through the real pinned SDK's HTTP
# serialization/transport against a localhost Toolbox API. No account needed.
uv run cloud/sdk_smoke.py
```

These tests run the real Rust CLI, real Rust tools and HTTP bridge in separate
local controller/sandbox directories with stand-ins for the SDK transport and
model. Three workers overlap in each profile; forged edits are denied; all
eight file/search/Bash tools operate remotely; the lead edits and verifies;
careful has one checker; memory on runs on the controller without hook timeouts.
Additional checks cover remote script inspection, changed approvals, unavailable
transport, missing/invalid placement settings, authentication, request bounds,
attach-only lifecycle behavior, and remote infra gates/snapshots/verification.
The optional SDK smoke test also passed all ten cases through SDK 0.220.0
against the localhost Toolbox API fixture. These establish local integration
behavior, **not** fresh hosted or live-model qualification. No new sandbox or paid model run was used for this feature.

### Fully cloud agent

[`cloud/daytona.py`](../cloud/daytona.py) runs rusty in a Daytona sandbox
and brings the results home.

```bash
export DAYTONA_API_KEY=...                       # or put it in .env
uv run cloud/daytona.py prepare --source --cpu 2 --memory 4
# Copy the printed snapshot name into the run command.
# --source builds both Rust binaries remotely from tracked HEAD; no local Docker.
uv run cloud/daytona.py run --snapshot rusty-<printed-hash> \
    --repo https://github.com/you/project --goal "get the test suite green" --mode careful
```

- **A pinned, prepared environment.** `prepare` builds a snapshot from a
  digest-pinned `debian:bookworm-slim` with git, ripgrep, python3 and make.
  `--source` builds rusty and rusty-memoryd with pinned Rust 1.90 Alpine,
  strips them, and includes MIT and dependency notices. Only archived HEAD
  runtime inputs are uploaded; `.env`, `.git` and untracked files stay local.
  The snapshot fingerprint includes source, recipe, resources and region
  (`rusty-<hash>`), so a run always starts from the same environment, and a
  new build gets a new snapshot. `prune` lists old `rusty-*` snapshots and
  deletes them only with `--yes`.
- **Tracked runs.** rusty runs in a background session in the sandbox. The
  run's id, sandbox, starting commit and command are saved to
  `cloud-runs/<run-id>/run.json` the moment the sandbox exists. Ctrl-C only
  stops following; `status`, `logs` and `stop` pick the run up again, and
  `list` shows every run started from here.
- **Results before cleanup.** When rusty finishes (or on `stop`), the launcher
  downloads these into `cloud-runs/<run-id>/`:
  - `patch.diff`, a binary-safe diff against the starting commit, ready for `git apply`;
  - `new-files.tgz`;
  - rusty's output and `--stats`;
  - the trajectory;
  - rusty's session state, without key files;
  - a `manifest.json` with sizes and hashes.

  The sandbox is deleted only after a confirmed exit and all essential exports
  arrive (patch, output, errors, trajectory, exit receipt, plus scoped memory
  when enabled). If the export fails, the sandbox is kept, and the launcher only ever deletes the sandbox labelled with its own run id.

Run state and exports use private directories (0700) and files (0600).
`--keep` keeps the sandbox after exporting. Keys reach the sandbox as
environment variables, never on a command line.

### NVIDIA endpoint access

The launcher checks endpoint transport before starting the agent, without
sending a model key. This does not validate the key or model availability.
Daytona Tier 1/2 blocks general outbound internet and rejects sandbox allowlist
overrides. NVIDIA's `integrate.api.nvidia.com` is blocked on the tested Tier 2
account. The documented solution is Tier 3, requiring a one-time $500 wallet
top-up. Support can be asked whether a scoped exception is possible; no such
exception has been granted. The launcher never upgrades or charges your account.
See [network limits](https://www.daytona.io/docs/en/network-limits/),
[tiers](https://www.daytona.io/docs/en/limits/) and
[troubleshooting](https://www.daytona.io/docs/en/troubleshooting/).
Do not disable TLS validation to work around connection resets.

Set `DAYTONA_TARGET=eu` before preparing and running to use EU. Snapshots are
region specific; reconnect uses the target saved in the run state. Resources
belong to `prepare`; each run inherits its snapshot's resources. For an
existing local Linux binary, `prepare --binary out/rusty` also packages the
sibling `rusty-memoryd`. The adapter pins Daytona SDK 0.220.0.

### Swarm and memory

One lead uses `--agents swarm` to schedule bounded parallel read-only workers;
only the lead edits and runs mutating commands. This runs within one sandbox,
so workers share a checkout and local memory socket without extra network hops.
Use `--memory-mode on` for the local advisor or `deep` for asynchronous model
advice, and `--memory-input <scoped-export.json.gz>` for explicit L2 transfer.
L1 stays in RAM; scoped L2 is exported separately, never as a raw database/WAL.
`--model` selects a model on the configured `RUSTY_BASE_URL` endpoint.

```bash
uv run cloud/daytona.py run --snapshot rusty-<printed-hash> \
    --repo https://github.com/you/project --ref <commit> \
    --agents swarm --memory-mode on --mode standard \
    --prompt "Use three read-only workers to inspect the components, then implement and verify the fix."

# Hosted deterministic integration: real sandbox and real tools, scripted model.
uv run cloud/swarm_smoke.py --snapshot rusty-<printed-hash> \
    --ref <commit> --scripted-provider --report hosted.json
# Live qualification: same journey, real model endpoint and credentials.
uv run cloud/swarm_smoke.py --snapshot rusty-<printed-hash> \
    --ref <commit> --report live.json
```

The scripted journey checks three workers overlap, forged worker writes are
denied, real reads succeed, the lead writes and verifies files, memory hooks
meet their deadline, and exports are hash-verified before deletion. Its provider
runs outside the checkout in a separate Daytona session to avoid queuing the
CLI behind a long-lived server. It is not evidence of live model quality.
Live qualification remains blocked until endpoint access and credentials work.

## Docker (any cloud, CI, Kubernetes)

```bash
docker build -t rusty .
docker run --rm -it -v "$PWD:/work" -e NVIDIA_API_KEY rusty
docker run --rm -v "$PWD:/work" -e NVIDIA_API_KEY rusty --yolo --goal "fix the failing tests"
docker build --target bin -o out .  # just the static binary
```

## AWS

- **EC2 or Cloud9-style dev boxes:** copy the static binary from
  `docker build --target bin -o out .` (pick an instance that matches your
  build architecture) and keep the key in SSM Parameter Store:
  `export NVIDIA_API_KEY=$(aws ssm get-parameter --with-decryption --name /rusty/nvidia --query Parameter.Value --output text)`.
- **ECS / Fargate / CodeBuild for unattended goals:** push the image to ECR
  and run `rusty --yolo --goal "…"` as the task command, with the key injected
  from Secrets Manager as an environment variable. `--stats` writes a JSON
  summary to stderr for your logs, and `--trajectory /out/run.json` saves the
  full conversation.

## Devbox

[`devbox.json`](../devbox.json) gives a reproducible shell with Rust,
ripgrep and Python: `devbox shell`, then `devbox run qa` or
`devbox run rusty`.

## Benchmarks

rusty runs as a Harbor agent: `--yolo --goal` for autonomy, `--trajectory`
for a reviewer-readable transcript, and `--stats` for token accounting.
