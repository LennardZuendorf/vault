The spec uses the superpowers skills. Its files live under `{spec_dir}/docs/superpowers/`, so a skill never writes into a repository's worktree.

- Save the design (the brainstorming skill's output) as `{spec_dir}/docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`.
- Save the implementation plan (the writing-plans skill's output) as `{spec_dir}/docs/superpowers/plans/YYYY-MM-DD-<feature>.md`.
- The plan is a list of `- [ ]` steps. Group them under `### Task N` or `## <repo>` headings when the task spans repositories.

Work in that order: design first, then plan, then code. Use the absolute paths above, not paths relative to a repository.
