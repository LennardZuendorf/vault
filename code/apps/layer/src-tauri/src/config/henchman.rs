//! What the Henchman answers from, and whom it leaves alone (HEN-7, HEN-8).
//! The user's own, on this Mac: not part of the setup a team shares.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HenchmanConfig {
    /// Whether this profile lets Henchman run (HEN-16).
    pub enabled: bool,
    /// The user's additional guidance for the next conversation open (HEN-17).
    pub instructions: String,
    /// Standing orders, oldest first (HEN-7).
    pub orders: Vec<StandingOrder>,
    /// The tasks (by id) and chats (by room) it hears nothing of (HEN-8).
    pub left_alone: Vec<String>,
    /// What it is woken with (HEN-15).
    pub agent: HenchmanAgent,
}

impl Default for HenchmanConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            instructions: String::new(),
            orders: Vec::new(),
            left_alone: Vec::new(),
            agent: HenchmanAgent::default(),
        }
    }
}

/// The agent the Henchman is woken with, and the model and effort its
/// conversation opens on, put back at launch too (HEN-15). An empty model
/// or effort is the agent's own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HenchmanAgent {
    pub agent_id: String,
    pub model: String,
    pub effort: String,
}

/// Sonnet at medium effort: the Henchman reads Layer and passes words on,
/// and writes no code. Left to Claude Code, it opened on the default model
/// at the effort the user set for their own work, Xhigh, for a brief.
impl Default for HenchmanAgent {
    fn default() -> Self {
        Self { agent_id: "claude".into(), model: "sonnet".into(), effort: "medium".into() }
    }
}

/// One thing the user told the Henchman to answer from now on, in their
/// words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StandingOrder {
    pub id: String,
    pub text: String,
}
