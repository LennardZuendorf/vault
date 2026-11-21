//! Agents over ACP (§20): starting one in a pane, and the window's side of
//! its conversation.

use tauri::{AppHandle, Emitter, Manager, State};

use crate::acp;
use crate::agents;
use crate::config::ModelChoice;
use crate::error::{Error, Result};
use crate::pty::{PaneInfo, PaneKind, SpawnOptions};
use crate::shellenv;

use super::panes::Resume;
use super::AppState;

/// What starting an ACP agent needs beyond its catalogue entry.
pub(crate) struct Start {
    pub task_id: String,
    pub checkout_id: Option<String>,
    pub cwd: String,
    pub title: String,
    pub prompt: Option<String>,
    pub resume: Resume,
}

/// Start `def` over ACP in a pane of its own (ACP-1). Nothing is written for
/// the CLI to load: no hooks, no plugin, no MCP config. The app's tools go to
/// it over its stdin (ACP-3), and it reports what it is doing through the
/// protocol (ACP-2).
pub(crate) fn start(app: &AppHandle, state: &AppState, def: &agents::AgentDef, s: Start) -> Result<PaneInfo> {
    let cmd = def.acp.ok_or_else(|| Error::Other(format!("{} cannot run as a conversation (ACP)", def.name)))?;
    let program =
        shellenv::which(cmd.program).ok_or_else(|| Error::NotFound(format!("{} is not on your PATH", cmd.program)))?;
    let pick = match &s.resume {
        Resume::No => acp::Pick::New,
        Resume::Newest => acp::Pick::Newest,
        Resume::Restart { session: Some(id), .. } => acp::Pick::Id(id.clone()),
        Resume::Restart { session: None, .. } => acp::Pick::New,
    };
    let mcp = crate::mcp::endpoint().map(|e| acp::Mcp {
        name: crate::mcp::SERVER_NAME.to_string(),
        url: e.url.clone(),
        token: e.token.clone(),
    });
    let handle = app.clone();
    // Remembered with the pane, so a restore or a restart picks up this
    // conversation and no other (ACP-9). `remember_pane` reads it too, for a
    // conversation that opened before the pane was first remembered.
    let agent = def.id.to_string();
    let on_session: acp::OnSession = Box::new(move |pane, session, offered| {
        let models = models_offered(offered);
        let _ = handle.state::<AppState>().config.update(|c| {
            for p in c.saved_panes.iter_mut().filter(|p| p.id == pane) {
                p.session = Some(session.to_string());
            }
            // The choices for the bot's model (BOT-17): an agent names its
            // models only to a conversation, so the last one's are kept.
            if !models.is_empty() {
                c.agent_models.insert(agent.clone(), models);
            }
        });
    });
    // Picking a conversation up hands it back to the agent, so an opening
    // prompt would only talk over it.
    let prompt = s.prompt.filter(|p| !p.trim().is_empty() && s.resume == Resume::No);
    let opts = SpawnOptions {
        task_id: s.task_id,
        checkout_id: s.checkout_id,
        cwd: s.cwd,
        kind: PaneKind::Agent,
        title: s.title,
        program,
        args: cmd.args.iter().map(|a| a.to_string()).collect(),
        agent_id: Some(def.id.to_string()),
        rows: None,
        cols: None,
        initial_input: None,
        prompted: prompt.is_some(),
        env: Vec::new(),
        title_activity: None,
        title_topic: None,
    };
    // Asked before the config is read: `is_room` reads it too.
    let henchman = super::henchman::is_room(state, std::path::Path::new(&opts.cwd));
    // A routine's run, on its routine's model and effort (ROUT-4).
    let presets = match super::routines::run_agent(app, state, &opts.cwd) {
        Some(chosen) => super::henchman::presets(&chosen, def.id),
        None => launch_presets(&state.config.read(), &opts.cwd, henchman, def.id),
    };
    let meta = henchman.then(super::henchman::session_meta);
    state.ptys.spawn_acp(app, opts, acp::Launch { mcp, pick, prompt, presets, meta, on_session: Some(on_session) })
}

/// What an agent opening in `cwd` is set to before its first prompt, as
/// (category, value). The Henchman always works in `auto` (HEN-2), on its
/// own model and effort (HEN-15); a Slack
/// thread's chat as the bot is set (BOT-12, BOT-17), so turning *Work
/// without asking* off there still holds; every other agent in `auto`
/// while the owner leaves that on (PANE-18). A value the agent doesn't
/// offer is skipped when it is set.
fn launch_presets(config: &crate::config::AppConfig, cwd: &str, henchman: bool, agent: &str) -> Vec<(String, String)> {
    if henchman {
        super::henchman::presets(&config.henchman.agent, agent)
    } else if crate::slack_bot::is_thread_room(config, cwd) {
        crate::slack_bot::presets_for(config, cwd)
    } else if config.ui.agents_auto_mode {
        vec![("mode".to_string(), "auto".to_string())]
    } else {
        Vec::new()
    }
}

/// The models a conversation was offered: its settings in the `model`
/// category.
fn models_offered(offered: &[acp::Setting]) -> Vec<ModelChoice> {
    offered
        .iter()
        .filter(|s| s.category.as_deref() == Some("model"))
        .flat_map(|s| &s.options)
        .map(|o| ModelChoice {
            value: o.value.clone(),
            name: if o.name.is_empty() { o.value.clone() } else { o.name.clone() },
        })
        .collect()
}

/// What changed in a conversation after `since`, and the pane is on screen
/// from now (ACP-7). Undone by `pty_detach`, as for a terminal.
#[tauri::command]
pub fn acp_view(app: AppHandle, state: State<AppState>, pane_id: String, since: Option<u64>) -> Result<acp::View> {
    let (view, seen) = state.ptys.acp_view(&pane_id, since)?;
    if seen {
        let _ = app.emit("pty:activity", &pane_id);
    }
    Ok(view)
}

/// Send a prompt, whole (ACP-4), with any images pasted into it (ACP-16).
#[tauri::command]
pub fn acp_prompt(state: State<AppState>, pane_id: String, text: String, images: Vec<acp::Image>) -> Result<()> {
    state.ptys.acp_prompt(&pane_id, &text, images)
}

/// Stop the running turn (ACP-6).
#[tauri::command]
pub fn acp_cancel(state: State<AppState>, pane_id: String) -> Result<bool> {
    state.ptys.acp_cancel(&pane_id)
}

/// Answer a permission question with one of its options (ACP-5).
#[tauri::command]
pub fn acp_answer(state: State<AppState>, pane_id: String, entry: usize, option: String) -> Result<()> {
    state.ptys.acp_answer(&pane_id, entry, &option)
}

/// Change a setting the agent offers: its mode, its model (ACP-12).
#[tauri::command]
pub fn acp_set(state: State<AppState>, pane_id: String, setting: String, value: String) -> Result<()> {
    state.ptys.acp_set(&pane_id, &setting, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, SlackBotConfig, SlackThread};

    fn auto() -> Vec<(String, String)> {
        vec![("mode".to_string(), "auto".to_string())]
    }

    fn with_bot(bot_auto: bool, agents_auto: bool) -> AppConfig {
        let bot = SlackBotConfig {
            owner: "UME".into(),
            agent_id: "claude".into(),
            bot_user: "UBOT".into(),
            bot_name: "drdoom".into(),
            enabled: true,
            auto: bot_auto,
            personality: String::new(),
            model: String::new(),
        };
        let thread = SlackThread {
            channel: "C1".into(),
            ts: "1.1".into(),
            room: "/chat/slack".into(),
            label: "#ops".into(),
            link: String::new(),
            seen: String::new(),
        };
        let mut config = AppConfig { slack_bot: Some(bot), slack_threads: vec![thread], ..Default::default() };
        config.ui.agents_auto_mode = agents_auto;
        config
    }

    #[test]
    fn task_agents_and_chats_open_in_auto_mode_while_the_owner_leaves_it_on() {
        assert_eq!(launch_presets(&AppConfig::default(), "/work/T-1", false, "claude"), auto());
        let mut off = AppConfig::default();
        off.ui.agents_auto_mode = false;
        assert!(launch_presets(&off, "/work/T-1", false, "claude").is_empty());
    }

    #[test]
    fn the_henchman_opens_in_auto_mode_on_sonnet_at_medium_effort_whatever_the_setting() {
        let mut want = auto();
        want.extend([("model".into(), "sonnet".into()), ("thought_level".into(), "medium".into())]);
        assert_eq!(launch_presets(&with_bot(false, false), "/chat/henchman", true, "claude"), want);
    }

    #[test]
    fn the_henchmans_model_is_set_only_on_the_agent_it_was_chosen_for() {
        assert_eq!(launch_presets(&AppConfig::default(), "/chat/henchman", true, "copilot"), auto());
        let mut config = AppConfig::default();
        config.henchman.agent.model = String::new();
        let want = vec![("mode".into(), "auto".into()), ("thought_level".into(), "medium".into())];
        assert_eq!(launch_presets(&config, "/chat/henchman", true, "claude"), want, "an empty one is the agent's own");
    }

    #[test]
    fn a_slack_thread_follows_the_bot_not_the_owners_setting() {
        assert!(launch_presets(&with_bot(false, true), "/chat/slack", false, "claude").is_empty());
        assert_eq!(launch_presets(&with_bot(true, false), "/chat/slack", false, "claude"), auto());
        assert_eq!(launch_presets(&with_bot(false, true), "/chat/mine", false, "claude"), auto());
    }
}
