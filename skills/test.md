---
description: Run the test suite and fix what fails
---
Run the tests and get them passing. {{args}}

1. Find the test command: check memory first, then the README, the Makefile, CI config and the manifest.
2. Run it. If it's slow, run the failing subset while you iterate.
3. Fix failures one at a time. Decide whether the test or the code is wrong before changing either, and never weaken a test just to make it pass.
4. Re-run the full suite at the end.

Report what failed, why, what you changed, and the final result. Save the test command with `remember` if it wasn't already known.
