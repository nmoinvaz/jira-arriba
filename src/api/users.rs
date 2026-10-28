use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::Client;

/// The account the login belongs to.
pub fn myself(client: &Client) -> Result<Value> {
    client.get("myself")
}

/// The account ID for `who`: `@me`, an account ID, an email, or a display name.
pub fn account_id(client: &Client, who: &str) -> Result<String> {
    let who = who.trim();
    if who == "@me" {
        return myself(client)?["accountId"]
            .as_str()
            .map(str::to_string)
            .context("Could not read your account ID");
    }
    if looks_like_account_id(who) {
        return Ok(who.to_string());
    }
    let found = client.get_query("user/search", &[("query", who)])?;
    let users: Vec<&Value> = found
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    match users.as_slice() {
        [one] => one["accountId"]
            .as_str()
            .map(str::to_string)
            .context("User has no account ID"),
        [] => bail!("No user matches {who}"),
        many => {
            let names: Vec<String> = many
                .iter()
                .map(|u| {
                    format!(
                        "{} <{}>",
                        u["displayName"].as_str().unwrap_or("?"),
                        u["emailAddress"].as_str().unwrap_or("hidden")
                    )
                })
                .collect();
            bail!("Several users match {who}: {}", names.join(", "))
        }
    }
}

/// Account IDs look like `557058:uuid`, `712020:uuid`, or 24 hex digits.
fn looks_like_account_id(s: &str) -> bool {
    if s.contains('@') || s.contains(' ') {
        return false;
    }
    s.contains(':') || (s.len() == 24 && s.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn account_ids_resolve_from_me_ids_and_searches() {
        let mut server = Server::new();
        server
            .mock("GET", "/myself")
            .with_body(r#"{"accountId": "me-id"}"#)
            .create();
        server
            .mock("GET", "/user/search")
            .match_query(Matcher::UrlEncoded("query".into(), "one@x.com".into()))
            .with_body(r#"[{"accountId": "one-id"}]"#)
            .create();
        server
            .mock("GET", "/user/search")
            .match_query(Matcher::UrlEncoded("query".into(), "nobody".into()))
            .with_body("[]")
            .create();
        server
            .mock("GET", "/user/search")
            .match_query(Matcher::UrlEncoded("query".into(), "Sam".into()))
            .with_body(r#"[{"accountId": "a", "displayName": "Sam A", "emailAddress": "a@x"}, {"accountId": "b", "displayName": "Sam B"}]"#)
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(account_id(&client, "@me").unwrap(), "me-id");
        assert_eq!(account_id(&client, "557058:abc").unwrap(), "557058:abc");
        assert_eq!(account_id(&client, "one@x.com").unwrap(), "one-id");
        assert_eq!(
            account_id(&client, "nobody").unwrap_err().to_string(),
            "No user matches nobody"
        );
        assert_eq!(
            account_id(&client, "Sam").unwrap_err().to_string(),
            "Several users match Sam: Sam A <a@x>, Sam B <hidden>"
        );
    }

    #[test]
    fn account_id_shapes() {
        assert!(looks_like_account_id(
            "557058:950a000f-1e2b-429e-82a3-be2b67428976"
        ));
        assert!(looks_like_account_id("5b10ac8d82e05b22cc7d4ef5"));
        assert!(!looks_like_account_id("me@x.com"));
        assert!(!looks_like_account_id("Nathan M"));
    }
}
