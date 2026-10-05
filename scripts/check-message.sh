#!/usr/bin/env bash
# Checks one commit message (a file path, or text on stdin with "-").
#   scripts/check-message.sh .git/COMMIT_EDITMSG
#   echo "feat: add x" | scripts/check-message.sh -
set -euo pipefail
msg=$(if [[ ${1:--} == - ]]; then cat; else cat "$1"; fi | grep -v '^#' || true)
subject=$(printf '%s\n' "$msg" | head -n1)

types='feat|fix|perf|refactor|docs|test|build|ci|style|chore|revert'
if [[ $subject =~ ^Merge\  || $subject =~ ^Revert\ \" ]]; then exit 0; fi
if ! [[ $subject =~ ^($types)(\([a-z0-9._/-]+\))?!?:\ [^A-Z\ ].*[^.]$ ]]; then
  echo "✗ not a Conventional Commit: \"$subject\"" >&2
  echo "  expected: type(scope): lower-case imperative summary, no full stop" >&2
  echo "  types: ${types//|/, }" >&2
  exit 1
fi
if (( ${#subject} > 72 )); then
  echo "✗ subject is ${#subject} characters; keep it to 72" >&2
  exit 1
fi
if printf '%s\n' "$msg" | grep -qiE 'generated (with|by) |co-authored-by: *(claude|copilot|chatgpt|gpt|gemini|cursor|codex)|\bclaude code\b|as an ai\b|🤖'; then
  echo "✗ remove tool attribution; you are the author of this change" >&2
  exit 1
fi
