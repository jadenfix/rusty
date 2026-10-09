# Reporting configuration: fsb-reporting-v1

The frozen Rusty configuration that FullStack-Bench reporting runs pin.
This receipt is committed after the code it describes, so it names that
commit rather than being part of it.

| Field | Value |
|---|---|
| Commit | `32cac02ccf9f3c94e2f20b413521c711a94213d8` (main after #62) |
| QA | `cargo xtask qa` exited 0 on that commit, 2026-10-09 19:55 UTC |
| Toolchain | rustc 1.97.0 (2d8144b78 2026-07-07); cargo 1.97.0 (c980f4866 2026-06-30); x86_64 Linux |
| Contract | 1 (docs/VERIFICATION.md, "Harness contract") |
| Reference `rusty` (glibc, `cargo build --release --locked`) | `18ab992bc75995f422ef29256aa9edc4ab53ef0fb8997bf0f53de3a31a1a45f6` |
| Reference `rusty-memoryd` | `8041feb8ad4fddbdfe85b001c9c5e42fd32280a142f43506d5e8a52838883d32` |

A harness that builds its own binary, such as a static musl build, records
that binary's hash alongside this commit. The reference hashes are for this
toolchain and target only.

## `rusty --capabilities`

```json
{
  "agents": [
    "off",
    "sub",
    "swarm",
    "auto"
  ],
  "budget": [
    "max-requests",
    "max-budget-tokens",
    "budget-secs",
    "ledger"
  ],
  "contract": 1,
  "mcp": {
    "check": true,
    "features": [
      "tools"
    ],
    "transports": [
      "stdio"
    ]
  },
  "memory": [
    "off",
    "recall",
    "learn",
    "reflect",
    "deep"
  ],
  "mode": [
    "auto",
    "careful",
    "standard",
    "vibe"
  ],
  "permissions": [
    "read-only",
    "ask",
    "auto",
    "yolo"
  ],
  "stats": [
    "model_budget",
    "claims",
    "safety",
    "goal"
  ],
  "tools": [
    "local",
    "daytona"
  ],
  "toolset": [
    "full",
    "shell"
  ],
  "verify": {
    "supported": true,
    "timeout_secs": [
      1,
      600
    ]
  },
  "version": "0.1.0"
}
```

## What the reporting cohort pins

- The commit above.
- Flags: `--memory off --agents off --mode <standard|careful>
  --toolset <full|shell> --yolo --stats --trajectory <path>`, plus
  `--goal`/`--verify` where the condition calls for them.
- Environment:
  - `RUSTY_NO_DOTENV=1` and a fresh `RUSTY_HOME`;
  - `RUSTY_ALLOW_DESTRUCTIVE` recorded per track (it is an experimental
    condition);
  - `RUSTY_GOAL_MAX_TURNS` recorded;
  - no `RUSTY_INFRA`, `RUSTY_MCP_CONFIG`, `RUSTY_TOOL_BRIDGE_*` or per-mode
    model overrides unless the condition names them.

## Development evidence

The FullStack-Bench trials listed under "Benchmark-motivated changes" in
docs/VERIFICATION.md, and their task families, informed earlier versions.
Results on them are development evidence. Headline claims need fresh,
frozen tasks.
