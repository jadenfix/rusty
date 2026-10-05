# Running rusty in the cloud

rusty is a single static binary with no runtime dependencies, so it runs
anywhere Linux does. Model keys always travel as environment variables, never
as arguments.

## Daytona

[`cloud/daytona.py`](../cloud/daytona.py) uses the Daytona Python SDK to
create a sandbox, install rusty, clone your repo, and either run a goal
headless or hand you an SSH session.

```bash
export DAYTONA_API_KEY=...          # or put it in .env
uv run cloud/daytona.py --repo https://github.com/you/project --goal "get cargo test green"
uv run cloud/daytona.py --repo https://github.com/you/project --ssh
```

Useful flags: `--agents swarm`, `--cpu 4 --memory 8`, `--keep`. Sandboxes
auto-stop after 30 idle minutes and are deleted when a headless run finishes,
unless you pass `--keep`.

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
