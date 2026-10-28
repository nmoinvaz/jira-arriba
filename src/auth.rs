use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::json;

use crate::api::{Client, users};
use crate::cli::{AuthCommand, LoginArgs, TokenArgs};
use crate::credentials::{self, Credentials, acli};
use crate::output::{duration, print_json, table};
use crate::prompt;

pub fn run(site: Option<&str>, json: bool, yes: bool, cmd: AuthCommand) -> Result<()> {
    match cmd {
        AuthCommand::Status => status(site, json),
        AuthCommand::Login(args) => login(site, json, yes, args),
        AuthCommand::Token(args) => token(site, args),
    }
}

/// Report the login and who Jira says it belongs to, exiting 1 when there is none that works.
fn status(site: Option<&str>, json: bool) -> Result<()> {
    let creds = match credentials::resolve(site) {
        Ok(c) => c,
        Err(e) => return not_logged_in(json, &format!("{e:#}")),
    };
    let client = Client::new(&creds)?;
    let me = match users::myself(&client) {
        Ok(me) => me,
        Err(e) => {
            return not_logged_in(
                json,
                &format!(
                    "{} rejected the login from {}: {e:#}",
                    creds.site, creds.source
                ),
            );
        }
    };
    let name = me["displayName"].as_str().unwrap_or("?");
    let email = me["emailAddress"]
        .as_str()
        .or(creds.email.as_deref())
        .unwrap_or("email hidden");
    let account = me["accountId"].as_str().unwrap_or("?");
    if json {
        return print_json(&json!({
            "loggedIn": true,
            "site": creds.site,
            "cloudId": creds.cloud_id,
            "accountId": account,
            "displayName": name,
            "email": email,
            "source": creds.source.to_string(),
            "expiry": creds.expiry.map(|e| e.to_rfc3339()),
        }));
    }
    println!("✓ Logged in to {} as {name} ({email})", creds.site);
    println!("{}", table(&rows(&creds, account), "  "));
    Ok(())
}

fn rows(creds: &Credentials, account: &str) -> Vec<Vec<String>> {
    let mut rows = vec![
        vec!["Source".to_string(), creds.source.to_string()],
        vec!["Account".to_string(), account.to_string()],
    ];
    if let Some(id) = &creds.cloud_id {
        rows.push(vec!["Cloud ID".to_string(), id.clone()]);
    }
    if let Some(expiry) = creds.expiry {
        let left = (expiry - Utc::now()).num_seconds();
        let text = if left > 0 {
            format!("in {}", duration(left))
        } else {
            "expired".to_string()
        };
        rows.push(vec!["Expires".to_string(), text]);
    }
    rows
}

fn not_logged_in(json: bool, reason: &str) -> Result<()> {
    if json {
        print_json(&json!({ "loggedIn": false, "error": reason }))?;
    } else {
        eprintln!("✗ Not logged in: {reason}");
    }
    std::process::exit(1)
}

/// Log in through acli, which keeps the login this tool reads, offering to install it first.
fn login(site: Option<&str>, json: bool, yes: bool, args: LoginArgs) -> Result<()> {
    if credentials::env_present() {
        bail!(
            "The login comes from the environment ({} or {}). Unset it to log in through acli.",
            credentials::API_TOKEN,
            credentials::ACCESS_TOKEN
        );
    }
    if !acli::installed() {
        let brew = acli::brew_available();
        let question =
            "acli is not installed, and it holds the browser login. Install it with Homebrew?";
        if !brew || !prompt::confirm(question, yes)? {
            bail!(
                "acli is not installed, and it holds the browser login. {}, or set {}, {}, and {}.",
                acli::install_hint(brew),
                credentials::API_TOKEN,
                credentials::EMAIL,
                credentials::SITE
            );
        }
        acli::install()?;
    }
    let mut acli_args: Vec<&str> = Vec::new();
    if args.token {
        let site = site.context("--token needs --site")?;
        let email = args.email.as_deref().context("--token needs --email")?;
        acli_args.extend(["--site", site, "--email", email, "--token"]);
    } else {
        // The browser is the default, so --web only spells it out.
        let _ = args.web;
        acli_args.push("--web");
    }
    acli::login(&acli_args)?;
    status(site, json)
}

fn token(site: Option<&str>, args: TokenArgs) -> Result<()> {
    let creds = credentials::resolve(site)?;
    if args.header {
        println!("Authorization: {}", creds.auth.header());
    } else {
        println!("{}", creds.auth.secret());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{Auth, Source};

    #[test]
    fn status_rows_show_source_and_expiry() {
        let creds = Credentials {
            auth: Auth::Bearer("t".into()),
            api: "https://api".into(),
            site: "x.atlassian.net".into(),
            cloud_id: Some("cloud".into()),
            email: None,
            expiry: Some(Utc::now() + chrono::Duration::minutes(5)),
            source: Source::Acli("oauth_global".into()),
        };
        let table_rows = rows(&creds, "acct");
        assert_eq!(table_rows[0], vec!["Source", "acli login (oauth_global)"]);
        assert_eq!(table_rows[2], vec!["Cloud ID", "cloud"]);
        assert!(
            table_rows[3][1].starts_with("in 4m"),
            "{}",
            table_rows[3][1]
        );

        let expired = Credentials {
            expiry: Some(Utc::now() - chrono::Duration::minutes(5)),
            cloud_id: None,
            ..creds
        };
        let table_rows = rows(&expired, "acct");
        assert_eq!(table_rows[2], vec!["Expires", "expired"]);
    }
}
