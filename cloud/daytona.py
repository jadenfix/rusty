# /// script
# requires-python = ">=3.10"
# dependencies = ["daytona", "python-dotenv"]
# ///
"""Run rusty in a Daytona cloud sandbox.

    uv run cloud/daytona.py --repo https://github.com/you/project \
        --goal "get the test suite green"          # headless, streams output
    uv run cloud/daytona.py --repo ... --ssh       # interactive: prints an ssh command

Needs DAYTONA_API_KEY plus your model keys (NVIDIA_API_KEY, ...) in the
environment or in .env. Keys go to the sandbox as environment variables,
never on a command line.
"""

import argparse
import os
import shlex
import sys

from daytona import CreateSandboxFromImageParams, Daytona, Resources
from dotenv import load_dotenv

RUSTY_GIT = "https://github.com/jadenfix/rusty"
KEY_PREFIXES = ("NVIDIA_API_KEY", "RUSTY_")


def main() -> int:
    load_dotenv()
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", required=True, help="git URL of the project to work on")
    ap.add_argument("--goal", help="objective for a headless goal run")
    ap.add_argument("--prompt", help="a single headless prompt instead of a goal")
    ap.add_argument("--ssh", action="store_true", help="leave the sandbox up and print an ssh command")
    ap.add_argument("--agents", default="off", choices=["off", "sub", "swarm", "auto"])
    ap.add_argument("--cpu", type=int, default=2)
    ap.add_argument("--memory", type=int, default=4, help="GiB")
    ap.add_argument("--keep", action="store_true", help="don't delete the sandbox afterwards")
    args = ap.parse_args()
    if not (args.goal or args.prompt or args.ssh):
        ap.error("pass --goal, --prompt or --ssh")

    env = {k: v for k, v in os.environ.items() if k.startswith(KEY_PREFIXES)}
    if "NVIDIA_API_KEY" not in env and "RUSTY_API_KEY" not in env:
        ap.error("set NVIDIA_API_KEY (or RUSTY_API_KEY) in the environment or .env")

    daytona = Daytona()
    print("☾ creating sandbox…", file=sys.stderr)
    sandbox = daytona.create(
        CreateSandboxFromImageParams(
            image="rust:1-bookworm",
            env_vars=env,
            resources=Resources(cpu=args.cpu, memory=args.memory),
            auto_stop_interval=30,
            labels={"app": "rusty"},
        ),
        timeout=300,
    )
    try:
        setup = (
            "apt-get update -qq && apt-get install -y -qq ripgrep python3 >/dev/null && "
            f"cargo install --locked --git {RUSTY_GIT} --root /usr/local >/dev/null 2>&1 && "
            f"git clone --depth 50 {shlex.quote(args.repo)} /work && rusty --version"
        )
        print("☾ installing rusty and cloning the project…", file=sys.stderr)
        r = sandbox.process.exec(setup, timeout=1200)
        if r.exit_code != 0:
            print(r.result, file=sys.stderr)
            return 1
        if args.ssh:
            access = sandbox.create_ssh_access(expires_in_minutes=120)
            print(f"\nssh {access.token}@ssh.app.daytona.io\nthen: cd /work && rusty\n")
            args.keep = True
            return 0
        task = ["--goal", args.goal] if args.goal else [args.prompt]
        cmd = shlex.join(["rusty", "--yolo", "--stats", "--agents", args.agents, *task])
        print(f"☾ running: rusty {'--goal' if args.goal else ''} …", file=sys.stderr)
        r = sandbox.process.exec(f"cd /work && NO_COLOR=1 {cmd} </dev/null 2>&1", timeout=7200)
        print(r.result)
        diff = sandbox.process.exec("cd /work && git --no-pager diff --stat", timeout=60)
        print("\n☾ changes in the sandbox:\n" + diff.result)
        return r.exit_code
    finally:
        if args.keep:
            print(f"☾ sandbox kept: {sandbox.id}", file=sys.stderr)
        else:
            sandbox.delete()


if __name__ == "__main__":
    sys.exit(main())
