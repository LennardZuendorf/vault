//! A pull request between draft and ready for review (PR-15).
//!
//! GitHub's REST API ignores `draft` on an existing pull request: only
//! GraphQL's two mutations move it, and they take the PR's node id rather
//! than its number.

use serde_json::{json, Value};

use super::{GitHub, PullRequest};
use crate::error::{Error, Result};

impl GitHub {
    /// REST returns the GraphQL node ID alongside the verified database ID.
    /// Do not look up the number again in a separate, unchecked query.
    pub(crate) async fn pull_edit_node(&self, owner: &str, repo: &str, pr: &PullRequest) -> Result<String> {
        let v = self.read_edit_target(owner, repo, pr).await?;
        v.get("node_id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned).ok_or_else(|| {
            Error::Other("GitHub did not identify the selected pull request. Nothing was edited.".into())
        })
    }

    pub(crate) async fn set_pull_draft(&self, node: &str, draft: bool) -> Result<bool> {
        let v = self.graphql(mutation(draft), json!({ "id": node })).await?;
        let field = if draft { "convertPullRequestToDraft" } else { "markPullRequestReadyForReview" };
        Ok(answer(&v, &format!("/data/{field}/pullRequest/isDraft"))?.as_bool().unwrap_or(draft))
    }
}

fn mutation(draft: bool) -> &'static str {
    if draft {
        "mutation($id: ID!) { convertPullRequestToDraft(input: { pullRequestId: $id }) { pullRequest { isDraft } } }"
    } else {
        "mutation($id: ID!) { markPullRequestReadyForReview(input: { pullRequestId: $id }) { pullRequest { isDraft } } }"
    }
}

/// What sits at `path`, or GitHub's own reason it isn't there: GraphQL
/// answers 200 with the failure inside, such as a token that may read the
/// PR but not change it.
fn answer<'a>(v: &'a Value, path: &str) -> Result<&'a Value> {
    if let Some(msg) = v.pointer("/errors/0/message").and_then(|m| m.as_str()) {
        return Err(Error::Other(format!("GitHub: {msg}")));
    }
    v.pointer(path).filter(|x| !x.is_null()).ok_or_else(|| Error::Other("GitHub did not say what became of it".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_way_asks_for_its_own_mutation() {
        assert!(mutation(true).contains("convertPullRequestToDraft"));
        assert!(mutation(false).contains("markPullRequestReadyForReview"));
    }

    #[test]
    fn a_refusal_inside_a_200_is_an_error_naming_githubs_reason() {
        let v = json!({ "data": { "markPullRequestReadyForReview": null },
                        "errors": [{ "message": "Resource not accessible by integration" }] });
        let err = answer(&v, "/data/markPullRequestReadyForReview/pullRequest/isDraft").map(|_| ());
        assert_eq!(err.map_err(|e| e.to_string()), Err("GitHub: Resource not accessible by integration".into()));
    }

    #[test]
    fn the_new_state_is_read_from_the_answer() {
        let v = json!({ "data": { "convertPullRequestToDraft": { "pullRequest": { "isDraft": true } } } });
        assert_eq!(answer(&v, "/data/convertPullRequestToDraft/pullRequest/isDraft").ok(), Some(&json!(true)));
    }
}
