# Running rusty in the cloud

rusty is a single static binary with no runtime dependencies, so it runs
anywhere Linux does. Model keys always travel as environment variables, never
as arguments.

## Daytona

[`cloud/daytona.py`](../cloud/daytona.py) runs rusty in a Daytona sandbox
and brings the results home.

```bash
export DAYTONA_API_KEY=...                       # or put it in .env
docker build --target bin -o out .               # the static Linux binary
uv run cloud/daytona.py prepare                  # once per rusty build
uv run cloud/daytona.py run --repo https://github.com/you/project \
    --goal "get the test suite green" --mode careful
```

- **A pinned, prepared environment.** `prepare` builds a snapshot from a
  digest-pinned `debian:bookworm-slim` with git, ripgrep, python3 and make,
  plus your exact rusty binary. It's named after the hash of all three
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

  The sandbox is deleted only after the patch and the log arrive. If the export fails, the sandbox is kept, and the launcher only ever deletes the sandbox labelled with its own run id.

`--keep` keeps the sandbox after exporting. Keys reach the sandbox as
environment variables, never on a command line.

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
