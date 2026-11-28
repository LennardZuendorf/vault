//! Where a repository's origin is, read from its remote URL.

use std::path::Path;

use super::run;
use crate::error::{Error, Result};

pub(crate) struct OriginIdentity {
    pub host: String,
    pub web_port: Option<u16>,
    pub ssh_port: Option<u16>,
    pub owner: String,
    pub repo: String,
}

/// Fresh, host-preserving origin for PR identity checks. Never fall back to
/// an enclosing repo, a cached slug, or a string that might contain a token.
pub(crate) fn checkout_origin(dir: &Path) -> Result<OriginIdentity> {
    super::verify_checkout_root(dir)?;
    let invalid = || Error::Git("The checkout's origin is not a Git hosting repository URL.".into());
    let raw = run(dir, &["remote", "get-url", "origin"])
        .map_err(|_| Error::Git("Could not read the checkout's origin.".into()))?;
    let raw = raw.trim_end_matches('\n');
    let url = if raw.contains("://") {
        reqwest::Url::parse(raw)
    } else {
        // scp-style URLs use a colon after the host, including bracketed IPv6.
        let colon = raw.find("]:").map(|i| i + 1).or_else(|| raw.find(':')).ok_or_else(invalid)?;
        let (host, path) = raw.split_at(colon);
        if host.is_empty() || host.contains('/') {
            return Err(invalid());
        }
        reqwest::Url::parse(&format!("ssh://{host}/{}", path[1..].trim_start_matches('/')))
    }
    .map_err(|_| invalid())?;
    if !matches!(url.scheme(), "https" | "http" | "ssh" | "git") || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid());
    }
    let host = url.host_str().ok_or_else(invalid)?.to_ascii_lowercase();
    let web_port = matches!(url.scheme(), "http" | "https").then(|| url.port_or_known_default()).flatten();
    let ssh_port = (url.scheme() == "ssh").then(|| url.port()).flatten();
    let path = url.path().trim_end_matches('/').strip_prefix('/').ok_or_else(invalid)?;
    let (owner, repo) = path.strip_suffix(".git").unwrap_or(path).split_once('/').ok_or_else(invalid)?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(invalid());
    }
    let (owner, repo) = (owner.into(), repo.into());
    Ok(OriginIdentity { host, web_port, ssh_port, owner, repo })
}

/// `owner/repo` parsed from the origin remote.
///
/// Handles the three shapes that turn up in practice, including the SSH form
/// with an explicit port that GitHub Enterprise installs often use:
///   git@host:acme/web.git
///   https://host/acme/web.git
///   ssh://git@host:2222/acme/web.git
pub fn origin_slug(dir: &Path) -> Result<(String, String)> {
    let url = run(dir, &["remote", "get-url", "origin"])?.trim().to_string();

    // A copy of a clone that had no origin yet fetches from the clone's
    // folder, and `/Users/me/work/platform/tool` read as `platform/tool`
    // sent GitHub a repository that does not exist: a 404 under the task's
    // pull requests, for a repo that was never on GitHub at all.
    let scp = !url.contains("://") && url.split_once(':').is_some_and(|(host, _)| !host.contains('/'));
    if !(url.contains("://") || scp) {
        return Err(Error::Git(format!("origin is a folder on this Mac ({url}), not GitHub")));
    }

    // Normalise scp-style `host:path` into `host/path` so one split works for
    // everything. A `//` right after the colon means it was a real scheme.
    let normalised = match url.split_once("://") {
        Some((_, rest)) => rest.to_string(),
        None => match url.split_once(':') {
            Some((host, path)) => format!("{host}/{path}"),
            None => url.clone(),
        },
    };

    let segments: Vec<&str> =
        normalised.trim_end_matches('/').trim_end_matches(".git").split('/').filter(|s| !s.is_empty()).collect();

    // The first segment is the host (possibly with userinfo and a port); the
    // last two are always owner and repo.
    if segments.len() >= 3 {
        let repo = segments[segments.len() - 1];
        let owner = segments[segments.len() - 2];
        return Ok((owner.to_string(), repo.to_string()));
    }

    Err(Error::Git(format!("cannot parse owner/repo from {url}")))
}

/// Clone the repository a workspace knows by its remote key
/// (`github.com/acme/billing`) into `into`, which must not exist yet
/// (REPO-9). Over SSH first, as most clones on a developer's Mac are, then
/// over HTTPS, for a public repository or one a credential helper knows.
pub fn clone_by_key(key: &str, into: &Path) -> Result<()> {
    let Some((host, path)) = key.split_once('/').filter(|(host, _)| !host.is_empty()) else {
        return Err(Error::Git(format!("{key} is a folder on another Mac, so there is nothing to clone it from")));
    };
    if into.exists() {
        return Err(Error::Other(format!("{} is there already", into.display())));
    }
    let parent = into.parent().ok_or_else(|| Error::Other(format!("{} has no folder around it", into.display())))?;
    let target = into.to_string_lossy();
    let said = |e: Error| match e {
        Error::Git(m) => m,
        e => e.to_string(),
    };
    let ssh = format!("git@{host}:{path}.git");
    let Err(over_ssh) = run(parent, &["clone", "--quiet", "--", &ssh, &target]) else { return Ok(()) };
    let https = format!("https://{key}.git");
    run(parent, &["clone", "--quiet", "--", &https, &target]).map(|_| ()).map_err(|over_https| {
        Error::Other(format!("Could not clone {key}.\nOver SSH: {}\nOver HTTPS: {}", said(over_ssh), said(over_https)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_kept_by_a_folder_elsewhere_is_not_cloned_and_a_folder_there_is_kept() {
        let err = clone_by_key("/Users/ada/code/billing", Path::new("/tmp/billing")).unwrap_err();
        assert!(err.to_string().contains("a folder on another Mac"), "{err}");
        let there = std::env::temp_dir();
        let err = clone_by_key("github.com/acme/billing", &there).unwrap_err();
        assert!(err.to_string().contains("is there already"), "{err}");
    }
}
