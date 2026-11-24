The spec is Kiro-style: requirements, then design, then tasks, in `{spec_dir}`.

- `requirements.md` (or `bugfix.md` for a bug): Goal, Requirements, Out of scope, Open questions. Each requirement is a list item with its id first (`R-1`), then the repositories that meet it (`R-3 [api, web]`), written as WHEN a condition THE SYSTEM SHALL a behaviour. A bug's `bugfix.md` has Current, Expected and Unchanged behaviour (WHEN ... THE SYSTEM SHALL CONTINUE TO ...). Open questions are `Q-1`, ..., with the answers they might take listed under them.
- `design.md`: Approach; Contracts, each with its id first (`C-1`), saying what passes between repositories and which one provides it and which use it; then one `## <repo>` section per repository.
- `tasks.md`: numbered steps, each `- [ ] 3. Call the refund endpoint (R-3, C-1; after api 2)`, naming the requirements and contracts it serves. Group them under `## <repo>` headings.

Work in that order. Settle the requirements before the design, and the design before the tasks. Write each file in `{spec_dir}`; never inside a repository's worktree.
