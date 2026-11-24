The spec is an OpenSpec project in `{spec_dir}`. Run every `openspec` command from that folder: `cd {spec_dir}` first, never from a repository's worktree.

- Each change lives in `openspec/changes/<id>/`. Write `proposal.md` first (why and what changes), then `design.md`, then `tasks.md`.
- Put delta specs under `openspec/changes/<id>/specs/<capability>/spec.md`, saying what the change adds, modifies or removes.
- `tasks.md` is a list of `- [ ]` steps. Group them under `## <repo>` headings when the task spans repositories.
- Run `openspec validate <id>` from `{spec_dir}` before you start to implement, and again when you change the spec.
- When the work is done and every step is ticked, archive the change with `openspec archive <id>`.

Never write OpenSpec files inside a repository's worktree.
