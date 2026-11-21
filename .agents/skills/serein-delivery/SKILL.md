---
name: serein-delivery
description: Implement and deliver Serein repository changes as a local commit on the current branch, or as an uncommitted local pass in !fast mode.
---

# Serein delivery

Follow the delivery mode and authorization in [AGENTS.md](../../../AGENTS.md).
Completion means the requested implementation, relevant verification and agreed delivery are done,
or a concrete external blocker has been reported with a reviewable local result.

## Select the workflow

- `!fast`: implement locally and run the smallest useful debug command. Leave changes uncommitted
  until the owner confirms, then follow the root formatting/lint gate and commit on the current
  branch. Skip the full evidence/package workflow.
- Instructions/templates only: validate changed skills and review the diff. No application builds,
  tests, packages, screenshots or runtime measurements are needed.
- Visible or runtime changes: read [evidence guidance](references/evidence.md) before editing so
  the relevant baseline can be collected. Load only the screenshot/measurement sections that apply.
- Author-visible SDK changes: also use `serein-sdk-wiki`; read the affected canonical authoring docs.

## Review and deliver

Inspect the current branch and dirty paths; preserve unrelated work. Do not create or switch
branches. Implement the complete requested slice, inspect the diff and run relevant verification.
Add tests only per the root Tests section; no low-signal tests. Fix failures caused by the task and
rerun checks invalidated by the fix. Use `cargo xtask check` for application changes outside
`!fast`; avoid rebuilding unchanged code for documentation delivery. Never write evidence to
`docs/progress.md` or `docs/adr/`.

Stage explicit task paths, review the staged diff and commit on the current branch. Do not push or
create/update a PR unless the owner explicitly asks; then use `.github/pull_request_template.md`
with explicit base/head and mark completed implementation ready for review.

Finish with the commit hash, result, verification and material limitations. Do not merge or
publish a release without a separate request.
