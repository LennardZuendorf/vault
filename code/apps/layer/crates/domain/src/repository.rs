use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::model::{Validate, ValidationError, invalid, text};

/// A repository the workspace works on. It is the same repository on every
/// Mac by where it fetches from (`remote_key`); where a clone of it is on
/// disk is each Mac's own, and never syncs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Repository {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// Its remote, spelled one way for every way of spelling it:
    /// `github.com/acme/billing` (see [`remote_key`]). Unique in the
    /// workspace, and it can't change.
    pub remote_key: String,
    pub name: String,
    pub default_branch: String,
    /// "frontend", "backend", ... None means ungrouped. (`group` and
    /// `check` are SQL words, so the columns are named longer.)
    pub group_name: Option<String>,
    /// How Update from base brings the base into a branch. None: guessed
    /// from the repository's history.
    pub update_by: Option<UpdateBy>,
    /// What says the work here is done (`bun run check`), run by a loop.
    pub check_command: Option<String>,
    /// Where specs are committed, from the root. None: `specs`.
    pub spec_folder: Option<String>,
    /// The spec toolset this repository prefers, by id: only the default
    /// Start work offers. None: no preference.
    pub spec_toolset: Option<String>,
    /// How a task's spec reaches this repository (`whole`, `own` or `none`).
    /// `none` keeps it off the repository: it stays on the Mac.
    pub spec_commit: SpecCommit,
    /// Who added it. Set by the server.
    pub added_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// How a branch takes in what its base has done since it was cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum UpdateBy {
    Merge,
    Rebase,
}

/// What Commit spec puts in a repository: the whole spec, only the part
/// that is this repository's, or nothing (the app keeps it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SpecCommit {
    #[default]
    Whole,
    Own,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export, optional_fields)]
pub struct RepositoryCreate {
    /// Any spelling of the remote: the server keeps [`remote_key`]'s.
    pub remote_key: String,
    pub name: String,
    pub default_branch: String,
    pub group_name: Option<String>,
    pub update_by: Option<UpdateBy>,
    pub check_command: Option<String>,
    pub spec_folder: Option<String>,
    pub spec_toolset: Option<String>,
    pub spec_commit: Option<SpecCommit>,
    /// Layer before 0.17 sends this instead of `spec_commit`; `true` is
    /// `none` (see [`specs_in_app`]). Never sent back.
    #[serde(default, skip_serializing)]
    #[ts(skip)]
    pub specs_in_app: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export, optional_fields)]
pub struct RepositoryPatch {
    pub name: Option<String>,
    pub default_branch: Option<String>,
    /// `null` takes it out of its group, and clears the three below.
    pub group_name: Option<Option<String>>,
    pub update_by: Option<Option<UpdateBy>>,
    pub check_command: Option<Option<String>>,
    pub spec_folder: Option<Option<String>>,
    pub spec_toolset: Option<Option<String>>,
    pub spec_commit: Option<SpecCommit>,
    /// Layer before 0.17 sends this instead of `spec_commit`; `true` is
    /// `none` (see [`specs_in_app`]). Never sent back.
    #[serde(default, skip_serializing)]
    #[ts(skip)]
    pub specs_in_app: Option<bool>,
}

impl Validate for RepositoryCreate {
    fn validate(&self) -> Result<(), ValidationError> {
        remote(&self.remote_key)?;
        name(&self.name)?;
        branch(&self.default_branch)?;
        settings(
            self.group_name.as_deref(),
            self.check_command.as_deref(),
            self.spec_folder.as_deref(),
            self.spec_toolset.as_deref(),
        )
    }
}

impl Validate for RepositoryPatch {
    fn validate(&self) -> Result<(), ValidationError> {
        self.name.as_deref().map(name).transpose()?;
        self.default_branch.as_deref().map(branch).transpose()?;
        settings(
            self.group_name.as_ref().and_then(Option::as_deref),
            self.check_command.as_ref().and_then(Option::as_deref),
            self.spec_folder.as_ref().and_then(Option::as_deref),
            self.spec_toolset.as_ref().and_then(Option::as_deref),
        )
    }
}

fn settings(
    group: Option<&str>,
    check: Option<&str>,
    spec_folder: Option<&str>,
    spec_toolset: Option<&str>,
) -> Result<(), ValidationError> {
    group.map(|g| text(g, "A group", 60)).transpose()?;
    check.map(|c| text(c, "A check", 500)).transpose()?;
    spec_folder.map(folder).transpose()?;
    spec_toolset.map(toolset_id).transpose()?;
    Ok(())
}

/// A toolset's id as the app spells it (`kiro`, `my-flow`): the manifest's
/// own rule, so a record can't name one no manifest could have.
fn toolset_id(value: &str) -> Result<(), ValidationError> {
    let valid = !value.is_empty()
        && value.len() <= 60
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        invalid(format!(
            "{value:?} can't be a toolset's id: lowercase letters, digits, - or _."
        ))
    }
}

/// Says an older client's `specs_in_app` as `spec_commit`, in the fields
/// it sent, so a team whose Macs are not all on one version still syncs.
/// `true` is `none`. `false` is dropped: an older client never sees `none`
/// (it reads a missing `specs_in_app` as `false`) and sends `false` with
/// every change, so taking it as `whole` would undo `own` or `none` each
/// time someone on an older Layer touched the repository.
pub fn specs_in_app(data: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(old) = data.remove("specs_in_app") else {
        return;
    };
    if old.as_bool() == Some(true) && !data.contains_key("spec_commit") {
        data.insert("spec_commit".into(), serde_json::json!(SpecCommit::None));
    }
}

fn remote(value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() || value.len() > 300 || value.chars().any(char::is_control) {
        return invalid(
            "A repository needs the address it fetches from, like github.com/acme/api.",
        );
    }
    Ok(())
}

/// One folder's name. Each Mac clones a repository into a folder of its
/// name, and checks it out in every task as one: a name like
/// `../.claude/skills/x` or `/etc` put them outside the folder chosen.
fn name(value: &str) -> Result<(), ValidationError> {
    text(value, "A repository's name", 100)?;
    let valid = !value.contains(['/', '\\'])
        && !value.starts_with(['.', '-'])
        && !value.chars().any(char::is_control);
    if valid {
        Ok(())
    } else {
        invalid(format!(
            "{value:?} can't be a repository's name: it is one folder's name, with no / or \\, \
             not starting with . or -."
        ))
    }
}

/// `name` as one folder's name, for a clone or a checkout on disk: what
/// [`RepositoryCreate`] refuses made harmless rather than refused, since a
/// record may be from before that, or from a server that let it through,
/// and a clone's folder name (`.dotfiles`) is any name at all.
pub fn repository_folder(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let name = name
        .trim_start_matches(|c: char| c == '.' || c == '-' || c.is_whitespace())
        .trim_end();
    if name.is_empty() {
        "repository".into()
    } else {
        name.to_string()
    }
}

/// A branch name git takes: no spaces, and never an option.
fn branch(value: &str) -> Result<(), ValidationError> {
    let valid = !value.is_empty()
        && value.len() <= 200
        && !value.starts_with('-')
        && !value.contains(|c: char| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
        && !value.contains("..");
    if valid {
        Ok(())
    } else {
        invalid(format!("{value:?} isn't a branch name git takes."))
    }
}

/// Relative to the repository's root, and inside it.
fn folder(value: &str) -> Result<(), ValidationError> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('-')
        || value.split('/').any(|seg| seg == "..")
    {
        return invalid("A spec folder is a folder inside the repository, like specs.");
    }
    Ok(())
}

/// A remote URL spelled one way for every way of spelling it:
/// `git@github.com:org/api.git`, `ssh://git@github.com/org/api` and
/// `https://github.com/org/api` are all `github.com/org/api`. A remote on
/// disk is its path. The same as the desktop app's `git::remote_key`, which
/// repo notes have always been kept by.
pub fn remote_key(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/').unwrap_or((rest, ""))
    } else if let Some(scp) = url.split_once(':').filter(|(host, _)| !host.contains('/')) {
        scp
    } else {
        // A path on disk, or already a key.
        return url.to_string();
    };
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    format!(
        "{}/{}",
        host.to_ascii_lowercase(),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_of_a_remote_is_one_key() {
        for url in [
            "git@github.com:acme/billing.git",
            "ssh://git@github.com/acme/billing",
            "https://GitHub.com/acme/billing/",
            "github.com/acme/billing",
        ] {
            assert_eq!(remote_key(url), "github.com/acme/billing", "{url}");
        }
    }

    #[test]
    fn a_branch_is_never_an_option() {
        assert!(branch("main").is_ok());
        assert!(branch("release/1.2").is_ok());
        for bad in ["", "-x", "a b", "a..b", "a:b"] {
            assert!(branch(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_repositorys_name_is_one_folder_and_never_a_path() {
        for good in ["api", "billing-service", "my repo", "web.v2", "café"] {
            assert!(name(good).is_ok(), "{good}");
            assert_eq!(repository_folder(good), good);
        }
        for bad in [
            "../.claude/skills/x",
            "/etc",
            "a/b",
            "a\\b",
            ".",
            "..",
            ".claude",
            "-x",
            "a\nb",
            "",
        ] {
            assert!(name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_name_that_got_through_still_makes_one_folder_inside_the_one_chosen() {
        let chosen = std::path::Path::new("/Users/ada/code");
        for (bad, folder) in [
            ("../.claude/skills/x", "claude-skills-x"),
            ("/etc", "etc"),
            ("..", "repository"),
            (".", "repository"),
            ("", "repository"),
            (".claude", "claude"),
            ("--upload-pack=x", "upload-pack=x"),
            ("a\\..\\b", "a-..-b"),
            ("a\nb", "a-b"),
        ] {
            assert_eq!(repository_folder(bad), folder, "{bad:?}");
            let into = chosen.join(repository_folder(bad));
            assert_eq!(into.parent(), Some(chosen), "{bad:?}");
        }
    }

    #[test]
    fn a_spec_folder_stays_inside_the_repository() {
        assert!(folder("docs/specs/").is_ok());
        for bad in ["", "/etc", "../x", "a/../../b", "-x"] {
            assert!(folder(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_spec_commit_is_whole_own_or_none() {
        for (word, mode) in [
            ("whole", SpecCommit::Whole),
            ("own", SpecCommit::Own),
            ("none", SpecCommit::None),
        ] {
            let patch: RepositoryPatch =
                serde_json::from_value(serde_json::json!({ "spec_commit": word })).unwrap();
            assert_eq!(patch.spec_commit, Some(mode), "{word}");
        }
        for bad in ["", "all", "Whole", "true"] {
            let patch = serde_json::from_value::<RepositoryPatch>(
                serde_json::json!({ "spec_commit": bad }),
            );
            assert!(patch.is_err(), "{bad:?}");
        }
        assert_eq!(SpecCommit::default(), SpecCommit::Whole);
    }

    #[test]
    fn a_spec_toolset_is_one_manifest_id() {
        assert!(toolset_id("kiro").is_ok());
        assert!(toolset_id("my_flow-2").is_ok());
        for bad in ["", "Kiro", "a b", "../x", "x/y", &"a".repeat(61)] {
            assert!(toolset_id(bad).is_err(), "{bad:?}");
        }
        let patch = RepositoryPatch {
            spec_toolset: Some(Some("../x".into())),
            ..Default::default()
        };
        assert!(patch.validate().is_err());
        let clear = RepositoryPatch {
            spec_toolset: Some(None),
            ..Default::default()
        };
        assert!(clear.validate().is_ok());
    }
}
