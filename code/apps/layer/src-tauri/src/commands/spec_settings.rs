//! The app's spec toolset settings (Settings → Code, §19): which toolsets
//! there are to pick from, the default one, and the custom ones the user adds.

use serde::Serialize;
use tauri::State;

use crate::error::{Error, Result};
use crate::spec::toolset::{self, Toolset};

use super::AppState;

/// A toolset as a pick list offers it: Start work, a repository's setting and
/// Settings → Code. Not the manifest, whose install steps are not for the UI.
#[derive(Debug, Serialize)]
pub struct ToolsetChoice {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    /// Added by the user, on this Mac.
    pub custom: bool,
}

/// What there is to pick: the three Layer ships, in their order, then the
/// custom ones. A custom toolset with a built-in's id stands in its place,
/// as `toolset::find` has it.
pub(crate) fn choices(custom: &[Toolset]) -> Vec<ToolsetChoice> {
    let pick = |t: &Toolset, custom| ToolsetChoice {
        id: t.id.clone(),
        name: t.name.clone(),
        version: t.version.clone(),
        custom,
    };
    let built_in = toolset::builtin();
    let mut out: Vec<ToolsetChoice> = built_in
        .iter()
        .map(|b| match custom.iter().find(|c| c.id == b.id) {
            Some(c) => pick(c, true),
            None => pick(b, false),
        })
        .collect();
    out.extend(custom.iter().filter(|c| !built_in.iter().any(|b| b.id == c.id)).map(|c| pick(c, true)));
    out
}

/// The toolsets a JSON list holds, each checked as a manifest, none twice. An
/// empty text is none.
pub(crate) fn parse_toolsets(json: &str) -> Result<Vec<Toolset>> {
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let toolsets: Vec<Toolset> = serde_json::from_str(json)
        .map_err(|e| Error::Other(format!("custom toolsets must be a list of manifests: {e}")))?;
    for (i, t) in toolsets.iter().enumerate() {
        toolset::validate(t).map_err(Error::Other)?;
        if toolsets[..i].iter().any(|earlier| earlier.id == t.id) {
            return Err(Error::Other(format!("{}: two toolsets have this id", t.id)));
        }
    }
    Ok(toolsets)
}

/// The toolset a task gets when nothing else says. None: Kiro.
#[tauri::command]
pub fn set_default_toolset(state: State<AppState>, id: Option<String>) -> Result<()> {
    set_default(&state, id)
}

fn set_default(state: &AppState, id: Option<String>) -> Result<()> {
    if let Some(id) = id.as_deref() {
        if state.config.with(|c| toolset::find(id, &c.toolsets)).is_none() {
            return Err(Error::Other(format!("there is no spec toolset {id:?}")));
        }
    }
    state.config.update(|c| c.default_toolset = id)
}

/// Replace the user's own toolsets with the manifests in `json`. Refused whole
/// when one is not a manifest: a half-saved list would leave some out
/// without a word. Their install steps run on this Mac, as written.
#[tauri::command]
pub fn set_custom_toolsets(state: State<AppState>, json: String) -> Result<()> {
    set_custom(&state, &json)
}

fn set_custom(state: &AppState, json: &str) -> Result<()> {
    let toolsets = parse_toolsets(json)?;
    state.config.update(|c| c.toolsets = toolsets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::spec_world::world;

    const MINE: &str = r#"[{"id":"mine","name":"Mine","files":["spec/*.md"],"steps":["spec/plan.md"]}]"#;

    #[test]
    fn the_choices_are_the_built_ins_then_the_users_own_and_a_custom_one_takes_its_place() {
        let mine: Vec<Toolset> = serde_json::from_str(MINE).unwrap();
        let ids: Vec<String> = choices(&mine).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["kiro", "openspec", "superpowers", "mine"]);

        let over: Vec<Toolset> = serde_json::from_str(r#"[{"id":"kiro","name":"Our Kiro","files":["a.md"]}]"#).unwrap();
        let shown = choices(&over);
        assert_eq!(shown.len(), 3);
        assert_eq!((shown[0].name.as_str(), shown[0].custom), ("Our Kiro", true));
        assert!(!shown[1].custom);
    }

    #[test]
    fn custom_toolsets_are_kept_whole_or_refused_with_why() {
        let (root, state, _) = world(&["api"]);

        set_custom(&state, MINE).unwrap();
        assert_eq!(state.config.with(|c| c.toolsets.len()), 1);

        let bad = set_custom(&state, r#"[{"id":"Bad Id","name":"x","files":["a.md"]}]"#).unwrap_err().to_string();
        assert!(bad.contains("the id must be"), "{bad}");
        let twice = set_custom(&state, &format!("[{0},{0}]", &MINE[1..MINE.len() - 1])).unwrap_err().to_string();
        assert!(twice.contains("two toolsets have this id"), "{twice}");
        let junk = set_custom(&state, "{").unwrap_err().to_string();
        assert!(junk.contains("list of manifests"), "{junk}");
        assert_eq!(state.config.with(|c| c.toolsets.len()), 1, "a refused list changes nothing");

        set_custom(&state, "  ").unwrap();
        assert!(state.config.with(|c| c.toolsets.is_empty()), "no text is no toolsets");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn custom_toolsets_shown_in_the_field_save_again_as_they_are() {
        let all = r#"[{"id":"mine","name":"Mine","files":["spec/**/*.md"],"steps":["spec/plan.md"],
            "install":[{"run":["echo","hi"],"cwd":"task","timeout_secs":5},
                       {"fetch":"https://example.com/x","ref":"v1"},{"skills":"skills/*"}],
            "commit_to":"spec","extras":["draft"]}]"#;
        let first = parse_toolsets(all).unwrap();

        let shown = serde_json::to_string_pretty(&first).unwrap();

        assert_eq!(parse_toolsets(&shown).unwrap(), first);
    }

    #[test]
    fn the_default_toolset_must_be_one_that_exists_and_none_goes_back_to_kiro() {
        let (root, state, _) = world(&["api"]);

        set_default(&state, Some("openspec".into())).unwrap();
        assert_eq!(state.config.with(|c| c.default_toolset.clone()).as_deref(), Some("openspec"));
        assert!(set_default(&state, Some("mine".into())).is_err(), "not there yet");
        set_custom(&state, MINE).unwrap();
        set_default(&state, Some("mine".into())).unwrap();

        set_default(&state, None).unwrap();
        assert_eq!(state.config.with(|c| c.default_toolset.clone()), None);
        std::fs::remove_dir_all(&root).ok();
    }
}
