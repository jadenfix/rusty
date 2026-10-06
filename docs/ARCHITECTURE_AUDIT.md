# Memory architecture reassessment

Keep the coding loop and memory companion separate. Use a local Unix socket,
a bounded RAM notepad and a transactional SQLite L2. This keeps inference and
storage off the tool execution path without adding a network service, async
framework, vector index or downloaded model weights.

## Changes and tradeoffs

| Decision | Reason | Cost or limit |
|---|---|---|
| RAM-only L1; remove duplicated event/checkpoint tables | Avoid copying private tool traces to disk and eliminate hook writes/compression | Short-term observations disappear on service restart; saved L2 survives |
| Keep SQLite for L2 | Transactions, crash recovery, imports and tombstones already work; a custom append log would need to rebuild them | Bundled SQLite is the largest added native dependency and is implemented in C; no new C code is authored |
| Explicit miniz_oxide compression backend | Rust implementation; removes the unused zlib-rs package from the lockfile | Compression is not encryption |
| Pack text and evidence only when smaller | Tiny/incompressible records avoid gzip expansion; no second compressor or codec crate | A one-byte raw marker and legacy gzip reader are maintained |
| Cache lesson terms and short evidence | Tokenize each query once and avoid repeatedly inflating/scanning full evidence | Bounded cached terms consume some RAM; initial load still decompresses lessons |
| Reuse a lazily initialized provider client | No TLS setup on the on/off path; deep requests can reuse connections | Deep inference still costs provider latency and tokens |
| One shared redactor and private persistence helpers | Cover hooks, direct saves/imports, history, sessions and audit metadata consistently | Pattern-based redaction cannot identify every arbitrary/encoded secret |

SQLite uses a 512 KiB page-cache target, in-memory temporary storage, a 16 MiB
main-database page quota and a 64 KiB retained-WAL target. These are component
bounds, not a guarantee that the entire daemon or directory stays at that size.
L1 is limited to 32 sessions with eight observations each; inference has one
worker, a four-job queue and bounded inputs/responses. See [MEMORY.md](MEMORY.md)
for the exact deadlines and retention rules.

Files use owner-only permissions and persistence refuses symlinks. Recognizable
credentials are scrubbed from saved JSON, memory and audit metadata. Secret-
containing infrastructure snapshots are refused so no invalid redacted rollback
is created; the existing harness can still proceed without a snapshot and records
that limitation. SQLite secure deletion and WAL truncation reduce stale bytes;
OS swap, backups and previously exported artifacts are outside this guarantee.
Explicitly saved L2 and ordinary CLI session files remain intentional disk use.

## Measurements

Local macOS arm64 release builds, before this reassessment versus this PR's
changes. The lookup fixture stores 512 lessons and measures one cold lookup plus
100 warm lookups over the real socket. A separate resource fixture fills all 32
L1 sessions with eight observations. Receipts: [memory-audit.json](evidence/memory-audit.json).

| Measurement | Before | After |
|---|---:|---:|
| Full-store warm lookup p95 | 0.560 ms | 0.067 ms |
| Full-store cold lookup | 2.775 ms | 2.728 ms |
| Full-store disk use | 744,552 bytes | 245,760 bytes |
| Loaded daemon RSS, 32 L1 sessions | 8,126,464 bytes | 7,946,240 bytes |
| Companion binary | 5,083,840 bytes | 5,083,984 bytes |

The meaningful savings are repeated retrieval work and disk writes. RSS and
binary size changed little; these measurements do not establish end-to-end task
speedups or global optimality. Fast compression is retained on small bounded L2
records; gzip best compression is reserved for explicit exports.

## Dependencies, licenses and verification

No runtime crate was added by this reassessment. Compression has an explicit
Rust backend; rusqlite disables default features and only adds bundled SQLite.
The existing HTTP/TLS stack is reused instead of adding another client. Rusty's
MIT [LICENSE](../LICENSE) and deduplicated [third-party notices](../THIRD_PARTY_NOTICES.txt)
ship in the Docker binary export and runtime image. The inventory covers every
pinned Cargo dependency, including target-specific/build dependencies; a test
fails when a lockfile component has no matching notice. Notice files are not
embedded in either executable.

Deterministic checks cover scope isolation, restart semantics, feedback,
secret canaries, symlinks, legacy migration, decompression bounds, interruption,
imports, and 120 sessions of sustained hooks without disk growth. Real CLI
smokes cover create/edit/Bash in off/on/deep; NVIDIA runs the same tasks on feature
pushes and main. Main CI runs only on main pushes and PRs targeting main, so
feature pushes no longer duplicate the PR CI run. Daytona launch/transfer remains
offline-qualified; a live hosted run is still a separate gate.
