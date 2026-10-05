---
description: Write a Conventional Commit for the staged changes and commit
---
Commit my staged changes. {{args}}

1. Run `git diff --staged`. If nothing is staged, show me `git status` and ask what to stage. Don't stage things yourself unless I said to.
2. Write the message in Conventional Commits form: `type(scope): summary`, where type is one of feat, fix, docs, style, refactor, perf, test, build, ci, chore or revert. The summary is imperative, lower case, at most 72 characters, with no full stop.
3. Add a body if the change needs one. Explain why in plain, human language, wrapped at 72 columns. Mark breaking changes with `!` and a `BREAKING CHANGE:` footer.
4. Write it the way a person would. Don't mention AI, agents or tools, and don't add attribution trailers.
5. Commit with `git commit -F -` using a heredoc, then show `git log -1 --stat`.
