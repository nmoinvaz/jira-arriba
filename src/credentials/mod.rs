//! Where the login comes from: environment variables first, then the login acli keeps.

pub mod acli;
pub mod keyring;

use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const API_TOKEN: &str = "JIRA_API_TOKEN";
pub const EMAIL: &str = "JIRA_EMAIL";
pub const SITE: &str = "JIRA_SITE";
pub const ACCESS_TOKEN: &str = "JIRA_ACCESS_TOKEN";
pub const CLOUD_ID: &str = "JIRA_CLOUD_ID";

/// The gateway OAuth tokens must go through, since they are not accepted by the site itself.
pub const GATEWAY: &str = "https://api.atlassian.com";

/// How requests prove who they are.
#[derive(Clone, Debug, PartialEq)]
pub enum Auth {
    /// An OAuth access token.
    Bearer(String),
    /// An API token with the email it was created for.
    Basic { email: String, token: String },
}

impl Auth {
    /// The Authorization header value.
    pub fn header(&self) -> String {
        match self {
            Auth::Bearer(token) => format!("Bearer {token}"),
            Auth::Basic { email, token } => format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"))
            ),
        }
    }

    /// The secret itself, for scripts that build their own requests.
    pub fn secret(&self) -> &str {
        match self {
            Auth::Bearer(token) => token,
            Auth::Basic { token, .. } => token,
        }
    }
}

/// Where the login was found.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The environment variables that supplied it.
    Env(Vec<&'static str>),
    /// acli's login, by its auth type.
    Acli(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Env(vars) => write!(f, "environment ({})", vars.join(", ")),
            Source::Acli(kind) => write!(f, "acli login ({kind})"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Credentials {
    pub auth: Auth,
    /// Root of the REST API, ending in `/rest/api/3`.
    pub api: String,
    /// Host of the site, such as `mysite.atlassian.net`, for web links.
    pub site: String,
    pub cloud_id: Option<String>,
    pub email: Option<String>,
    /// When an OAuth token stops working.
    pub expiry: Option<DateTime<Utc>>,
    pub source: Source,
}

impl Credentials {
    /// Web link to an issue on the site.
    pub fn browse_url(&self, key: &str) -> String {
        format!("https://{}/browse/{key}", self.site)
    }
}

/// The login to use: from the environment when set, otherwise acli's. `site` picks among acli's
/// logins and names the site for an OAuth token given without a cloud ID.
pub fn resolve(site: Option<&str>) -> Result<Credentials> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    if let Some(creds) = from_env(&env, site, lookup_cloud_id)? {
        return Ok(creds);
    }
    acli::credentials(site)
}

/// Whether the environment supplies a login, so acli's is not consulted.
pub fn env_present() -> bool {
    [API_TOKEN, ACCESS_TOKEN]
        .iter()
        .any(|v| std::env::var(v).is_ok_and(|t| !t.trim().is_empty()))
}

/// Credentials from the environment, or None when it sets no token. A token missing its
/// companions is an error, so a half-set environment is not silently ignored.
pub fn from_env(
    get: &dyn Fn(&str) -> Option<String>,
    site: Option<&str>,
    cloud_lookup: impl Fn(&str, Option<&str>) -> Result<(String, String)>,
) -> Result<Option<Credentials>> {
    let site = site.map(str::to_string).or_else(|| get(SITE));
    if let Some(token) = get(API_TOKEN) {
        let email = get(EMAIL).with_context(|| format!("{API_TOKEN} is set but {EMAIL} is not"))?;
        let site =
            normalize_site(&site.with_context(|| format!("{API_TOKEN} is set but {SITE} is not"))?);
        return Ok(Some(Credentials {
            auth: Auth::Basic {
                email: email.clone(),
                token,
            },
            api: format!("https://{site}/rest/api/3"),
            site,
            cloud_id: None,
            email: Some(email),
            expiry: None,
            source: Source::Env(vec![API_TOKEN, EMAIL, SITE]),
        }));
    }
    if let Some(token) = get(ACCESS_TOKEN) {
        let site = site.as_deref().map(normalize_site);
        let (cloud_id, site, vars) = match get(CLOUD_ID) {
            Some(id) => (id, site.unwrap_or_default(), vec![ACCESS_TOKEN, CLOUD_ID]),
            None => {
                let (id, site) = cloud_lookup(&token, site.as_deref())
                    .with_context(|| format!("{ACCESS_TOKEN} is set but {CLOUD_ID} is not"))?;
                (id, site, vec![ACCESS_TOKEN, SITE])
            }
        };
        return Ok(Some(Credentials {
            auth: Auth::Bearer(token),
            api: gateway_api(&cloud_id),
            site,
            cloud_id: Some(cloud_id),
            email: None,
            expiry: None,
            source: Source::Env(vars),
        }));
    }
    Ok(None)
}

/// The REST root for a site reached through the OAuth gateway.
pub fn gateway_api(cloud_id: &str) -> String {
    format!("{GATEWAY}/ex/jira/{cloud_id}/rest/api/3")
}

/// A site as a bare host: no scheme, path, or trailing slash.
pub fn normalize_site(site: &str) -> String {
    let s = site.trim();
    let s = s
        .strip_prefix("https://")
        .or(s.strip_prefix("http://"))
        .unwrap_or(s);
    s.split('/').next().unwrap_or("").trim().to_lowercase()
}

/// Ask the gateway which sites `token` can reach, returning the cloud ID and host of `site`, or of
/// the only site when none is named.
fn lookup_cloud_id(token: &str, site: Option<&str>) -> Result<(String, String)> {
    crate::api::init_tls();
    let resources: Value = reqwest::blocking::Client::new()
        .get(format!("{GATEWAY}/oauth/token/accessible-resources"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .context("Failed to reach the Atlassian gateway")?
        .error_for_status()
        .context("The gateway rejected the token")?
        .json()
        .context("Failed to parse the gateway's response")?;
    pick_resource(&resources, site)
}

fn pick_resource(resources: &Value, site: Option<&str>) -> Result<(String, String)> {
    let sites: Vec<(String, String)> = resources
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let id = r["id"].as_str()?;
                    let host = normalize_site(r["url"].as_str()?);
                    Some((id.to_string(), host))
                })
                .collect()
        })
        .unwrap_or_default();
    match site {
        Some(wanted) => sites
            .into_iter()
            .find(|(_, host)| host == wanted)
            .with_context(|| format!("The token cannot reach {wanted}")),
        None => match sites.as_slice() {
            [one] => Ok(one.clone()),
            [] => bail!("The token reaches no Jira site"),
            many => bail!(
                "The token reaches several sites, so set {SITE} or {CLOUD_ID}: {}",
                many.iter()
                    .map(|(_, h)| h.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn no_lookup(_: &str, _: Option<&str>) -> Result<(String, String)> {
        bail!("lookup should not run")
    }

    #[test]
    fn api_token_uses_the_site_directly() {
        let vars = env(&[
            (API_TOKEN, "tok"),
            (EMAIL, "me@x.com"),
            (SITE, "https://my.atlassian.net/"),
        ]);
        let creds = from_env(&|k| vars.get(k).cloned(), None, no_lookup)
            .unwrap()
            .unwrap();
        assert_eq!(creds.api, "https://my.atlassian.net/rest/api/3");
        assert_eq!(creds.site, "my.atlassian.net");
        assert_eq!(
            creds.browse_url("A-1"),
            "https://my.atlassian.net/browse/A-1"
        );
        assert_eq!(creds.auth.header(), "Basic bWVAeC5jb206dG9r");
        assert_eq!(creds.auth.secret(), "tok");
        assert_eq!(
            creds.source.to_string(),
            "environment (JIRA_API_TOKEN, JIRA_EMAIL, JIRA_SITE)"
        );
    }

    #[test]
    fn site_flag_stands_in_for_the_variable() {
        let vars = env(&[(API_TOKEN, "tok"), (EMAIL, "me@x.com")]);
        let creds = from_env(
            &|k| vars.get(k).cloned(),
            Some("Other.Atlassian.net"),
            no_lookup,
        )
        .unwrap()
        .unwrap();
        assert_eq!(creds.site, "other.atlassian.net");
    }

    #[test]
    fn half_set_environment_is_an_error() {
        let vars = env(&[(API_TOKEN, "tok")]);
        let err = from_env(&|k| vars.get(k).cloned(), None, no_lookup).unwrap_err();
        assert_eq!(
            err.to_string(),
            "JIRA_API_TOKEN is set but JIRA_EMAIL is not"
        );
        let vars = env(&[(API_TOKEN, "tok"), (EMAIL, "me@x.com")]);
        let err = from_env(&|k| vars.get(k).cloned(), None, no_lookup).unwrap_err();
        assert_eq!(
            err.to_string(),
            "JIRA_API_TOKEN is set but JIRA_SITE is not"
        );
    }

    #[test]
    fn access_token_goes_through_the_gateway() {
        let vars = env(&[(ACCESS_TOKEN, "tok"), (CLOUD_ID, "abc")]);
        let creds = from_env(&|k| vars.get(k).cloned(), None, no_lookup)
            .unwrap()
            .unwrap();
        assert_eq!(
            creds.api,
            "https://api.atlassian.com/ex/jira/abc/rest/api/3"
        );
        assert_eq!(creds.auth, Auth::Bearer("tok".into()));
        assert_eq!(creds.auth.header(), "Bearer tok");

        let vars = env(&[(ACCESS_TOKEN, "tok"), (SITE, "my.atlassian.net")]);
        let creds = from_env(&|k| vars.get(k).cloned(), None, |_, site| {
            Ok(("found".to_string(), site.unwrap().to_string()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(creds.cloud_id.as_deref(), Some("found"));
        assert_eq!(creds.site, "my.atlassian.net");
    }

    #[test]
    fn empty_environment_defers_to_acli() {
        let vars = env(&[(API_TOKEN, "  ")]);
        let get = |k: &str| vars.get(k).cloned().filter(|v| !v.trim().is_empty());
        assert!(from_env(&get, None, no_lookup).unwrap().is_none());
    }

    #[test]
    fn resources_pick_the_named_or_only_site() {
        let one = json!([{"id": "a", "url": "https://one.atlassian.net"}]);
        assert_eq!(
            pick_resource(&one, None).unwrap(),
            ("a".to_string(), "one.atlassian.net".to_string())
        );
        let two = json!([
            {"id": "a", "url": "https://one.atlassian.net"},
            {"id": "b", "url": "https://two.atlassian.net"}
        ]);
        assert_eq!(
            pick_resource(&two, Some("two.atlassian.net")).unwrap().0,
            "b"
        );
        let err = pick_resource(&two, None).unwrap_err().to_string();
        assert!(
            err.contains("one.atlassian.net, two.atlassian.net"),
            "{err}"
        );
        assert!(pick_resource(&two, Some("none.atlassian.net")).is_err());
        assert!(pick_resource(&json!([]), None).is_err());
    }

    #[test]
    fn sites_normalize_to_hosts() {
        assert_eq!(
            normalize_site("https://My.Site.com/browse/X"),
            "my.site.com"
        );
        assert_eq!(normalize_site(" site.jira.com "), "site.jira.com");
    }
}
