use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "source.toolkit.fluxcd.io",
    version = "v1",
    kind = "GitRepository",
    plural = "gitrepositories"
)]
#[kube(namespaced)]
#[kube(status = "GitRepositoryStatus")]
pub struct GitRepositorySpec {
    pub url: String,
    pub interval: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ref")]
    pub git_ref: Option<GitRepositoryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct GitRepositoryRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl GitRepository {
    /// Hostname of the source repo, e.g. `git.example.com`.
    /// Parsed from `spec.url`; see [`parse_git_url`].
    pub fn repo_host(&self) -> Option<String> {
        parse_git_url(&self.spec.url).map(|(host, _)| host)
    }

    /// Repo path below the host with no leading slash and no trailing
    /// `.git`, e.g. `group/subgroup/project`.
    pub fn repo_path(&self) -> Option<String> {
        parse_git_url(&self.spec.url).map(|(_, path)| path)
    }

    /// The checked-out ref: branch, else tag, else semver, else commit.
    /// (The ref *name* — a GitRepositoryRef with no branch/tag — is not a
    /// usable web ref, so it is deliberately ignored here.)
    pub fn repo_branch(&self) -> Option<String> {
        let r = self.spec.git_ref.as_ref()?;
        r.branch
            .clone()
            .or_else(|| r.tag.clone())
            .or_else(|| r.semver.clone())
            .or_else(|| r.commit.clone())
    }
}

/// Split a git remote URL into `(host, path)` where `path` has no leading
/// slash and no trailing `.git`. Handles `https://`/`http://`, `ssh://`,
/// and scp-style `git@host:org/repo.git`. Returns `None` if no host and
/// path can be separated.
pub(crate) fn parse_git_url(url: &str) -> Option<(String, String)> {
    let had_scheme = url.contains("://");
    // Drop the scheme, then any `user@` credential prefix.
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);

    let (host, path) = if had_scheme {
        // scheme URL: host[:port]/path
        let (host_port, path) = rest.split_once('/')?;
        let host = host_port.split(':').next().unwrap_or(host_port);
        (host, path)
    } else {
        // scp-style: host:path
        rest.split_once(':')?
    };
    if host.is_empty() {
        return None;
    }
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    Some((host.to_string(), path.to_string()))
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct GitRepositoryStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<GitRepositoryArtifact>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "observedGeneration"
    )]
    pub observed_generation: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct GitRepositoryArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lastUpdateTime"
    )]
    pub last_update_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::parse_git_url;

    #[test]
    fn parses_https_url() {
        assert_eq!(
            parse_git_url("https://gitlab.example.com/group/project.git"),
            Some(("gitlab.example.com".into(), "group/project".into()))
        );
    }

    #[test]
    fn parses_nested_group_path_without_dot_git() {
        assert_eq!(
            parse_git_url("https://host/group/subgroup/project"),
            Some(("host".into(), "group/subgroup/project".into()))
        );
    }

    #[test]
    fn parses_scp_style_ssh_url() {
        assert_eq!(
            parse_git_url("git@github.example.com:org/repo.git"),
            Some(("github.example.com".into(), "org/repo".into()))
        );
    }

    #[test]
    fn parses_ssh_scheme_url_with_port() {
        assert_eq!(
            parse_git_url("ssh://git@github.example.com:2222/org/repo.git"),
            Some(("github.example.com".into(), "org/repo".into()))
        );
    }

    #[test]
    fn rejects_hostless_input() {
        assert_eq!(parse_git_url("not-a-url"), None);
    }
}
