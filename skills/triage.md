---
description: Triage a live incident: scope it, find the last change, mitigate, then dig
---
Incident: {{args}}

Mitigate first, understand second. Keep a plan with the plan tool and work in this order:

1. Confirm the target. State the kube context, namespace, cloud account and workspace rusty detected and that they are the ones having the incident. If they are not, stop and ask; do not guess a context.
2. Scope it with read-only commands only: what is failing, since when, and how wide. `kubectl get events --sort-by=.lastTimestamp | tail -40`, `kubectl get pods -o wide` for restarts and pending pods, `kubectl rollout history`, logs with `--since=30m --tail=200`, the cloud provider's status, recent alerts. Keep output short.
3. Find the last change. Most incidents follow one. Check /audit for what rusty ran, `git log --since=24h --oneline`, `kubectl rollout history`, `helm history`, `terraform state pull | jq .serial`, and ask the user what was deployed by hand.
4. Decide: roll back or fix forward. If the last change is suspect and a rollback path exists (a rusty snapshot, `kubectl rollout undo`, `helm rollback`, the previous image tag), propose the exact rollback command and wait for a go. Rolling back is cheaper than being right.
5. Only once the service is stable, find the root cause with evidence: diffs, logs, metrics. Change nothing else until the user agrees.

Every few steps write one status line a person could paste into an incident channel: what is broken, impact, what has been done, next step, time (UTC).

End with: current state; every change made, each with its rollback command; open questions; and the facts the postmortem will need (first bad change, first symptom, detection, mitigation, recovery, all with times).
