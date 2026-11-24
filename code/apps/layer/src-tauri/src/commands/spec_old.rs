//! Where earlier builds kept a task's spec (§19), read and never written: the
//! task folder's `.spec-drafts/` with what was approved, each repository's
//! `specs/<task>/` copy, and a `SPEC.md`. Moving that into `spec/` is
//! `spec_copy::move_old_spec`'s; this is what it reads.

use std::path::{Path, PathBuf};

use crate::config::Task;
use crate::spec::{self, Kind, Part};

use super::panes::agent_file_dir;
use super::AppState;

/// Where a spec was kept before it was kept in the repository (SPEC-17).
pub(crate) const LEGACY_FILE: &str = "SPEC.md";

/// The task folder's drafts and what was approved, from before the spec was
/// plain files. Left in place after a move, and taken with the task folder.
pub(crate) const DRAFTS: &str = ".spec-drafts";

/// Under `DRAFTS`: the spec as it was approved.
pub(crate) const APPROVED: &str = "approved";

/// One repository's copy of the spec as earlier builds kept it.
pub(crate) struct OldCopy {
    /// The checkout's folder in the task: `api`.
    pub folder: String,
    /// Where its copy is.
    pub dir: PathBuf,
}

/// A task's spec as earlier builds kept it.
pub(crate) struct Old {
    /// The task folder's `.spec-drafts`: None for a task with no folder of its own.
    pub drafts: Option<PathBuf>,
    /// Every repository's copy, in checkout order.
    pub copies: Vec<OldCopy>,
}

impl Old {
    /// Feature or bug: which requirements file there is, else what was chosen
    /// when the spec was started.
    pub fn kind(&self) -> Kind {
        let has = |dir: &Path, kind: Kind| dir.join(Part::Requirements.file(kind)).is_file();
        let mut dirs: Vec<PathBuf> = self.drafts.iter().map(|d| d.join(APPROVED)).collect();
        dirs.extend(self.copies.iter().map(|c| c.dir.clone()));
        for dir in dirs {
            if has(&dir, Kind::Bugfix) {
                return Kind::Bugfix;
            }
            if has(&dir, Kind::Feature) {
                return Kind::Feature;
            }
        }
        self.drafts
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("state.json")).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| serde_json::from_value(v.get("kind")?.clone()).ok())
            .unwrap_or_default()
    }
}

/// `task`'s spec as earlier builds kept it.
pub(crate) fn old_of(state: &AppState, task: &Task) -> Old {
    let name = spec::folder_name(&task.branch);
    let copies = state
        .config
        .checkouts_of(&task.id)
        .into_iter()
        .filter_map(|checkout| {
            let project = state.config.project(&checkout.project_id).ok()?;
            let folder = Path::new(&checkout.path).file_name()?.to_string_lossy().to_string();
            let dir = if project.spec_commit == crate::config::SpecCommit::None {
                state.config.folder().join("specs").join(spec::folder_name(&project.name)).join(&name)
            } else {
                let base = project.spec_folder.clone().unwrap_or_else(|| "specs".into());
                Path::new(&checkout.path).join(base).join(&name)
            };
            Some(OldCopy { folder, dir })
        })
        .collect();
    Old { drafts: agent_file_dir(state, task).map(|d| d.join(DRAFTS)), copies }
}
