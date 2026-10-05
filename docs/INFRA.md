# rusty for infrastructure

A strong model already knows kubectl, helm and terraform. What it cannot do
on its own is know which cluster a command will hit, keep a copy of what it
is about to change, stop a secret from landing in the transcript, prove a
rollout finished, or leave a trail someone can review afterwards. rusty does
those parts. Nothing here needs setup; it switches on when the tools are on
your PATH.

## The live target

At startup rusty reads local config only (no network, no slow CLIs), so it
starts as fast as before:

| what | how |
|---|---|
| kube context and namespace | `kubectl config view --minify` |
| AWS profile and region | `AWS_PROFILE`, `AWS_REGION` or `~/.aws/config` |
| gcloud project | `CLOUDSDK_CORE_PROJECT` or gcloud's config files |
| terraform workspace and backend | `.terraform/environment` and `.terraform/terraform.tfstate`, no CLI |
| git branch | `git rev-parse --abbrev-ref HEAD` |

The result is a `target` row in the banner, `/target` (probes again, and
looks up the AWS account with one `sts get-caller-identity` call), a row in
`/settings`, and one short block in the system prompt so the model names the
target before it changes anything. `RUSTY_INFRA=off` disables the probes and
everything below.

If any name contains `prod`, `prd` or `live` as a word, the session starts in
careful mode and says so. `--mode` on the command line wins, and the choice
is not saved.

## Secrets never reach the model

Every tool result is redacted before it is shown, sent or saved:

- `KEY=value` and `key: value` pairs whose key ends in password, secret,
  token, api_key, access_key, private_key, credentials and the like
- tokens by shape: AWS access keys, GitHub, GitLab, Slack, NVIDIA, OpenAI,
  Stripe, Google, Vault, npm, JWTs
- `Bearer` and `Basic` headers, `user:pass@host` in URLs
- PEM private key blocks
- the `data` and `stringData` of a Kubernetes `Secret`

The model sees `[redacted]` and a note saying how many values were hidden
and how to change one (rewrite the whole line) without matching it.
Template references like `${DB_PASSWORD}` and short placeholders are left
alone so manifest edits keep working. Limits worth knowing: a bare value
printed on its own (`kubectl get secret -o jsonpath=...`) has no key next
to it and is not caught unless its shape is known; the model's own
commands are not redacted, only their output.

## Before a change: a snapshot

Before any command that changes infrastructure, rusty captures what it is
about to touch into `~/.config/rusty/projects/<project>/snapshots/<time>-<tool>-<verb>/`,
mode 0700, files 0600:

| change | kept |
|---|---|
| `kubectl apply/create/replace -f` or `-k` | `before.yaml` (the live objects), `rollback.yaml` (same, applyable) |
| `kubectl scale/set/patch/rollout/label/annotate/delete/drain ...` | the same, for the named resources |
| `helm upgrade/install/rollback/uninstall` | `values.yaml`, `manifest.yaml` of the current release |
| `terraform apply/destroy/import/state ...` | `terraform.tfstate` from `state pull` |

`rollback.yaml` has resourceVersion, uid, managedFields and status stripped,
since the raw copy would be refused by apply after the change. The tool
result tells the model the path and the exact rollback command. When there
is nothing to keep (new objects, a manifest on stdin) it says so.

## Careful mode: diff first, verify after

In careful mode (`/mode careful` or `--mode careful`):

- `kubectl apply/create/replace -f`, `helm upgrade/install` and
  `terraform apply` are refused unless their dry run ran earlier in the
  session against the same files, namespace and context: `kubectl diff` or
  `--dry-run=server`, `helm diff`/`template`/`--dry-run`,
  `terraform plan -out=FILE` followed by `apply FILE`. Editing a file the
  dry run depended on makes it stale. `terraform apply` with no plan file,
  `terraform destroy` and applying from stdin are refused with the recipe.
  The refusal is a tool result, not a permission prompt: it cannot be
  approved, and the model is told the exact command to run first.
- After a successful change, rusty waits for it: `kubectl rollout status`
  with a time limit for every Deployment, StatefulSet and DaemonSet the
  command touched (listed with `get -o name`), `helm status` plus the
  rollouts in the release manifest, or a post-apply `terraform plan
  -detailed-exitcode` that must be empty. The result is on screen, in the
  tool output and in the change record. A failed check tells the model the
  change is not healthy and must not be reported as done. The limit is 180s
  per rollout; `RUSTY_ROLLOUT_TIMEOUT=300` changes it.

Standard and vibe get the same advice in the prompt and the same snapshots
and audit, but do not refuse or wait.

Permissions stay separate. Whether a command may run at all is still the
permission policy's call (`/permissions`); the harness only adds the dry
run, the snapshot and the check around a command that is allowed.

## The audit log and the change record

Every kubectl, helm, terraform, aws, gcloud, az, docker, ssh, ansible,
flux, argocd, pulumi and similar command gets one JSON line in
`~/.config/rusty/projects/<project>/audit.jsonl` (mode 0600): time, mode,
target, the command, whether it was a change or a dry run, how it ended,
the exit code, the snapshot path and the verification result. Denied and
declined commands are logged too.

- `/audit [n]` shows the last n lines across sessions.
- `/changes` shows this session's changes: time, command, exit, snapshot,
  verified or not.
- The same record is printed when the session ends and saved next to the
  session file as `<session>.changes.txt`.

## Skills

- `/triage <what is wrong>`: confirm the target, scope with read-only
  commands, find the last change in the audit log and git, propose the
  rollback and wait, root-cause only once stable. Writes status lines a
  person can paste into the incident channel.
- `/change <what you want to do>`: a plan with the current state, the dry
  run, what healthy means, a rollback per step and the blast radius, then
  stops for a go.
- `/postmortem <incident>`: a blameless write-up with a timeline from the
  audit log and git, root cause as a chain, and action items that would
  have stopped this one. Unverified facts are marked.

## What it does not do

It does not parse YAML or HCL; the kubectl, helm and terraform command lines
are what it understands, and anything it does not recognise is only logged.
It does not snapshot cloud resources changed through `aws` or `gcloud`. It
does not verify anything in standard or vibe mode. It cannot see a secret
the model types into a command, only what commands print. And a diff run in
another terminal does not count: the session has to have seen it.
