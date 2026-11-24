//! A repository registered here: the user's clone of it, and what the app
//! knows about it. Its shared half is the workspace's (`shared.rs`).

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

pub use layer_domain::SpecCommit;

/// `spec_commit` as written now (`"own"`), or as `specs_in_app` was.
fn commit_or_flag<'de, D: Deserializer<'de>>(d: D) -> Result<SpecCommit, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Flag(bool),
        Mode(SpecCommit),
    }
    Ok(match Either::deserialize(d)? {
        Either::Flag(true) => SpecCommit::None,
        Either::Flag(false) => SpecCommit::Whole,
        Either::Mode(mode) => mode,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Absolute path to the user's own clone: where the repository was found,
    /// and what the app copies. Task worktrees do not come from it.
    pub path: String,
    pub default_branch: String,
    /// "frontend", "backend", ... One group per repo; None means ungrouped.
    #[serde(default)]
    pub group: Option<String>,
    /// The app's own bare copy of the repository, which task worktrees
    /// belong to (`git/store.rs`). None until it has been made.
    #[serde(default)]
    pub store: Option<String>,
    /// How Update from base brings the base into a branch here: set in the
    /// Repos view, or the way this repo was last updated. None: guessed from
    /// its history (UPD-7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_by: Option<crate::git::UpdateBy>,
    /// What says the work here is done (`bun run check`), run in a
    /// worktree by a loop (LOOP-1). Set only by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// The check command this Mac's user typed or confirmed here. `check`
    /// may come from the workspace, which any member could once write; a
    /// loop runs only a command that is this, word for word (LOOP-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_approved: Option<String>,
    /// Where specs are committed here (SPEC-1), from its root. None: `specs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_folder: Option<String>,
    /// The spec toolset this repository prefers, by id: only the default
    /// Start work offers (a task's own toolset decides its layout). None:
    /// no preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_toolset: Option<String>,
    /// How a task's spec reaches this repository. It was `specs_in_app`, a
    /// switch: a config from then holds that key, which reads as this
    /// (`true` is `none`, the app keeps the spec).
    #[serde(default, alias = "specs_in_app", deserialize_with = "commit_or_flag")]
    pub spec_commit: SpecCommit,
    /// Where it fetches from, as `git::remote_key` spells it
    /// (`github.com/acme/api`): read from `origin` when it was added, or
    /// when its profile became a workspace. None: it has no `origin`, and
    /// stays on this Mac only (PROF-12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The workspace's record of the repository (PROF-13), shared by every
    /// clone of it here. None until it is in a workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
}

impl Project {
    /// The repository new task worktrees are cut from: the app's copy once
    /// there is one, the user's clone until then. For a worktree that
    /// already exists, ask it (`commands::owner_of`) instead.
    pub fn repo(&self) -> PathBuf {
        PathBuf::from(self.store.as_deref().unwrap_or(&self.path))
    }

    /// The check command, when there is one and this Mac's user confirmed
    /// that very text (LOOP-1). A loop runs only this, and only this is
    /// named to agents as the command to run (LOOP-10).
    pub fn confirmed_check(&self) -> Option<&str> {
        self.check.as_deref().filter(|c| !c.trim().is_empty() && self.check_approved.as_deref() == Some(*c))
    }
}
