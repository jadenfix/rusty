# Running rusty in the cloud

rusty is a single static binary with no runtime dependencies, so it runs
anywhere Linux does. Model keys always travel as environment variables, never
as arguments.

## Daytona

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
