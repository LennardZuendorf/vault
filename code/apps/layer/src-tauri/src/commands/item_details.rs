use serde::{Deserialize, Deserializer};

use crate::config::item_details::{normalize_tag, tag_identity, ItemColor, ItemDetails};
use crate::config::{AppConfig, ConfigStore};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DetailsPatch {
    #[serde(default, deserialize_with = "present")]
    pub display_name: Option<Option<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    pub color: Option<Option<ItemColor>>,
}

fn present<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone)]
pub enum DetailsTarget {
    Task(String),
    Chat(String),
}

#[derive(Debug)]
pub struct DetailsChange {
    pub details: ItemDetails,
    pub kind: &'static str,
    pub changed: bool,
    pub catalog_changed: bool,
}

#[derive(Debug)]
pub struct TagChange {
    pub tag: String,
    pub changed: bool,
    pub catalog_changed: bool,
}

/// Synchronous core for desktop and MCP callers, invoked through `blocking`.
pub fn edit_details(store: &ConfigStore, target: DetailsTarget, patch: DetailsPatch) -> Result<DetailsChange> {
    mutate(store, target, move |details| {
        if let Some(name) = patch.display_name {
            details.display_name = name;
        }
        if let Some(tags) = patch.tags {
            details.tags = tags;
        }
        if let Some(color) = patch.color {
            details.color = color;
        }
        Ok(())
    })
}

pub fn set_tag(store: &ConfigStore, target: DetailsTarget, tag: &str, assigned: bool) -> Result<DetailsChange> {
    mutate(store, target, |details| {
        let tag = required_tag(tag)?;
        let identity = tag_identity(&tag);
        if assigned {
            if !details.tags.iter().any(|old| tag_identity(old) == identity) {
                details.tags.push(tag);
            }
        } else {
            details.tags.retain(|old| tag_identity(old) != identity);
        }
        Ok(())
    })
}

pub fn add_tag(store: &ConfigStore, tag: &str) -> Result<TagChange> {
    store.edit_item_details(|cfg| {
        let tag = required_tag(tag)?;
        let before = cfg.tag_catalog.len();
        let tag = catalog_tag(&mut cfg.tag_catalog, tag);
        let changed = before != cfg.tag_catalog.len();
        Ok((TagChange { tag, changed, catalog_changed: changed }, changed))
    })
}

fn required_tag(raw: &str) -> Result<String> {
    normalize_tag(raw)?.ok_or_else(|| Error::Other("Tag is blank. Enter a tag name.".into()))
}

fn catalog_tag(catalog: &mut Vec<String>, tag: String) -> String {
    let identity = tag_identity(&tag);
    if let Some(saved) = catalog.iter().find(|saved| tag_identity(saved) == identity) {
        return saved.clone();
    }
    catalog.push(tag.clone());
    tag
}

impl DetailsTarget {
    fn checked<'a>(&'a self, cfg: &AppConfig) -> Result<(&'static str, &'a str)> {
        match self {
            Self::Task(id) if cfg.tasks.iter().any(|task| task.id == *id) => Ok(("task", id)),
            Self::Chat(room)
                if cfg.saved_panes.iter().any(|pane| {
                    pane.task_id == super::CHAT_TASK_ID
                        && pane.kind == "agent"
                        && pane.checkout_id.is_none()
                        && pane.cwd.as_deref() == Some(room.as_str())
                        && !super::henchman::is_saved(pane)
                        && !routine_room(room)
                }) =>
            {
                Ok(("chat", room))
            }
            Self::Task(id) => Err(Error::NotFound(format!("Work task {id}. Refresh Work and try again."))),
            Self::Chat(_) => Err(Error::NotFound("Chat room. Refresh Chat and try again.".into())),
        }
    }
}

fn routine_room(room: &str) -> bool {
    // Check old roots too. The room is only a lookup key, never opened.
    std::path::Path::new(room).ancestors().any(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "routines")
            && ancestor.parent().and_then(std::path::Path::file_name).is_some_and(|name| name == "_chat")
    })
}

fn mutate(
    store: &ConfigStore,
    target: DetailsTarget,
    edit: impl FnOnce(&mut ItemDetails) -> Result<()>,
) -> Result<DetailsChange> {
    store.edit_item_details(|cfg| {
        let (kind, key) = target.checked(cfg)?;
        let before = match &target {
            DetailsTarget::Task(_) => cfg.task_details.get(key),
            DetailsTarget::Chat(_) => cfg.chat_details.get(key),
        }
        .cloned()
        .unwrap_or_default();
        let mut details = before.clone();
        edit(&mut details)?;
        let mut details = details.normalize()?;
        let catalog_before = cfg.tag_catalog.len();
        for tag in &mut details.tags {
            *tag = catalog_tag(&mut cfg.tag_catalog, std::mem::take(tag));
        }
        let catalog_changed = cfg.tag_catalog.len() != catalog_before;
        let changed = before != details || catalog_changed;
        let map = match target {
            DetailsTarget::Task(_) => &mut cfg.task_details,
            DetailsTarget::Chat(_) => &mut cfg.chat_details,
        };
        if details == ItemDetails::default() {
            map.remove(key);
        } else {
            map.insert(key.to_owned(), details.clone());
        }
        Ok((DetailsChange { details, kind, changed, catalog_changed }, changed))
    })
}

#[cfg(test)]
mod tests;
