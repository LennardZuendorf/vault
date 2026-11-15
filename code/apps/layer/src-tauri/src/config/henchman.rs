//! What the Henchman answers from, and whom it leaves alone (HEN-7, HEN-8).
//! The user's own, on this Mac: not part of the setup a team shares.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HenchmanConfig {
    /// Standing orders, oldest first (HEN-7).
    pub orders: Vec<StandingOrder>,
    /// The tasks (by id) and chats (by room) it hears nothing of (HEN-8).
    pub left_alone: Vec<String>,
}

/// One thing the user told the Henchman to answer from now on, in their
/// words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StandingOrder {
    pub id: String,
    pub text: String,
}
