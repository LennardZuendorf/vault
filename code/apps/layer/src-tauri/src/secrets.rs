//! Tokens live in the macOS keychain, never in config.json.
//!
//! Every integration shares **one** keychain item rather than one each, and
//! it is read at most once per run. macOS prompts per item per process, so three
//! items read on every settings fetch meant a stack of password dialogs on every
//! launch. One item, cached, means at most one prompt — and none at all unless
//! something actually needs a token.

use std::collections::HashMap;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

use crate::error::Result;

/// The release build's identifier, and the service its tokens were saved under.
const SERVICE: &str = "eu.codevillain.villain-layer";
static SERVICE_NAME: OnceLock<String> = OnceLock::new();

/// Keep tokens under this build's own identifier. The dev build has its own
/// config for the same reason: sharing one keychain item with the installed
/// app, each wrote back the whole bundle it had cached, and whichever wrote
/// last erased a token the other had just saved.
pub fn use_service(identifier: &str) {
    let _ = SERVICE_NAME.set(identifier.to_string());
}

fn service() -> &'static str {
    SERVICE_NAME.get().map(String::as_str).unwrap_or(SERVICE)
}
/// The single item holding every token, as a JSON object.
const BUNDLE: &str = "tokens";

pub const JIRA: &str = "jira-token";
pub const GITHUB: &str = "github-token";
const LEGACY_GITHUB_ORIGIN: &str = "github-token-origin";
const GITHUB_BINDING: &str = "github-token-binding";
/// Notifications' own Slack token, from before they were posted by the bot
/// (§24). Read only to forget it: `forget_old_slack`.
pub const SLACK: &str = "slack-token";
pub const GRAFANA: &str = "grafana-token";
/// The Slack bot's own bot token (`xoxb-`), which notifications post with
/// too, and its app-level token (`xapp-`, which only opens the Socket Mode
/// connection) (§24).
pub const SLACK_BOT: &str = "slack-bot-token";
pub const SLACK_BOT_APP: &str = "slack-bot-app-token";

/// The session a workspace on a server signed in with (PROF-16).
pub const SESSION: &str = "layer-session";

/// Where a saved browser sign-in's password is kept (BRW-18).
pub fn sign_in_key(id: &str) -> String {
    format!("browser-sign-in-{id}")
}

/// The v1 layout: one keychain item per integration.
const LEGACY: [&str; 3] = [JIRA, GITHUB, SLACK];

type Bundle = HashMap<String, String>;

static CACHE: OnceLock<Mutex<Option<Bundle>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<Bundle>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn entry(account: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(service(), account)?)
}

fn read_item(account: &str) -> Result<Option<String>> {
    match entry(account)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_item(account: &str, value: &str) -> Result<()> {
    entry(account)?.set_password(value)?;
    Ok(())
}

fn delete_item(account: &str) -> Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Read the bundle, falling back to a one-time migration of the v1 per-service
/// items. Cached for the life of the process.
fn load() -> Result<Bundle> {
    let mut guard = cache().lock();
    if let Some(bundle) = guard.as_ref() {
        return Ok(bundle.clone());
    }
    // Tests keep the bundle in memory: a test run must not read, prompt
    // for or write the keychain the user's own tokens are in.
    if cfg!(test) {
        *guard = Some(Bundle::new());
        return Ok(Bundle::new());
    }

    let bundle = match read_item(BUNDLE)? {
        Some(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        None => {
            // Fold any v1 items into the bundle, then retire them. This is the
            // only time a launch can prompt more than once.
            let mut migrated = Bundle::new();
            for account in LEGACY {
                if let Ok(Some(value)) = read_item(account) {
                    migrated.insert(account.to_string(), value);
                }
            }
            if !migrated.is_empty() {
                write_item(BUNDLE, &serde_json::to_string(&migrated)?)?;
                for account in LEGACY {
                    let _ = delete_item(account);
                }
            }
            migrated
        }
    };

    *guard = Some(bundle.clone());
    Ok(bundle)
}

fn store(bundle: Bundle) -> Result<()> {
    if cfg!(test) {
        *cache().lock() = Some(bundle);
        return Ok(());
    }
    if bundle.is_empty() {
        delete_item(BUNDLE)?;
    } else {
        write_item(BUNDLE, &serde_json::to_string(&bundle)?)?;
    }
    *cache().lock() = Some(bundle);
    Ok(())
}

pub fn get(key: &str) -> Result<Option<String>> {
    Ok(load()?.get(key).cloned())
}

/// Held across a read-change-write of the bundle.
///
/// Every token is in one item, so a change is read, edit, write back — and
/// two at once, connecting Jira while GitHub reconnects, each wrote back the
/// bundle it had read, and whichever landed second erased the other token.
static WRITING: Mutex<()> = Mutex::new(());

pub fn set(key: &str, secret: &str) -> Result<()> {
    let _writing = WRITING.lock();
    let mut bundle = load()?;
    bundle.insert(key.to_string(), secret.to_string());
    if key == GITHUB {
        clear_github_binding(&mut bundle);
    }
    store(bundle)
}

pub(crate) struct GithubCredential {
    token: Option<String>,
    proof: Option<String>,
}

impl GithubCredential {
    fn from_bundle(mut bundle: Bundle) -> Self {
        Self { token: bundle.remove(GITHUB), proof: bundle.remove(GITHUB_BINDING) }
    }

    /// Existing branch reads may still use a legacy connection. This token
    /// is not proof of identity for a saved pin. Once a proof exists, a
    /// mismatch is not a legacy credential and must not be sent elsewhere.
    pub(crate) fn legacy_token(&self) -> Option<&str> {
        self.token.as_deref().filter(|_| self.proof.is_none())
    }

    pub(crate) fn for_origin(self, origin: &str) -> Result<String> {
        if let Some(token) =
            self.token.filter(|token| self.proof.as_deref().is_some_and(|proof| proof == github_binding(token, origin)))
        {
            return Ok(token);
        }
        Err(crate::error::Error::Other(
            "Reconnect GitHub in Settings and enter the token again. The saved token is not bound to this API origin."
                .into(),
        ))
    }

    #[cfg(test)]
    pub(crate) fn fixture(token: &str, origin: Option<&str>) -> Self {
        let mut bundle = Bundle::from([(GITHUB.into(), token.into())]);
        if let Some(origin) = origin {
            write_github_credential(&mut bundle, Some((token, origin)));
        }
        Self::from_bundle(bundle)
    }

    #[cfg(test)]
    pub(crate) fn fixture_after_older_rewrite(token: &str, origin: &str, replacement: &str) -> Self {
        let mut bundle = Bundle::new();
        write_github_credential(&mut bundle, Some((token, origin)));
        // Older builds changed only keys they knew; they kept other entries.
        bundle.insert(GITHUB.into(), replacement.into());
        Self::from_bundle(bundle)
    }
}

pub(crate) fn github_credential() -> Result<GithubCredential> {
    let bundle =
        load().map_err(|_| crate::error::Error::Other("Could not read the GitHub connection. Try again.".into()))?;
    Ok(GithubCredential::from_bundle(bundle))
}

/// A downgraded build can replace the token while preserving unknown bundle
/// keys. The versioned proof must describe both this exact token and its origin.
fn github_binding(token: &str, origin: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"villain-layer github token binding v1\0");
    hash.update((origin.len() as u64).to_be_bytes());
    hash.update(origin.as_bytes());
    hash.update(token.as_bytes());
    format!("v1:{}", hex::encode(hash.finalize()))
}

fn clear_github_binding(bundle: &mut Bundle) {
    bundle.remove(GITHUB_BINDING);
    bundle.remove(LEGACY_GITHUB_ORIGIN);
}

/// The token and its proof must be written or cleared in one Keychain item
/// update. A config write failure must never bind a new token to an old origin.
pub(crate) fn set_github_credential(verified: Option<(&str, &str)>) -> Result<()> {
    let _writing = WRITING.lock();
    let mut bundle = load()?;
    write_github_credential(&mut bundle, verified);
    store(bundle)
}

fn write_github_credential(bundle: &mut Bundle, verified: Option<(&str, &str)>) {
    clear_github_binding(bundle);
    match verified {
        Some((token, origin)) => {
            bundle.insert(GITHUB.into(), token.into());
            bundle.insert(GITHUB_BINDING.into(), github_binding(token, origin));
        }
        None => {
            bundle.remove(GITHUB);
        }
    }
}

/// Drop the token notifications had before the bot posted them. Their
/// channel and switches stay in the config and go on under the bot. Called
/// where the bundle is read anyway, so it never adds a keychain prompt.
pub fn forget_old_slack() -> Result<()> {
    if get(SLACK)?.is_some() {
        delete(SLACK)?;
    }
    Ok(())
}

pub fn delete(key: &str) -> Result<()> {
    let _writing = WRITING.lock();
    let mut bundle = load()?;
    bundle.remove(key);
    if key == GITHUB {
        clear_github_binding(&mut bundle);
    }
    store(bundle)
}

/// Delete every token saved under `service`: another profile's, as it is
/// deleted (PROF-9).
pub fn forget_service(service: &str) -> Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    match keyring::Entry::new(service, BUNDLE)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Save `secret` under `key` in another profile's own item: a workspace
/// opened from this window keeps the session it signed in with (PROF-16).
/// The item is new, so nothing in it is lost.
pub fn seed(service: &str, key: &str, secret: &str) -> Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    let item = keyring::Entry::new(service, BUNDLE)?;
    let mut bundle: Bundle = match item.get_password() {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(keyring::Error::NoEntry) => Bundle::new(),
        Err(e) => return Err(e.into()),
    };
    bundle.insert(key.to_string(), secret.to_string());
    item.set_password(&serde_json::to_string(&bundle)?)?;
    Ok(())
}

/// Copy the tokens `from`, another identifier, saved into this build's own
/// item, unless this build has one already (DISK-4). Reading another
/// identifier's item is what prompts, once.
pub fn adopt(from: &str) -> Result<()> {
    let _writing = WRITING.lock();
    if read_item(BUNDLE)?.is_some() {
        return Ok(());
    }
    let saved = match keyring::Entry::new(from, BUNDLE)?.get_password() {
        Ok(v) => v,
        Err(keyring::Error::NoEntry) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    write_item(BUNDLE, &saved)?;
    *cache().lock() = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_two_an_older_builds_token_rewrite_invalidates_the_origin_binding() {
        let origin = "https://api.github.com";
        let answer = GithubCredential::fixture_after_older_rewrite("verified-token", origin, "replacement-token")
            .for_origin(origin);
        assert!(answer.is_err(), "an old build's new token must not inherit the origin binding");
        let error = answer.err().unwrap();
        assert!(error.to_string().contains("Reconnect GitHub"));
        assert!(!error.to_string().contains("replacement-token"));
    }

    #[test]
    fn review_two_a_legacy_or_unknown_proof_requires_reconnect() {
        let origin = "https://api.github.com";
        let token = "fixture-token";
        let mut bundle = Bundle::from([(GITHUB.into(), token.into()), (LEGACY_GITHUB_ORIGIN.into(), origin.into())]);
        assert!(GithubCredential::from_bundle(bundle.clone()).for_origin(origin).is_err());
        write_github_credential(&mut bundle, Some((token, origin)));
        assert_eq!(GithubCredential::from_bundle(bundle.clone()).for_origin(origin).unwrap(), token);
        assert!(GithubCredential::from_bundle(bundle.clone()).for_origin("https://api.other.test").is_err());
        let proof = bundle.get_mut(GITHUB_BINDING).unwrap();
        proof.replace_range(..3, "v9:");
        assert!(GithubCredential::from_bundle(bundle.clone()).for_origin(origin).is_err());
        write_github_credential(&mut bundle, None);
        assert!(bundle.is_empty());
    }
}
