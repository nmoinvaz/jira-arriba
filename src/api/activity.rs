//! Recent activity on an issue: its newest changelog entries and comments, for the watch.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::Client;
use crate::output::parse_date;

/// Changelog entries read per issue, enough to cover a poll's worth of activity.
const PAGE: usize = 50;

/// Comments read per issue, newest first.
const COMMENTS: &str = "20";

/// Who did something.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Actor {
    pub id: String,
    pub name: String,
}

fn actor(v: &Value) -> Actor {
    Actor {
        id: v["accountId"].as_str().unwrap_or("").to_string(),
        name: v["displayName"].as_str().unwrap_or("someone").to_string(),
    }
}

fn created(v: &Value) -> Option<DateTime<Utc>> {
    v["created"]
        .as_str()
        .and_then(parse_date)
        .map(|d| d.with_timezone(&Utc))
}

/// One changelog entry: the field changes one person made at once.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub id: String,
    pub at: DateTime<Utc>,
    pub by: Actor,
    pub items: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub id: String,
    pub at: DateTime<Utc>,
    pub by: Actor,
    pub body: Value,
}

/// Changelog entries after `since`, oldest first. Jira pages the changelog oldest first, so the
/// last page is read after asking how long it is.
pub fn changelog_since(client: &Client, key: &str, since: DateTime<Utc>) -> Result<Vec<History>> {
    let path = format!("issue/{key}/changelog");
    let probe = client.get_query(&path, &[("maxResults", "1")])?;
    let total = probe["total"].as_u64().unwrap_or(0) as usize;
    if total == 0 {
        return Ok(Vec::new());
    }
    let start = total.saturating_sub(PAGE).to_string();
    let size = PAGE.to_string();
    let page = client.get_query(&path, &[("startAt", &start), ("maxResults", &size)])?;
    Ok(page["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let at = created(h)?;
            (at > since).then(|| History {
                id: h["id"].as_str().unwrap_or("").to_string(),
                at,
                by: actor(&h["author"]),
                items: h["items"].as_array().cloned().unwrap_or_default(),
            })
        })
        .collect())
}

/// Comments after `since`, oldest first, from the newest page.
pub fn comments_since(client: &Client, key: &str, since: DateTime<Utc>) -> Result<Vec<Comment>> {
    let page = client.get_query(
        &format!("issue/{key}/comment"),
        &[("orderBy", "-created"), ("maxResults", COMMENTS)],
    )?;
    let mut comments: Vec<Comment> = page["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let at = created(c)?;
            (at > since).then(|| Comment {
                id: c["id"].as_str().unwrap_or("").to_string(),
                at,
                by: actor(&c["author"]),
                body: c["body"].clone(),
            })
        })
        .collect();
    comments.reverse();
    Ok(comments)
}

/// Whether an ADF body mentions the account.
pub fn mentions(body: &Value, account_id: &str) -> bool {
    if body["type"] == "mention" && body["attrs"]["id"] == account_id {
        return true;
    }
    match body {
        Value::Object(o) => o.values().any(|v| mentions(v, account_id)),
        Value::Array(a) => a.iter().any(|v| mentions(v, account_id)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};
    use serde_json::json;

    fn stamp(minutes_ago: i64) -> String {
        (Utc::now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339()
    }

    #[test]
    fn changelog_reads_the_last_page_and_keeps_recent_entries() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/A-1/changelog")
            .match_query(Matcher::UrlEncoded("maxResults".into(), "1".into()))
            .with_body(r#"{"total": 120, "values": []}"#)
            .create();
        let page = server
            .mock("GET", "/issue/A-1/changelog")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("startAt".into(), "70".into()),
                Matcher::UrlEncoded("maxResults".into(), "50".into()),
            ]))
            .with_body(
                json!({"values": [
                    {"id": "1", "created": stamp(30), "author": {"accountId": "a", "displayName": "Ann"}, "items": []},
                    {"id": "2", "created": stamp(1), "author": {"accountId": "b", "displayName": "Bob"},
                     "items": [{"field": "status", "fromString": "Open", "toString": "Closed"}]}
                ]})
                .to_string(),
            )
            .create();
        server
            .mock("GET", "/issue/A-2/changelog")
            .match_query(Matcher::Any)
            .with_body(r#"{"total": 0, "values": []}"#)
            .create();
        let client = Client::for_tests(&server.url());
        let since = Utc::now() - chrono::Duration::minutes(10);
        let recent = changelog_since(&client, "A-1", since).unwrap();
        page.assert();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, "2");
        assert_eq!(recent[0].by.name, "Bob");
        assert_eq!(recent[0].items[0]["toString"], "Closed");
        assert!(changelog_since(&client, "A-2", since).unwrap().is_empty());
    }

    #[test]
    fn comments_come_back_oldest_first_after_since() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/A-1/comment")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("orderBy".into(), "-created".into()),
                Matcher::UrlEncoded("maxResults".into(), "20".into()),
            ]))
            .with_body(
                json!({"comments": [
                    {"id": "3", "created": stamp(1), "author": {"accountId": "a", "displayName": "Ann"}, "body": {}},
                    {"id": "2", "created": stamp(5), "author": {"accountId": "b"}, "body": {}},
                    {"id": "1", "created": stamp(60), "author": {"accountId": "a"}, "body": {}}
                ]})
                .to_string(),
            )
            .create();
        let client = Client::for_tests(&server.url());
        let since = Utc::now() - chrono::Duration::minutes(10);
        let recent = comments_since(&client, "A-1", since).unwrap();
        let ids: Vec<&str> = recent.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["2", "3"]);
        assert_eq!(recent[0].by.name, "someone");
    }

    #[test]
    fn mentions_are_found_anywhere_in_the_body() {
        let body = json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [
                {"type": "text", "text": "hey "},
                {"type": "mention", "attrs": {"id": "me", "text": "@Me"}}
            ]}
        ]});
        assert!(mentions(&body, "me"));
        assert!(!mentions(&body, "you"));
        assert!(!mentions(&json!("plain"), "me"));
    }
}
