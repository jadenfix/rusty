---
description: Review the current changes like a careful senior engineer
---
Review the code changes in this repository. {{args}}

1. Work out what to review. If I named a branch, commit or PR above, diff against it. Otherwise review uncommitted work: `git status`, then `git diff` and `git diff --staged`. If there is nothing uncommitted, review the last commit (`git show HEAD`).
2. Read enough surrounding code to understand each change. Use outline and search rather than reading whole files.
3. Look for real problems first: wrong logic, unhandled errors, broken edge cases, races, security holes, missing or wrong tests, behaviour changes the description doesn't mention.
4. Only after that, note anything that would make the code meaningfully simpler.

Report findings ranked by severity. For each one give `path:line`, what goes wrong, and a concrete scenario that triggers it. Skip style nits. If you find nothing serious, say so plainly. Do not edit files.
