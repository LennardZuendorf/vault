//! Where a repository's origin is, read from its remote URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::run;
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubOrigin {
    pub host: String,
    pub owner: String,
    pub repo: String,
    /// HTTPS uses the web origin. SSH has none until a GitHub source is resolved.
    pub web_origin: Option<String>,
}

impl GithubOrigin {
    pub fn parse(remote: &str) -> Result<Self> {
        let invalid = || Error::Git("origin is not a GitHub repository URL".into());
        let remote = remote.trim();
        let (url, web) = if remote.contains("://") {
            let url = reqwest::Url::parse(remote).map_err(|_| invalid())?;
            let web = match url.scheme() {
                "https" | "http" => Some(url.origin().ascii_serialization()),
                "ssh" => None,
                _ => return Err(invalid()),
            };
            (url, web)
        } else {
            let (authority, path) = remote.split_once(':').ok_or_else(invalid)?;
            if authority.contains('/') || path.starts_with('/') {
                return Err(invalid());
            }
            (reqwest::Url::parse(&format!("ssh://{authority}/{path}")).map_err(|_| invalid())?, None)
        };
        let host =
            url.host_str().filter(|s| !s.is_empty()).ok_or_else(invalid)?.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() || url.query().is_some() || url.fragment().is_some() {
            return Err(invalid());
        }
        let path = url.path().trim_matches('/');
        let (owner, repo) = path.split_once('/').ok_or_else(invalid)?;
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        let segment = |value: &str| {
            !value.is_empty()
                && !value.starts_with('-')
                && !matches!(value, "." | "..")
                && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        };
        if !segment(owner) || !segment(repo) {
            return Err(invalid());
        }
        Ok(Self { host, owner: owner.into(), repo: repo.into(), web_origin: web })
    }

    pub fn slug(&self) -> (&str, &str) {
        (&self.owner, &self.repo)
    }

    pub fn matches(&self, expected: &Self) -> bool {
        self.host == expected.host
            && self.owner.eq_ignore_ascii_case(&expected.owner)
            && self.repo.eq_ignore_ascii_case(&expected.repo)
            && (self.web_origin.is_none() || self.web_origin == expected.web_origin)
    }
}

pub fn github_origin(dir: &Path) -> Result<GithubOrigin> {
    let effective = run(dir, &["remote", "get-url", "origin"])?;
    GithubOrigin::parse(&effective)
}

pub fn verify_github_push_origin(dir: &Path, expected: &GithubOrigin) -> Result<()> {
    let push = run(dir, &["remote", "get-url", "--push", "--all", "origin"])?;
    let mut urls = push.lines();
    let actual = urls.next().and_then(|url| GithubOrigin::parse(url).ok());
    if urls.next().is_some()
        || !actual.is_some_and(|r| r.matches(expected))
        || !github_origin(dir).is_ok_and(|r| r.matches(expected))
    {
        return Err(Error::Git("origin fetch and push do not name the same GitHub repository".into()));
    }
    Ok(())
}

/// How long an origin read by `github_origin_cached` is trusted. A remote changed
/// by hand outside the app is seen within this; one the app moves is seen
/// at once (`forget_origins`).
const ORIGIN_KEPT: Duration = Duration::from_secs(10 * 60);

/// The full origin, and when it was read.
type Read = (Instant, GithubOrigin);

/// Origins read, by folder.
static ORIGINS: LazyLock<Mutex<HashMap<PathBuf, Read>>> = LazyLock::new(Default::default);

/// `github_origin`, remembered for a while, for the PR sweep. It asked git for
/// every repository of every task every 90 seconds, for a URL that changes
/// about never. Only a valid origin is remembered: a failure is asked again.
pub fn github_origin_cached(dir: &Path) -> Result<GithubOrigin> {
    if let Some((at, remote)) = ORIGINS.lock().get(dir) {
        if at.elapsed() < ORIGIN_KEPT {
            return Ok(remote.clone());
        }
    }
    let remote = github_origin(dir)?;
    let mut origins = ORIGINS.lock();
    // Folders of checkouts since removed go with the rest of the stale.
    origins.retain(|_, (at, _)| at.elapsed() < ORIGIN_KEPT);
    origins.insert(dir.to_path_buf(), (Instant::now(), remote.clone()));
    Ok(remote)
}

/// Forget every slug read: the app has pointed a copy at another origin, and
/// its worktrees with it.
pub fn forget_origins() {
    ORIGINS.lock().clear();
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
    fn a_github_origin_cannot_be_a_file_url() {
        let repo = std::env::temp_dir().join(format!("vl-remote-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q"]).unwrap();
        run(&repo, &["remote", "add", "origin", "file://localhost/acme/web.git"]).unwrap();
        let reported = github_origin_cached(&repo);
        let _ = std::fs::remove_dir_all(&repo);
        assert!(reported.is_err(), "{reported:?}");
    }

    #[test]
    fn github_remotes_keep_the_site_without_using_an_ssh_port_as_the_web_port() {
        for raw in ["git@Git.example.com:acme/web.git", "ssh://git@Git.example.com:2222/acme/web.git"] {
            let remote = GithubOrigin::parse(raw).unwrap();
            assert_eq!(remote.host, "git.example.com");
            assert_eq!(remote.slug(), ("acme", "web"));
            assert_eq!(remote.web_origin, None);
        }
        let remote = GithubOrigin::parse("https://github.com:443/acme/web.git").unwrap();
        assert_eq!(remote.host, "github.com");
        assert_eq!(remote.slug(), ("acme", "web"));
        assert_eq!(remote.web_origin.as_deref(), Some("https://github.com"));
    }

    #[test]
    fn an_origin_is_read_once_and_again_after_the_app_moves_it() {
        let dir = std::env::temp_dir().join(format!("vl-origin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]).unwrap();
        run(&dir, &["remote", "add", "origin", "git@github.com:acme/web.git"]).unwrap();
        assert_eq!(github_origin_cached(&dir).unwrap().slug(), ("acme", "web"));

        run(&dir, &["remote", "set-url", "origin", "git@ghe.example.com:acme/web.git"]).unwrap();
        assert_eq!(github_origin_cached(&dir).unwrap().host, "github.com", "remembered, not asked again");
        forget_origins();
        assert_eq!(github_origin_cached(&dir).unwrap().host, "ghe.example.com");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_repository_kept_by_a_folder_elsewhere_is_not_cloned_and_a_folder_there_is_kept() {
        let err = clone_by_key("/Users/ada/code/billing", Path::new("/tmp/billing")).unwrap_err();
        assert!(err.to_string().contains("a folder on another Mac"), "{err}");
        let there = std::env::temp_dir();
        let err = clone_by_key("github.com/acme/billing", &there).unwrap_err();
        assert!(err.to_string().contains("is there already"), "{err}");
    }
    #[test]
    fn routes_github_dot_repositories() {
        let origin = GithubOrigin::parse("https://github.com/acme/.github.git").unwrap();
        assert_eq!(origin.slug(), ("acme", ".github"));
    }

    #[test]
    fn an_https_push_cannot_bypass_the_web_origin_by_fetching_over_ssh() {
        let dir = std::env::temp_dir().join(format!("vl-push-origin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]).unwrap();
        run(&dir, &["remote", "add", "origin", "ssh://git@git.example.com:2222/acme/web.git"]).unwrap();
        run(&dir, &["remote", "set-url", "--push", "origin", "https://git.example.com:8443/acme/web.git"]).unwrap();
        let mut expected = github_origin(&dir).unwrap();
        expected.web_origin = Some("https://git.example.com".into());
        let ssh = github_origin(&dir).unwrap();
        // The verified web address and the configured fetch transport both matter.
        let no_web_proof = verify_github_push_origin(&dir, &ssh);
        let wrong_web = verify_github_push_origin(&dir, &expected);
        run(&dir, &["remote", "set-url", "--push", "origin", "https://git.example.com/acme/web.git"]).unwrap();
        let right_web = verify_github_push_origin(&dir, &expected);
        let _ = std::fs::remove_dir_all(dir);
        assert!(no_web_proof.is_err());
        assert!(wrong_web.is_err());
        assert!(right_web.is_ok());
    }

    #[test]
    fn github_origin_uses_gits_effective_fetch_url() {
        let dir = std::env::temp_dir().join(format!("vl-effective-origin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]).unwrap();
        run(&dir, &["remote", "add", "origin", "https://github.com/old/web.git"]).unwrap();
        run(&dir, &["config", "url.https://git.example.com/new/.insteadOf", "https://github.com/old/"]).unwrap();
        let effective = github_origin(&dir).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!((effective.host.as_str(), effective.owner.as_str()), ("git.example.com", "new"));
    }
}
