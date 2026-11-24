//! A spec toolset (Kiro, OpenSpec, superpowers, or one a team adds) as data:
//! where its files go, which of them hold steps, what installing it runs and
//! the paragraph agents are told about it. Nothing here touches the disk or
//! runs anything; later code reads a `Toolset` and does the work.

// Later tasks call all of this; until then nothing outside the tests does.
#![allow(dead_code)]

use super::glob;
use serde::{Deserialize, Serialize};

/// One toolset's manifest. The built-in ones are JSON files in `toolsets/`;
/// a custom one has the same shape in the app config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toolset {
    pub id: String,
    pub name: String,
    /// Pinned version of the tool it installs, shown in the UI.
    #[serde(default)]
    pub version: Option<String>,
    /// Globs relative to the spec folder: what the Spec tab lists.
    pub files: Vec<String>,
    /// Globs of the files holding `- [ ]` steps; each must fall inside `files`.
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub install: Vec<InstallStep>,
    /// Folder inside a repository the spec is committed to, `{task}` allowed.
    #[serde(default)]
    pub commit_to: Option<String>,
    /// The paragraph for the task's `AGENTS.md`. Built-ins load it from
    /// `toolsets/<id>.md`; a custom manifest writes it inline.
    #[serde(default)]
    pub agents_md: String,
    #[serde(default)]
    pub extras: Vec<Extra>,
}

/// One thing installing a toolset does. The kinds are a closed set so a
/// manifest cannot ask for anything else. It is read through `RawStep`: an
/// untagged enum would take a step naming two kinds as the first and drop
/// the rest without a word, and serde cannot refuse unknown keys there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawStep", untagged)]
pub enum InstallStep {
    /// An argv, never a shell string.
    Run { run: Vec<String>, cwd: RunIn, timeout_secs: Option<u64> },
    /// A git URL at a pinned ref, cloned once and reused.
    Fetch { fetch: String, r#ref: String },
    /// Copy skill folders (a glob in the fetched copy) into the skill
    /// directories of the agent CLIs set up on this Mac.
    Skills { skills: String },
}

/// Every key a step may have; `try_from` checks that they make one kind.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    run: Option<Vec<String>>,
    #[serde(default)]
    cwd: Option<RunIn>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    fetch: Option<String>,
    #[serde(rename = "ref")]
    r#ref: Option<String>,
    skills: Option<String>,
}

impl TryFrom<RawStep> for InstallStep {
    type Error = String;

    fn try_from(raw: RawStep) -> Result<Self, String> {
        match raw {
            RawStep { run: Some(run), cwd, timeout_secs, fetch: None, r#ref: None, skills: None } => {
                Ok(InstallStep::Run { run, cwd: cwd.unwrap_or_default(), timeout_secs })
            }
            RawStep {
                fetch: Some(fetch),
                r#ref: Some(r#ref),
                run: None,
                cwd: None,
                timeout_secs: None,
                skills: None,
            } => Ok(InstallStep::Fetch { fetch, r#ref }),
            RawStep { skills: Some(skills), run: None, cwd: None, timeout_secs: None, fetch: None, r#ref: None } => {
                Ok(InstallStep::Skills { skills })
            }
            _ => Err("an install step is one of run (with cwd, timeout_secs), fetch (with ref) or skills".into()),
        }
    }
}

/// Where a `run` step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunIn {
    #[default]
    Spec,
    Task,
}

/// Features only some toolsets have (today Kiro's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Extra {
    Draft,
    Check,
    Questions,
}

/// What `{version}`, `{task}`, `{openspec_tools}` and `{skill_dirs}` stand
/// for in a manifest's arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    pub version: String,
    pub task: String,
    pub openspec_tools: String,
    pub skill_dirs: String,
}

/// The toolsets Layer ships, in the order the UI lists them. A manifest that
/// fails to parse is left out rather than taking the app down; the tests
/// below fail on that.
pub fn builtin() -> Vec<Toolset> {
    [
        (include_str!("../../toolsets/kiro.json"), include_str!("../../toolsets/kiro.md")),
        (include_str!("../../toolsets/openspec.json"), include_str!("../../toolsets/openspec.md")),
        (include_str!("../../toolsets/superpowers.json"), include_str!("../../toolsets/superpowers.md")),
    ]
    .into_iter()
    .filter_map(|(json, paragraph)| {
        let mut t: Toolset = serde_json::from_str(json).ok()?;
        t.agents_md = paragraph.to_string();
        Some(t)
    })
    .collect()
}

/// The toolset with this id. A custom one wins over a built-in with the
/// same id, so a team can adjust OpenSpec's install without a new id.
pub fn find(id: &str, custom: &[Toolset]) -> Option<Toolset> {
    custom.iter().find(|t| t.id == id).cloned().or_else(|| builtin().into_iter().find(|t| t.id == id))
}

/// Whether `id` can name a toolset: it becomes a folder name in the config
/// folder and is stored on tasks and repositories.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Refuse a manifest that could write outside the spec folder, run nothing,
/// or have steps the Spec tab would never list.
pub fn validate(t: &Toolset) -> Result<(), String> {
    let label = if t.id.is_empty() { "toolset" } else { t.id.as_str() };
    if !valid_id(&t.id) {
        return Err(format!("{label}: the id must be lowercase letters, digits, - or _"));
    }
    if t.name.trim().is_empty() {
        return Err(format!("{label}: it needs a name"));
    }
    // The version names the fetch cache folder (`toolsets/<id>/<version>/`)
    // and goes into refs and argv, so it must be one plain path segment.
    if let Some(v) = &t.version {
        let plain = !v.is_empty()
            && v != "."
            && v != ".."
            && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
        if !plain {
            return Err(format!("{label}: the version {v:?} must be letters, digits, . _ + or -"));
        }
    }
    if t.files.is_empty() {
        return Err(format!("{label}: it needs at least one file glob"));
    }
    for g in t.files.iter().chain(&t.steps) {
        if !glob::is_safe_rel(g) {
            return Err(format!("{label}: {g:?} must be a relative path inside the spec folder"));
        }
    }
    for step in &t.steps {
        // A step glob is covered when some file glob matches a path shaped
        // like it, each `*` standing for one name.
        let sample = step.replace('*', "x");
        if !t.files.iter().any(|f| glob::matches(f, &sample)) {
            return Err(format!("{label}: the step glob {step:?} is not inside its files"));
        }
    }
    for step in &t.install {
        match step {
            InstallStep::Run { run, .. } if run.first().is_none_or(|a| a.is_empty()) => {
                return Err(format!("{label}: a run step needs a command"));
            }
            InstallStep::Fetch { fetch, r#ref } if fetch.trim().is_empty() || r#ref.trim().is_empty() => {
                return Err(format!("{label}: a fetch step needs a URL and a ref"));
            }
            InstallStep::Skills { skills } if !glob::is_safe_rel(skills) => {
                return Err(format!("{label}: the skills glob {skills:?} must stay inside the fetched copy"));
            }
            _ => {}
        }
    }
    if let Some(to) = &t.commit_to {
        if !glob::is_safe_rel(&to.replace("{task}", "x")) {
            return Err(format!("{label}: commit_to {to:?} must be a relative path inside a repository"));
        }
    }
    Ok(())
}

/// Fill the placeholders in one argument. One pass, so a value that itself
/// contains `{task}` stays as written; an unknown `{name}` is left alone.
pub fn expand(arg: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let known = [
            ("{version}", &vars.version),
            ("{task}", &vars.task),
            ("{openspec_tools}", &vars.openspec_tools),
            ("{skill_dirs}", &vars.skill_dirs),
        ]
        .into_iter()
        .find(|(name, _)| after.starts_with(name));
        match known {
            Some((name, value)) => {
                out.push_str(value);
                rest = &after[name.len()..];
            }
            None => {
                out.push('{');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openspec() -> Toolset {
        find("openspec", &[]).expect("openspec is built in")
    }

    #[test]
    fn every_builtin_toolset_is_valid() {
        let all = builtin();
        let ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["kiro", "openspec", "superpowers"]);
        for t in &all {
            assert_eq!(validate(t), Ok(()), "{} should be valid", t.id);
            assert!(!t.agents_md.trim().is_empty(), "{} has a paragraph for AGENTS.md", t.id);
        }
    }

    #[test]
    fn the_builtin_manifests_carry_the_pinned_values() {
        let open = openspec();
        assert_eq!(open.version.as_deref(), Some("1.14.3"));
        assert_eq!(open.commit_to.as_deref(), Some("openspec"));
        assert_eq!(
            open.install,
            vec![InstallStep::Run {
                run: ["npx", "-y", "@fission-ai/openspec@{version}", "init", "--tools", "{openspec_tools}", "."]
                    .map(String::from)
                    .to_vec(),
                cwd: RunIn::Spec,
                timeout_secs: Some(300),
            }]
        );
        let sp = find("superpowers", &[]).expect("superpowers is built in");
        assert_eq!(sp.version.as_deref(), Some("7.0.2"));
        assert_eq!(
            sp.install,
            vec![
                InstallStep::Fetch { fetch: "https://github.com/obra/superpowers".into(), r#ref: "v{version}".into() },
                InstallStep::Skills { skills: "skills/*".into() },
            ]
        );
        let kiro = find("kiro", &[]).expect("kiro is built in");
        assert_eq!(kiro.commit_to.as_deref(), Some("specs/{task}"));
        assert_eq!(kiro.extras, vec![Extra::Draft, Extra::Check, Extra::Questions]);
        assert!(kiro.install.is_empty());
        assert!(open.extras.is_empty(), "only Kiro has extras");
    }

    #[test]
    fn a_step_glob_outside_the_file_globs_is_refused() {
        let mut t = openspec();
        t.steps = vec!["other/*.md".into()];
        let err = validate(&t).expect_err("steps outside files are refused");
        assert!(err.contains("other/*.md"), "{err}");
    }

    #[test]
    fn a_glob_with_dotdot_or_a_leading_slash_is_refused() {
        for bad in ["../x", "/x", "a/../x"] {
            let mut t = openspec();
            t.files = vec![bad.into()];
            t.steps = vec![];
            assert!(validate(&t).is_err(), "files = [{bad:?}] should be refused");
        }
        let mut t = openspec();
        t.steps = vec!["../openspec/tasks.md".into()];
        assert!(validate(&t).is_err());
    }

    #[test]
    fn a_run_step_with_an_empty_argv_is_refused() {
        let mut t = openspec();
        t.install = vec![InstallStep::Run { run: vec![], cwd: RunIn::Spec, timeout_secs: None }];
        assert!(validate(&t).is_err());
        t.install = vec![InstallStep::Run { run: vec!["".into()], cwd: RunIn::Task, timeout_secs: None }];
        assert!(validate(&t).is_err());
    }

    #[test]
    fn a_manifest_without_an_id_or_files_is_refused() {
        let mut t = openspec();
        t.id = "Not An Id".into();
        assert!(validate(&t).is_err());
        let mut t = openspec();
        t.name = " ".into();
        assert!(validate(&t).is_err());
        let mut t = openspec();
        t.files.clear();
        t.steps.clear();
        assert!(validate(&t).is_err());
    }

    #[test]
    fn a_version_that_is_not_one_path_segment_is_refused() {
        for bad in ["", "../../x", "..", ".", "1/2", "1\\2", "1 2", "v1;rm", "1.0\n"] {
            let mut t = openspec();
            t.version = Some(bad.into());
            assert!(validate(&t).is_err(), "version {bad:?} should be refused");
        }
        for good in ["1.14.3", "7.0.2", "2026-11-24", "1.0.0+build_5", "..1"] {
            let mut t = openspec();
            t.version = Some(good.into());
            assert_eq!(validate(&t), Ok(()), "version {good:?} should pass");
        }
        let mut t = openspec();
        t.version = None;
        assert_eq!(validate(&t), Ok(()), "a toolset may have no version");
    }

    #[test]
    fn a_commit_folder_that_leaves_the_repository_is_refused() {
        let mut t = openspec();
        t.commit_to = Some("../elsewhere".into());
        assert!(validate(&t).is_err());
        t.commit_to = Some("/abs".into());
        assert!(validate(&t).is_err());
    }

    #[test]
    fn a_custom_toolset_overrides_the_builtin_with_its_id() {
        let mut mine = openspec();
        mine.name = "My OpenSpec".into();
        let found = find("openspec", std::slice::from_ref(&mine)).expect("found");
        assert_eq!(found.name, "My OpenSpec");
        assert_eq!(find("openspec", &[]).expect("built in").name, "OpenSpec");
        assert!(find("nope", &[mine]).is_none());
    }

    #[test]
    fn placeholders_expand_in_run_arguments() {
        let vars = Vars {
            version: "1.14.3".into(),
            task: "ACME-1-fix".into(),
            openspec_tools: "claude,codex".into(),
            skill_dirs: ".claude/skills".into(),
        };
        assert_eq!(expand("@fission-ai/openspec@{version}", &vars), "@fission-ai/openspec@1.14.3");
        assert_eq!(expand("specs/{task}", &vars), "specs/ACME-1-fix");
        assert_eq!(expand("{openspec_tools}", &vars), "claude,codex");
        assert_eq!(expand("{skill_dirs}", &vars), ".claude/skills");
        assert_eq!(expand("{unknown} stays", &vars), "{unknown} stays");
        assert_eq!(expand("plain", &vars), "plain");
    }

    #[test]
    fn an_expanded_value_is_not_expanded_again() {
        let vars = Vars { version: "{task}".into(), task: "t".into(), ..Vars::default() };
        assert_eq!(expand("{version}", &vars), "{task}");
    }

    #[test]
    fn a_manifest_survives_being_written_to_the_config_and_read_back() {
        for t in builtin() {
            let back: Toolset = serde_json::from_str(&serde_json::to_string(&t).expect("writes")).expect("reads");
            assert_eq!(back, t);
        }
    }

    #[test]
    fn a_manifest_reads_from_json_and_refuses_a_step_with_two_kinds() {
        let json = r#"{"id":"mine","name":"Mine","files":["a/**/*.md"],
            "install":[{"run":["echo","x"]},{"fetch":"https://x/y","ref":"v1"},{"skills":"s/*"}]}"#;
        let t: Toolset = serde_json::from_str(json).expect("parses");
        assert_eq!(validate(&t), Ok(()));
        assert!(matches!(&t.install[0], InstallStep::Run { cwd: RunIn::Spec, timeout_secs: None, .. }));
        let both = r#"{"id":"mine","name":"Mine","files":["a.md"],"install":[{"run":["x"],"fetch":"u","ref":"r"}]}"#;
        assert!(serde_json::from_str::<Toolset>(both).is_err());
    }
}
