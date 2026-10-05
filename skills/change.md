---
description: Write a change plan with dry run, snapshot, verification and rollback, then wait
---
Plan this change; do not make it yet: {{args}}

1. Target. Name the kube context, namespace, cloud account, terraform workspace and git branch this touches (rusty's detected target is in your instructions) and whether any of it is production. If anything is unclear, ask before going on.
2. Current state. Read it with read-only commands and the files that drive it. Write down what you are changing from: image tags, replica counts, values, resource sizes.
3. Preconditions. What must be true before the first command: clean git status on the right branch, credentials for the right account, a maintenance window, a fresh backup, nobody else mid-deploy.
4. Dry run. Run and show the diff or plan: `kubectl diff -f` or `--dry-run=server`, `helm diff upgrade` or `--dry-run`, `terraform plan -out=rusty.tfplan`. Summarise in one paragraph what it will create, change and destroy, and anything that restarts pods or interrupts traffic.
5. Steps. The exact commands in order, one change per step. rusty snapshots what each one touches first and, in careful mode, waits for the rollout after; say what healthy means for this change (ready replicas, a URL returning 200, a query result, a metric).
6. Rollback. The exact command to undo each step and how long it takes. If a step cannot be undone (data migration, deleted resource), say so in bold.
7. Blast radius. Who notices if it goes wrong, and how you would know within five minutes.

Record the steps with the plan tool, then stop and wait for a go. Recommend careful mode if the session is not in it.
