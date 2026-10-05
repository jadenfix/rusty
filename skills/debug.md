---
description: Track down a bug from a symptom and fix the root cause
---
Debug this: {{args}}

1. Reproduce it first, with a test, a command, or a tiny script. If you can't reproduce it, say so and say what you tried.
2. List the two or three most likely causes, then rule them out one at a time with evidence (logs, a narrowed test, git log or blame on the suspicious code).
3. Fix the root cause, not the symptom. Keep the change small.
4. Add a regression test that fails without the fix.
5. Run the relevant tests and show they pass.

End with the cause in one sentence, the fix, and how you verified it.
