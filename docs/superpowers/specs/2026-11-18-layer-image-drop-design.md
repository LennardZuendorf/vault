# Layer image drop design

## Behaviour

Drop image files from Finder onto a Layer conversation's message box. Layer
copies them into `attachments/` in that task's workspace folder, next to the
repository checkouts. A standalone chat uses its own chat folder. Layer adds
the saved absolute paths to the end of that conversation's editable draft,
one quoted path per line, without replacing anything already typed or pasted.
Nothing goes to the agent until the user sends the draft.

The box shows a quiet highlight during a file drag. While Layer saves the
images, the user can keep typing, but cannot send an incomplete draft. An image
that cannot be saved gets an inline error; any images saved successfully are
still added. Layer never opens a dropped file in the window.

## Approaches

- **Save and insert into the draft (chosen):** The agent gets a durable file
  reference on Send, and the user can add context or remove the reference first.
- **Save and send immediately:** Adding a repository notifies running agents.
  That fits a change to the whole task, but a drop is meant for one conversation
  and may not make sense without the user's explanation.
- **Reuse paste's inline image blocks:** This would show previews but would not
  give the task or chat a local copy. Paste stays unchanged.

## Scope and limits

- Accept PNG, JPEG, GIF and WebP files, up to 10 MiB each and 10 files per drop.
  Use the file signature, not the source name or MIME type, to decide its format.
- Preserve the original on disk. Keep the saved copy if its reference is
  removed from the draft. Do not overwrite an existing file.
- Never write into a checkout, follow an `attachments/` symlink, or use an
  arbitrary destination supplied by the frontend. If a legacy task's root is
  itself a checkout, or `attachments/` is already a checkout, show an inline
  error rather than migrate the task or write to Git.
- Conversation panes only. No changes to terminal input, pasted images, agent
  notifications, sync, remote URL drops, or file cleanup. No new dependencies.
