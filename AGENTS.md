# RespOS agent instructions

## Project knowledge

Before changing kernel, user runtime, build, image, or test behavior, read the relevant files under
`docs/codex/`:

- `project-context.md` for scope and module ownership;
- `architecture.md` for call chains and invariants;
- `current-status.md` for the current revision and verified test state;
- `decisions.md` for established design choices;
- `workflows.md` for supported build and test commands;
- `pitfalls.md` for known failure patterns.

Treat current source, build scripts, Git history, and reproducible test output as stronger evidence than
older prose or model memory. A historical score or result is not a current result unless its commit,
image, command, architecture, and date still match.

## Maintaining project knowledge

When a task produces durable RespOS knowledge, update the appropriate `docs/codex/` file in the same
working branch if documentation changes are within the user's requested scope. Durable knowledge includes:

- a verified architecture boundary or invariant;
- a design decision that affects later implementations;
- a reproducible build/test workflow;
- a current test result or known blocker with commit, date, and command;
- a confirmed pitfall whose cause and diagnosis are known.

Do not update these documents for trivial edits, temporary debug output, or unsupported hypotheses. Mark
uncertain information as `待验证`; never present model memory as project fact without repository or test
evidence. Do not store chat transcripts, credentials, tokens, personal data, or unrelated preferences.

Keep fast-changing results in `current-status.md`, stable structure in `architecture.md`, accepted choices
in `decisions.md`, and failure patterns in `pitfalls.md`. Prefer correcting or linking existing material
over duplicating it.

## Collaboration and workspace safety

Preserve unrelated and uncommitted work. Before editing, inspect `git status --short` and avoid files being
changed by another contributor when possible. Do not commit, push, delete branches/files, or rewrite history
unless the user asks for that operation.

For knowledge imported from another developer or Codex instance, require an evidence-bearing patch rather
than copying its private memory store. Review it like code: verify paths and commands, classify current versus
historical claims, resolve conflicts against the current branch, and keep unresolved differences explicitly
marked `待验证`.

## Cross-device knowledge synchronization

When a Codex instance on another device has RespOS-specific memory that may be useful to the project, use the
repository documents as the synchronization format:

1. Update the local branch and read the current `AGENTS.md` and all relevant `docs/codex/` files first. The
   repository version is the baseline; local model memory is only a source of candidate additions.
2. Summarize only the durable delta from local memory. Do not upload or copy `~/.codex`, session files, raw
   memory databases, chat transcripts, credentials, tokens, personal data, or unrelated preferences.
3. Verify each candidate against the current source tree, Git history, build scripts, test image, or a
   reproducible command. Record the evidence, applicable scope, verification date, and status. If it cannot be
   verified, either omit it or add it explicitly as `待验证`; never overwrite a confirmed current fact with it.
4. Update the existing topic file under `docs/codex/` instead of creating a per-device memory dump. Resolve
   overlap by correcting, dating, or linking existing entries, and preserve useful historical context only when
   it explains current code or prevents a repeated failure.
5. Run path/command checks, cross-document consistency review, `git diff --check`, and `git status --short`.
   Present the documentation patch for review. Commit and push the patch only when the user explicitly requests
   those repository operations.

If two devices report conflicting facts, do not choose by recency of memory alone. Prefer evidence from the
current target commit and reproducible tests; retain unresolved alternatives as `待验证` with the evidence each
side still needs.
