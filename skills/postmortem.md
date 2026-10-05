---
description: Write a blameless postmortem from the audit log, git history and what you can observe
---
Postmortem for: {{args}}

Gather facts before writing a word. Take the timeline from rusty's audit log (the audit.jsonl file under the project's rusty directory, or /audit), `git log` around the incident, `kubectl get events`, `helm history`, `kubectl rollout history`, and what the user tells you. Quote commands and timestamps; never guess a time, and mark anything you could not verify with "(unverified)".

Then write, in this order and without blaming a person:

1. Summary. Two sentences: what broke, for whom, for how long.
2. Impact. Users, requests, data, money. Numbers where you have them, ranges where you don't.
3. Timeline. UTC, one line each, including: the change that started it, the first symptom, detection, first response, mitigation, full recovery.
4. Root cause. The chain from change to symptom, and what was true of the system that let it happen. "The deploy had no readiness probe" is a cause; "someone forgot" is not.
5. Detection. How it was noticed and how long after it began. Was there an alert? Should there have been?
6. What went well, what went badly, where we got lucky.
7. Action items. Each with an owner placeholder, a due date placeholder, and whether it prevents, detects sooner, or shortens a repeat. Three that would have stopped this one beat ten generic ones.

Write it to postmortem-<YYYY-MM-DD>.md in the project unless told otherwise, and end by listing the facts still marked unverified.
