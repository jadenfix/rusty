---
description: Map an unfamiliar codebase, or answer a question about it
---
Explore this codebase. {{args}}

Work from the outside in and keep reads cheap:
1. `list_files` at depth 2, plus the README and the build manifest (Cargo.toml, package.json, pyproject.toml, go.mod, Makefile).
2. `outline` the main source directories to see the shape of the code before reading any bodies.
3. `search` for entry points, then read only the ranges that matter.
4. If subagents or a swarm are available and the codebase has several independent areas, hand each area to its own worker.

Then give me a short map: what the project does, how to build and test it, the entry points, the main modules and how data flows between them, and anything surprising. Use `path:line` references. Save the build and test commands with `remember`. If I asked a specific question above, answer it first.
