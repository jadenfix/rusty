# Notes for anyone changing rusty, human or tool

- Keep it small. Few abstractions, few dependencies, no frameworks. Add a crate
  only when the standard library really can't do the job.
- Commits follow Conventional Commits, with one logical change each and the
  body explaining why (see CONTRIBUTING.md). Enable the hook with
  `cargo xtask install-hooks`.
- Pull requests read like a person wrote them: plain language, no mention of
  AI tools or coding agents, no "generated with" lines, no co-author trailers
  for tools.
- `cargo xtask qa` must pass before you push. For behaviour that depends on the
  model, run `cargo xtask qa --live` or the relevant `cargo xtask eval <task>`.
- Never commit `.env` or print a key. Keys reach child processes and
  containers as environment variables, never as arguments.
- Terminal UI changes need a real capture (`expect` + a pty) checked for
  alignment, clipping and both narrow and wide terminals.
- Eval checks stay hidden from the agent: never copy `check.sh` or `check.py`
  into the task's `files/`.
