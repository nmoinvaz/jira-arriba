use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

use super::Client;

/// Fields `issue view` shows without `--fields`.
pub const DEFAULT_FIELDS: &[&str] = &[
    "summary",
    "issuetype",
    "status",
    "priority",
    "resolution",
    "assignee",
    "reporter",
    "created",
    "updated",
    "labels",
    "components",
    "fixVersions",
    "parent",
    "description",
    "comment",
    "attachment",
];

/// Fields `issue search` shows without `--fields`.
pub const SEARCH_FIELDS: &[&str] = &["issuetype", "status", "assignee", "summary"];

static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9_]*-\d+$").unwrap());
static URL_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:/browse/|selectedIssue=|/issues/)([A-Za-z][A-Za-z0-9_]*-\d+)").unwrap()
});

/// An issue key from a key or a Jira URL, in upper case.
pub fn parse_key(arg: &str) -> Result<String> {
    let arg = arg.trim();
    if KEY.is_match(arg) {
        return Ok(arg.to_uppercase());
    }
    if let Some(c) = URL_KEY.captures(arg) {
        return Ok(c[1].to_uppercase());
    }
    bail!("{arg} is not an issue key such as PROJ-123, or a link to one")
}

/// An issue with `fields` (a comma list, `*all`, or `*navigable`) and `expand`.
pub fn fetch(client: &Client, key: &str, fields: &str, expand: &str) -> Result<Value> {
    let mut query = vec![("fields", fields)];
    if !expand.is_empty() {
        query.push(("expand", expand));
    }
    client.get_query(&format!("issue/{key}"), &query)
}

/// Issues matching `jql` with `fields`, up to `limit`, or all of them.
pub fn search(
    client: &Client,
    jql: &str,
    fields: &[String],
    limit: Option<usize>,
) -> Result<Vec<Value>> {
    let mut issues = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let remaining = limit.map(|l| l.saturating_sub(issues.len()));
        if remaining == Some(0) {
            break;
        }
        let mut body = json!({
            "jql": jql,
            "fields": fields,
            "maxResults": remaining.unwrap_or(100).min(100),
        });
        if let Some(t) = &token {
            body["nextPageToken"] = json!(t);
        }
        let page = client.post("search/jql", &body)?;
        let batch = page["issues"].as_array().cloned().unwrap_or_default();
        let empty = batch.is_empty();
        issues.extend(batch);
        token = page["nextPageToken"].as_str().map(str::to_string);
        if page["isLast"].as_bool().unwrap_or(true) || token.is_none() || empty {
            break;
        }
    }
    Ok(issues)
}

/// Create an issue from its `fields`, returning Jira's `{id, key, self}`.
pub fn create(client: &Client, fields: Map<String, Value>) -> Result<Value> {
    client.post("issue", &json!({ "fields": fields }))
}

/// Set `fields` and apply `update` operations on an issue.
pub fn edit(
    client: &Client,
    key: &str,
    fields: Map<String, Value>,
    update: Map<String, Value>,
) -> Result<()> {
    if fields.is_empty() && update.is_empty() {
        bail!("Nothing to change");
    }
    let mut body = Map::new();
    if !fields.is_empty() {
        body.insert("fields".into(), Value::Object(fields));
    }
    if !update.is_empty() {
        body.insert("update".into(), Value::Object(update));
    }
    client.put(&format!("issue/{key}"), &Value::Object(body))?;
    Ok(())
}

/// A status change the issue can make.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Transition {
    pub id: String,
    pub name: String,
    /// Status the transition leads to.
    pub to: String,
}

pub fn transitions(client: &Client, key: &str) -> Result<Vec<Transition>> {
    let v = client.get(&format!("issue/{key}/transitions"))?;
    Ok(v["transitions"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| Transition {
                    id: t["id"].as_str().unwrap_or_default().to_string(),
                    name: t["name"].as_str().unwrap_or_default().to_string(),
                    to: t["to"]["name"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The transition named `status`, by its own name or its target status, ignoring case.
pub fn find_transition<'a>(transitions: &'a [Transition], status: &str) -> Result<&'a Transition> {
    let wanted = status.trim();
    transitions
        .iter()
        .find(|t| t.to.eq_ignore_ascii_case(wanted) || t.name.eq_ignore_ascii_case(wanted))
        .with_context(|| {
            format!(
                "No transition to {wanted}. Available: {}",
                transitions
                    .iter()
                    .map(|t| format!("{} (to {})", t.name, t.to))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

pub fn transition(client: &Client, key: &str, id: &str) -> Result<()> {
    client.post(
        &format!("issue/{key}/transitions"),
        &json!({ "transition": { "id": id } }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn keys_come_from_keys_and_links() {
        assert_eq!(parse_key("proj-123").unwrap(), "PROJ-123");
        assert_eq!(parse_key(" D4T-1 ").unwrap(), "D4T-1");
        assert_eq!(
            parse_key("https://x.atlassian.net/browse/ABC-9?focusedCommentId=1").unwrap(),
            "ABC-9"
        );
        assert_eq!(
            parse_key("https://x.atlassian.net/jira/software/c/projects/A/issues/A-2").unwrap(),
            "A-2"
        );
        assert_eq!(
            parse_key("https://x.atlassian.net/jira/software/boards/1?selectedIssue=B2-3").unwrap(),
            "B2-3"
        );
        assert!(parse_key("123").is_err());
        assert!(parse_key("https://x.atlassian.net/").is_err());
    }

    #[test]
    fn search_follows_page_tokens_up_to_the_limit() {
        let mut server = Server::new();
        server
            .mock("POST", "/search/jql")
            .match_body(Matcher::PartialJson(json!({"jql": "project = A", "maxResults": 3})))
            .with_body(r#"{"issues": [{"key": "A-1"}, {"key": "A-2"}], "isLast": false, "nextPageToken": "t2"}"#)
            .create();
        server
            .mock("POST", "/search/jql")
            .match_body(Matcher::PartialJson(
                json!({"nextPageToken": "t2", "maxResults": 1}),
            ))
            .with_body(r#"{"issues": [{"key": "A-3"}], "isLast": false, "nextPageToken": "t3"}"#)
            .create();
        let client = Client::for_tests(&server.url());
        let found = search(&client, "project = A", &["summary".into()], Some(3)).unwrap();
        let keys: Vec<&str> = found.iter().map(|i| i["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["A-1", "A-2", "A-3"]);
    }

    #[test]
    fn search_stops_at_the_last_page() {
        let mut server = Server::new();
        server
            .mock("POST", "/search/jql")
            .match_body(Matcher::PartialJson(json!({"maxResults": 100})))
            .with_body(r#"{"issues": [{"key": "A-1"}], "isLast": true}"#)
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(search(&client, "project = A", &[], None).unwrap().len(), 1);
    }

    #[test]
    fn transitions_match_by_name_or_target() {
        let ts = vec![
            Transition {
                id: "1".into(),
                name: "Close Issue".into(),
                to: "Closed".into(),
            },
            Transition {
                id: "2".into(),
                name: "Start".into(),
                to: "In Progress".into(),
            },
        ];
        assert_eq!(find_transition(&ts, "closed").unwrap().id, "1");
        assert_eq!(find_transition(&ts, "start").unwrap().id, "2");
        assert_eq!(find_transition(&ts, "in progress").unwrap().id, "2");
        assert_eq!(
            find_transition(&ts, "Done").unwrap_err().to_string(),
            "No transition to Done. Available: Close Issue (to Closed), Start (to In Progress)"
        );
    }

    #[test]
    fn transitions_are_listed_and_posted() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/A-1/transitions")
            .with_body(r#"{"transitions": [{"id": "31", "name": "Done", "to": {"name": "Done"}}]}"#)
            .create();
        let post = server
            .mock("POST", "/issue/A-1/transitions")
            .match_body(Matcher::Json(json!({"transition": {"id": "31"}})))
            .with_status(204)
            .create();
        let client = Client::for_tests(&server.url());
        let ts = transitions(&client, "A-1").unwrap();
        assert_eq!(ts[0].to, "Done");
        transition(&client, "A-1", "31").unwrap();
        post.assert();
    }

    #[test]
    fn edit_sends_fields_and_update_and_rejects_nothing() {
        let mut server = Server::new();
        let put = server
            .mock("PUT", "/issue/A-1")
            .match_body(Matcher::Json(json!({
                "fields": {"summary": "s"},
                "update": {"labels": [{"add": "x"}]}
            })))
            .with_status(204)
            .create();
        let client = Client::for_tests(&server.url());
        let mut fields = Map::new();
        fields.insert("summary".into(), json!("s"));
        let mut update = Map::new();
        update.insert("labels".into(), json!([{"add": "x"}]));
        edit(&client, "A-1", fields, update).unwrap();
        put.assert();
        assert_eq!(
            edit(&client, "A-1", Map::new(), Map::new())
                .unwrap_err()
                .to_string(),
            "Nothing to change"
        );
    }
}
