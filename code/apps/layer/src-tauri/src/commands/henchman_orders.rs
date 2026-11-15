//! The Henchman's standing orders, and the tasks and chats it leaves alone
//! (HEN-7, HEN-8): kept in the config, changed by its tools on the user's
//! word and removed from the lair. Each change is told to the window as
//! `henchman:orders`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::config::{AppConfig, StandingOrder};
use crate::error::{Error, Result};
use crate::pty::PaneInfo;

use super::panes::CHAT_TASK_ID;
use super::AppState;

/// More than this is a list nobody reads, the Henchman included: each one
/// goes into every message it is sent.
const MAX_ORDERS: usize = 40;
/// An order is a sentence or two, not a document.
const MAX_ORDER: usize = 500;

/// What the lair shows of them.
#[derive(Debug, Clone, Serialize)]
pub struct Orders {
    pub orders: Vec<StandingOrder>,
    pub left_alone: Vec<LeftAlone>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeftAlone {
    /// The task's id, or the chat's room.
    pub key: String,
    /// Its ticket key or name; the chat's name.
    pub name: String,
}

#[tauri::command]
pub fn henchman_orders(state: State<AppState>) -> Orders {
    orders(&state)
}

/// One the user writes in the lair, rather than tells the Henchman.
#[tauri::command]
pub fn henchman_order_add(app: AppHandle, state: State<AppState>, text: String) -> Result<Orders> {
    add(&app, &state, &text)?;
    Ok(orders(&state))
}

#[tauri::command]
pub fn henchman_order_remove(app: AppHandle, state: State<AppState>, id: String) -> Result<Orders> {
    remove(&app, &state, &id)?;
    Ok(orders(&state))
}

/// Leave a task or chat alone, or hand it back (HEN-8). By the key the
/// lair lists, so a chat that is closed can be handed back too.
#[tauri::command]
pub fn henchman_leave_alone(app: AppHandle, state: State<AppState>, key: String, alone: bool) -> Result<Orders> {
    state.config.update(|c| set_alone(c, &key, alone))?;
    changed(&app);
    Ok(orders(&state))
}

pub(crate) fn orders(state: &AppState) -> Orders {
    let panes = state.ptys.list(Some(CHAT_TASK_ID));
    state.config.with(|c| Orders {
        orders: c.henchman.orders.clone(),
        left_alone: c
            .henchman
            .left_alone
            .iter()
            .map(|k| LeftAlone { key: k.clone(), name: name_of(c, &panes, k) })
            .collect(),
    })
}

/// What a pane is left alone by: its task, or for a chat its room, which
/// outlives the pane across restarts.
pub(crate) fn key_of(p: &PaneInfo) -> &str {
    if p.task_id == CHAT_TASK_ID {
        &p.cwd
    } else {
        &p.task_id
    }
}

pub(crate) fn add(app: &AppHandle, state: &AppState, text: &str) -> Result<StandingOrder> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Other("an order needs words".into()));
    }
    if text.chars().count() > MAX_ORDER {
        return Err(Error::Other(format!("an order is at most {MAX_ORDER} characters: say it shorter")));
    }
    let order = StandingOrder { id: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(), text: text.into() };
    let kept = order.clone();
    state.config.update(|c| push(c, kept))??;
    changed(app);
    Ok(order)
}

pub(crate) fn remove(app: &AppHandle, state: &AppState, id: &str) -> Result<()> {
    let found = state.config.update(|c| {
        let before = c.henchman.orders.len();
        c.henchman.orders.retain(|o| o.id != id);
        c.henchman.orders.len() != before
    })?;
    if !found {
        return Err(Error::NotFound(format!("standing order {id}")));
    }
    changed(app);
    Ok(())
}

/// Leave the task or chat a pane belongs to alone, or hand it back.
pub(crate) fn leave_pane_alone(app: &AppHandle, state: &AppState, pane: &PaneInfo, alone: bool) -> Result<()> {
    let key = key_of(pane).to_string();
    state.config.update(|c| set_alone(c, &key, alone))?;
    changed(app);
    Ok(())
}

fn push(c: &mut AppConfig, order: StandingOrder) -> Result<()> {
    if c.henchman.orders.len() >= MAX_ORDERS {
        return Err(Error::Other(format!(
            "there are {MAX_ORDERS} standing orders already: remove one first, or fold two into one"
        )));
    }
    if !c.henchman.orders.iter().any(|o| o.text == order.text) {
        c.henchman.orders.push(order);
    }
    Ok(())
}

fn set_alone(c: &mut AppConfig, key: &str, alone: bool) {
    c.henchman.left_alone.retain(|k| k != key);
    if alone {
        c.henchman.left_alone.push(key.to_string());
    }
}

fn name_of(c: &AppConfig, chats: &[PaneInfo], key: &str) -> String {
    if let Some(t) = c.tasks.iter().find(|t| t.id == key) {
        return t.issue_key.clone().unwrap_or_else(|| t.name.clone());
    }
    match chats.iter().find(|p| p.cwd == key) {
        Some(p) => format!("Chat · {}", p.topic.clone().unwrap_or_else(|| p.title.clone())),
        None if key.contains("/_chat/") => "A closed chat".into(),
        None => "A task since deleted".into(),
    }
}

fn changed(app: &AppHandle) {
    let _ = app.emit("henchman:orders", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(text: &str) -> StandingOrder {
        StandingOrder { id: text.into(), text: text.into() }
    }

    #[test]
    fn an_order_said_twice_is_kept_once() {
        let mut c = AppConfig::default();
        push(&mut c, order("yes to running the tests")).unwrap();
        push(&mut c, StandingOrder { id: "other".into(), ..order("yes to running the tests") }).unwrap();
        assert_eq!(c.henchman.orders.len(), 1);
    }

    #[test]
    fn past_the_limit_an_order_is_refused_and_says_what_to_do() {
        let mut c = AppConfig::default();
        for i in 0..MAX_ORDERS {
            push(&mut c, order(&i.to_string())).unwrap();
        }
        let err = push(&mut c, order("one more")).unwrap_err().to_string();
        assert!(err.contains("remove one first"), "{err}");
    }

    #[test]
    fn left_alone_is_kept_once_and_handed_back_by_its_key() {
        let mut c = AppConfig::default();
        set_alone(&mut c, "task-1", true);
        set_alone(&mut c, "task-1", true);
        assert_eq!(c.henchman.left_alone, ["task-1"]);
        set_alone(&mut c, "task-1", false);
        assert!(c.henchman.left_alone.is_empty());
    }

    #[test]
    fn a_config_from_before_the_henchman_had_orders_still_reads() {
        let c: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(c.henchman.orders.is_empty());
        let c: AppConfig = serde_json::from_str(r#"{"henchman":{}}"#).unwrap();
        assert!(c.henchman.left_alone.is_empty());
    }
}
