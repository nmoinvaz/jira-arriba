//! The login acli keeps: its profiles in `~/.config/acli` and the token in the platform keyring.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Auth, Credentials, Source, gateway_api, keyring, normalize_site};

/// Keyring service go-keyring files acli's secrets under.
const SERVICE: &str = "acli";

/// Tokens this close to expiring are refreshed first, so a long download does not outlive one.
const EXPIRY_MARGIN_SECS: i64 = 60;

#[derive(Debug, Default, Deserialize)]
struct Config {
    #[serde(default)]
    current_profile: Option<String>,
    #[serde(default)]
    profiles: Vec<Profile>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub cloud_id: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub auth_type: String,
}

impl Profile {
    /// How acli names the profile in `current_profile`.
    fn id(&self) -> String {
        format!("{}:{}", self.cloud_id, self.account_id)
    }
}

/// The OAuth token acli stores, gzipped, in the keyring.
#[derive(Debug, Deserialize)]
struct Token {
    access_token: String,
    #[serde(default)]
    expiry: Option<String>,
}

impl Token {
    fn expiry(&self) -> Option<DateTime<Utc>> {
        self.expiry
            .as_deref()
            .and_then(|e| DateTime::parse_from_rfc3339(e).ok())
            .map(|e| e.with_timezone(&Utc))
    }

    fn expires_within(&self, secs: i64) -> bool {
        self.expiry()
            .is_some_and(|e| e <= Utc::now() + chrono::Duration::seconds(secs))
    }
}

/// Where acli keeps its config on every platform.
pub fn config_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(".config").join("acli"))
}

/// Credentials for acli's current Jira login, or the one for `site`.
pub fn credentials(site: Option<&str>) -> Result<Credentials> {
    let dir = config_dir().context("Could not find the home directory")?;
    let config = load(&dir)?;
    if config.profiles.is_empty() {
        bail!(
            "Not logged in. Run `jira-arriba auth login`, or set JIRA_API_TOKEN, JIRA_EMAIL, and JIRA_SITE."
        );
    }
    let profile = select(&config.profiles, config.current_profile.as_deref(), site)?;
    match profile.auth_type.as_str() {
        "oauth" | "oauth_global" => oauth(profile),
        "api_token" => api_token(profile),
        other => bail!("acli login type {other} is not supported"),
    }
}

/// acli's Jira profiles, from its Jira config or else the shared one.
fn load(dir: &Path) -> Result<Config> {
    for name in ["jira_config.yaml", "global_auth_config.yaml"] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&text)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        if !config.profiles.is_empty() {
            return Ok(config);
        }
    }
    Ok(Config::default())
}

/// The profile for `site`, or the current one, or the first.
fn select<'a>(
    profiles: &'a [Profile],
    current: Option<&str>,
    site: Option<&str>,
) -> Result<&'a Profile> {
    if let Some(site) = site {
        let wanted = normalize_site(site);
        return profiles
            .iter()
            .find(|p| normalize_site(&p.site) == wanted)
            .with_context(|| {
                format!(
                    "acli has no login for {wanted}, only: {}",
                    profiles
                        .iter()
                        .map(|p| p.site.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
    }
    Ok(current
        .and_then(|id| profiles.iter().find(|p| p.id() == id))
        .unwrap_or(&profiles[0]))
}

fn oauth(profile: &Profile) -> Result<Credentials> {
    let account = format!("oauth:{}:{}", profile.cloud_id, profile.account_id);
    let mut token = read_token(&account)?;
    if token.expires_within(EXPIRY_MARGIN_SECS) {
        refresh()?;
        token = read_token(&account)?;
        if token.expires_within(0) {
            bail!(
                "acli's token for {} has expired. Run `jira-arriba auth login`.",
                profile.site
            );
        }
    }
    Ok(Credentials {
        auth: Auth::Bearer(token.access_token.clone()),
        api: gateway_api(&profile.cloud_id),
        site: normalize_site(&profile.site),
        cloud_id: Some(profile.cloud_id.clone()),
        email: Some(profile.email.clone()).filter(|e| !e.is_empty()),
        expiry: token.expiry(),
        source: Source::Acli(profile.auth_type.clone()),
    })
}

fn read_token(account: &str) -> Result<Token> {
    let raw = keyring::read(SERVICE, account)?
        .context("acli has no token in the keyring. Run `jira-arriba auth login`.")?;
    let bytes = keyring::decode(&raw)?;
    serde_json::from_slice(&bytes).context("acli's stored token is not in the expected layout")
}

/// acli's API token login. The keyring entry name is not documented, so the likely ones are tried.
fn api_token(profile: &Profile) -> Result<Credentials> {
    let suffixes = [
        format!("{}:{}", profile.cloud_id, profile.account_id),
        format!("{}:{}", profile.site, profile.email),
    ];
    for prefix in ["api_token", "apitoken", "token", "basic"] {
        for suffix in &suffixes {
            if let Some(raw) = keyring::read(SERVICE, &format!("{prefix}:{suffix}"))? {
                let bytes = keyring::decode(&raw)?;
                let token = api_token_text(&bytes)?;
                return Ok(Credentials {
                    auth: Auth::Basic {
                        email: profile.email.clone(),
                        token,
                    },
                    api: format!("https://{}/rest/api/3", normalize_site(&profile.site)),
                    site: normalize_site(&profile.site),
                    cloud_id: Some(profile.cloud_id.clone()).filter(|c| !c.is_empty()),
                    email: Some(profile.email.clone()),
                    expiry: None,
                    source: Source::Acli(profile.auth_type.clone()),
                });
            }
        }
    }
    bail!(
        "acli's API token login for {} could not be read from the keyring. Set JIRA_API_TOKEN, JIRA_EMAIL, and JIRA_SITE instead.",
        profile.site
    )
}

/// The token from a stored API token entry, which may be the bare token or a JSON wrapper.
fn api_token_text(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes)
        .context("acli's stored API token is not text")?
        .trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        for key in ["api_token", "token", "access_token"] {
            if let Some(t) = v[key].as_str() {
                return Ok(t.to_string());
            }
        }
    }
    Ok(text.to_string())
}

/// Have acli refresh its token, which `auth status` does once the token has expired.
fn refresh() -> Result<()> {
    let status = Command::new("acli")
        .args(["jira", "auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => bail!("acli could not refresh its token. Run `jira-arriba auth login`."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "acli's token has expired and acli is not installed to refresh it. Install acli, or set JIRA_API_TOKEN, JIRA_EMAIL, and JIRA_SITE."
        ),
        Err(e) => Err(e).context("Failed to run acli"),
    }
}

/// Whether acli is on the PATH.
pub fn installed() -> bool {
    Command::new("acli")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Run `acli jira auth login` with `args`, letting it use the terminal.
pub fn login(args: &[&str]) -> Result<()> {
    let status = Command::new("acli")
        .args(["jira", "auth", "login"])
        .args(args)
        .status()
        .context("Failed to run acli")?;
    if !status.success() {
        bail!("acli jira auth login failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(site: &str, cloud: &str, account: &str) -> Profile {
        Profile {
            site: site.to_string(),
            cloud_id: cloud.to_string(),
            account_id: account.to_string(),
            auth_type: "oauth_global".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn config_parses_acli_layout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("jira_config.yaml"),
            "version: 1\ncurrent_profile: c1:a1\nprofiles:\n    - site: one.jira.com\n      cloud_id: c1\n      account_id: a1\n      display_name: One\n      email: one@x.com\n      auth_type: oauth_global\n",
        )
        .unwrap();
        let config = load(dir.path()).unwrap();
        assert_eq!(config.current_profile.as_deref(), Some("c1:a1"));
        assert_eq!(config.profiles[0].email, "one@x.com");
        assert_eq!(config.profiles[0].id(), "c1:a1");
        assert!(
            load(&dir.path().join("missing"))
                .unwrap()
                .profiles
                .is_empty()
        );
    }

    #[test]
    fn global_config_is_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("jira_config.yaml"), "version: 1\n").unwrap();
        std::fs::write(
            dir.path().join("global_auth_config.yaml"),
            "profiles:\n    - site: g.jira.com\n      cloud_id: c\n      account_id: a\n      auth_type: oauth\n",
        )
        .unwrap();
        assert_eq!(load(dir.path()).unwrap().profiles[0].site, "g.jira.com");
    }

    #[test]
    fn selection_prefers_site_then_current() {
        let profiles = vec![
            profile("one.jira.com", "c1", "a1"),
            profile("two.atlassian.net", "c2", "a2"),
        ];
        assert_eq!(
            select(&profiles, Some("c2:a2"), None).unwrap().site,
            "two.atlassian.net"
        );
        assert_eq!(
            select(&profiles, Some("gone"), None).unwrap().site,
            "one.jira.com"
        );
        assert_eq!(select(&profiles, None, None).unwrap().site, "one.jira.com");
        assert_eq!(
            select(&profiles, Some("c1:a1"), Some("https://Two.atlassian.net"))
                .unwrap()
                .site,
            "two.atlassian.net"
        );
        let err = select(&profiles, None, Some("three.jira.com")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "acli has no login for three.jira.com, only: one.jira.com, two.atlassian.net"
        );
    }

    #[test]
    fn token_expiry_is_read_with_margin() {
        let soon = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        let token = Token {
            access_token: "x".into(),
            expiry: Some(soon),
        };
        assert!(token.expires_within(60));
        assert!(!token.expires_within(0));
        let none = Token {
            access_token: "x".into(),
            expiry: None,
        };
        assert!(!none.expires_within(60));
        let bad = Token {
            access_token: "x".into(),
            expiry: Some("yesterday".into()),
        };
        assert!(bad.expiry().is_none());
    }

    #[test]
    fn api_tokens_may_be_bare_or_wrapped() {
        assert_eq!(api_token_text(b" abc \n").unwrap(), "abc");
        assert_eq!(api_token_text(br#"{"api_token": "t"}"#).unwrap(), "t");
        assert_eq!(
            api_token_text(br#"{"other": 1}"#).unwrap(),
            r#"{"other": 1}"#
        );
    }
}
