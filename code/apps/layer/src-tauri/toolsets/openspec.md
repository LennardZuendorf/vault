The spec is an OpenSpec project in `{spec_dir}`. `openspec` is not installed on this Mac: run every command as `npx -y @fission-ai/openspec@{version} <command>` from that folder (`cd {spec_dir}` first), never from a repository's worktree.

- Each change lives in `openspec/changes/<id>/`. Write `proposal.md` first (why and what changes), then `design.md`, then `tasks.md`.
- Put delta specs under `openspec/changes/<id>/specs/<capability>/spec.md`, saying what the change adds, modifies or removes.
- `tasks.md` is a list of `- [ ]` steps. Group them under `# <repo>` headings when the task spans repositories.
- Run `npx -y @fission-ai/openspec@{version} validate <id>` from `{spec_dir}` before you start to implement, and again when you change the spec.
- Do not archive the change here. Archiving (`archive <id>`) merges its deltas into `openspec/specs/` in each repository, after the change has merged; it happens there, outside Layer.

Never write OpenSpec files inside a repository's worktree.
