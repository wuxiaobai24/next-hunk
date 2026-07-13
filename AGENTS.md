# AGENTS.md

Rules for any coding agent operating in this repository. These are mandatory,
not suggestions.

## Workflow: pull requests only

**All code changes must be merged via a Pull Request. Never commit or push
directly to a protected branch (`main` and any `release/*` branch).**

This applies to every agent (and human) — no exceptions for "small" or
"trivial" changes.

Hard requirements:

1. **Branch off `main`** for any change.
   - Branch naming: `feat/<short>`, `fix/<short>`, `chore/<short>`,
     `docs/<short>`, or `agent/<short>`.
   - Do not work on `main`. Do not reuse a shared long-lived branch.

2. **Open a Pull Request** from your feature branch into `main` before any
   change can land. The PR must:
   - Have a concise title matching the repo's style (imperative mood).
   - Describe *what* and *why*, not just the diff.
   - List verification done (tests run, commands executed, output).
   - Reference the issue it closes, if any (`Closes #N`).

3. **Do not self-merge.** The PR must be reviewed (by a human or a designated
   reviewer agent) and pass CI before merge.
   - Squash-merge is the default; the commit subject becomes the squashed
     commit message.
   - Do not force-push to a PR after review unless asked.

4. **Never** run `git push` to `main`, `git commit` directly on `main`,
   fast-forward `main` onto a local branch, or delete a protected branch.

5. Keep the PR scoped: one logical change per PR. Split unrelated work into
   separate PRs on separate branches.

If the user asks to "commit", "ship", "land", or "merge" a change, interpret it
as: open (or update) a PR — not a direct push. Only push directly when the user
*explicitly* overrides this rule in writing for a specific change.

## Code & change hygiene

- Follow the existing style; do not reformat unrelated code.
- Keep the working tree clean: stage only intended files. Never commit
  secrets, `target/`, or generated fixtures.
- Run `cargo test` before requesting review. Note any failing or skipped
  tests in the PR body.
- `CHANGELOG.md` is the source of truth for user-visible changes — update it
  under the unreleased section when the change is user-facing.

## Communication

- Respond in Simplified Chinese when talking to the user, unless asked
  otherwise. Code, commit messages, and PR descriptions stay in English.
