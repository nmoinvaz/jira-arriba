//! Attachments and inline media of an issue, found in every place Jira records them.

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use super::Client;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub size: Option<u64>,
    pub mime_type: Option<String>,
    pub author: Option<String>,
    pub created: Option<String>,
}

impl Attachment {
    /// From the metadata Jira returns for an attachment, with `filename` as a fallback name.
    fn from_meta(id: &str, meta: &Value, filename: &str) -> Self {
        Self {
            id: id.to_string(),
            filename: meta["filename"]
                .as_str()
                .filter(|f| !f.is_empty())
                .unwrap_or(filename)
                .to_string(),
            size: meta["size"].as_u64(),
            mime_type: meta["mimeType"].as_str().map(str::to_string),
            author: meta["author"]["displayName"].as_str().map(str::to_string),
            created: meta["created"].as_str().map(str::to_string),
        }
    }
}

/// Inline media in rendered HTML: an image with its alt text, a file link with its name, or a
/// bare content link.
static INLINE_ALT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"attachment/content/(\d+)"[^>]*?alt="([^"]+)""#).unwrap());
static INLINE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"attachment/content/(\d+)"[^>]*?data-attachment-name="([^"]+)""#).unwrap()
});
static INLINE_ANY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"attachment/content/(\d+)").unwrap());

/// Every attachment of `key`, from its attachment field, its changelog, and the inline media in
/// its description and comments.
pub fn discover(client: &Client, key: &str) -> Result<Vec<Attachment>> {
    let issue = client.get_query(
        &format!("issue/{key}"),
        &[
            ("expand", "renderedFields,changelog"),
            ("fields", "attachment,description,comment"),
        ],
    )?;
    collect(&issue, |id| metadata(client, id))
}

/// The attachments an issue JSON names, confirming each one only the changelog or inline HTML
/// mentions through `lookup`, which returns its metadata or None once deleted.
///
/// Sources, in order of trust:
/// 1. `fields.attachment`, which may be null when the project hides the Attachment field even
///    though attachments exist.
/// 2. The changelog's Attachment items, each an add (`to` set) or a remove (`from` set).
/// 3. Inline images and file links in the rendered description and comments.
pub fn collect(
    issue: &Value,
    mut lookup: impl FnMut(&str) -> Result<Option<Value>>,
) -> Result<Vec<Attachment>> {
    let mut found: BTreeMap<u64, Attachment> = BTreeMap::new();
    let num = |id: &str| id.parse::<u64>().ok();

    for a in issue["fields"]["attachment"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(filename)) = (a["id"].as_str(), a["filename"].as_str()) else {
            continue;
        };
        if let Some(n) = num(id) {
            found.insert(n, Attachment::from_meta(id, a, filename));
        }
    }

    // Walking the history in order leaves only the adds not later removed.
    let mut live: BTreeMap<u64, String> = BTreeMap::new();
    for history in issue["changelog"]["histories"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for item in history["items"].as_array().into_iter().flatten() {
            if item["field"].as_str() != Some("Attachment") {
                continue;
            }
            if let (Some(id), Some(name)) = (item["to"].as_str(), item["toString"].as_str())
                && let Some(n) = num(id)
            {
                live.insert(n, name.to_string());
            }
            if let Some(n) = item["from"].as_str().and_then(num) {
                live.remove(&n);
            }
        }
    }

    let rendered = &issue["renderedFields"];
    let mut html = rendered["description"].as_str().unwrap_or("").to_string();
    for c in rendered["comment"]["comments"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(body) = c["body"].as_str() {
            html.push('\n');
            html.push_str(body);
        }
    }
    for m in INLINE_ALT
        .captures_iter(&html)
        .chain(INLINE_NAME.captures_iter(&html))
    {
        if let Some(n) = num(&m[1]) {
            live.entry(n).or_insert_with(|| m[2].to_string());
        }
    }
    for m in INLINE_ANY.captures_iter(&html) {
        if let Some(n) = num(&m[1]) {
            live.entry(n)
                .or_insert_with(|| format!("attachment-{}", &m[1]));
        }
    }

    // Each extra mention is confirmed, since deleted files linger in the changelog and HTML.
    for (n, name) in live {
        if found.contains_key(&n) {
            continue;
        }
        let id = n.to_string();
        if let Some(meta) = lookup(&id)? {
            found.insert(n, Attachment::from_meta(&id, &meta, &name));
        }
    }

    Ok(found.into_values().collect())
}

/// An attachment's metadata, or None when it no longer exists.
pub fn metadata(client: &Client, id: &str) -> Result<Option<Value>> {
    client.get_opt(&format!("attachment/{id}"))
}

/// Download an attachment to `dest`, returning the bytes written. The content endpoint redirects
/// to a signed media URL that rejects the Authorization header, so the redirect is resolved first
/// and then fetched without credentials.
pub fn download(client: &Client, id: &str, dest: &Path) -> Result<u64> {
    let location = client
        .redirect(&format!("attachment/content/{id}"))
        .with_context(|| format!("Attachment {id} has no download link"))?;
    client.download_url(&location, dest)
}

/// Attach a file to an issue, returning the attachments Jira created.
pub fn upload(client: &Client, key: &str, file: &Path) -> Result<Vec<Attachment>> {
    let v = client.upload(&format!("issue/{key}/attachments"), file)?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .map(|m| Attachment::from_meta(m["id"].as_str().unwrap_or(""), m, ""))
                .collect()
        })
        .unwrap_or_default())
}

pub fn delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("attachment/{id}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};
    use serde_json::json;

    fn meta(id: &str, name: &str, size: u64) -> Value {
        json!({"id": id, "filename": name, "size": size, "mimeType": "text/plain",
               "author": {"displayName": "Ann"}, "created": "2026-10-01T00:00:00.000+0000"})
    }

    #[test]
    fn field_changelog_and_html_sources_merge() {
        let issue = json!({
            "fields": {"attachment": [meta("10", "a.txt", 5)]},
            "changelog": {"histories": [
                {"items": [{"field": "Attachment", "from": null, "to": "11", "toString": "b.log"}]},
                {"items": [{"field": "Attachment", "from": null, "to": "12", "toString": "gone.zip"}]},
                {"items": [{"field": "Attachment", "from": "12", "fromString": "gone.zip", "to": null}]},
                {"items": [{"field": "Attachment", "from": null, "to": "13", "toString": "deleted-quietly.png"}]},
                {"items": [{"field": "status", "from": "1", "to": "2"}]}
            ]},
            "renderedFields": {
                "description": "<img src=\"/rest/api/3/attachment/content/14\" alt=\"shot.png\">",
                "comment": {"comments": [
                    {"body": "<a href=\"/rest/api/3/attachment/content/15\" data-attachment-name=\"trace.txt\">x</a>"},
                    {"body": "<a href=\"/rest/api/3/attachment/content/16\">bare</a>"}
                ]}
            }
        });
        let mut asked = Vec::new();
        let found = collect(&issue, |id| {
            asked.push(id.to_string());
            Ok(match id {
                "11" => Some(meta("11", "b.log", 7)),
                "13" => None,
                "14" => Some(json!({"id": "14", "size": 9})),
                "15" => Some(meta("15", "trace.txt", 3)),
                "16" => Some(meta("16", "bare.bin", 1)),
                _ => panic!("unexpected lookup {id}"),
            })
        })
        .unwrap();
        let names: Vec<(&str, &str)> = found
            .iter()
            .map(|a| (a.id.as_str(), a.filename.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("10", "a.txt"),
                ("11", "b.log"),
                ("14", "shot.png"),
                ("15", "trace.txt"),
                ("16", "bare.bin")
            ]
        );
        assert_eq!(found[0].size, Some(5));
        assert_eq!(found[0].author.as_deref(), Some("Ann"));
        assert_eq!(found[2].size, Some(9));
        assert!(!asked.contains(&"10".to_string()));
        assert!(!asked.contains(&"12".to_string()));
    }

    #[test]
    fn hidden_attachment_field_still_finds_changelog_files() {
        let issue = json!({
            "fields": {"attachment": null},
            "changelog": {"histories": [
                {"items": [{"field": "Attachment", "to": "20", "toString": "only.txt"}]}
            ]}
        });
        let found = collect(&issue, |id| Ok(Some(meta(id, "only.txt", 2)))).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].filename, "only.txt");
        assert!(collect(&json!({}), |_| Ok(None)).unwrap().is_empty());
    }

    #[test]
    fn discover_download_upload_and_delete_use_the_endpoints() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/A-1")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("expand".into(), "renderedFields,changelog".into()),
                Matcher::UrlEncoded("fields".into(), "attachment,description,comment".into()),
            ]))
            .with_body(json!({"fields": {"attachment": [meta("1", "a.txt", 1)]}}).to_string())
            .create();
        server
            .mock("GET", "/attachment/content/1")
            .with_status(303)
            .with_header("location", &format!("{}/signed/1", server.url()))
            .create();
        server.mock("GET", "/signed/1").with_body("abc").create();
        server
            .mock("POST", "/issue/A-1/attachments")
            .with_body(json!([meta("2", "up.txt", 3)]).to_string())
            .create();
        let del = server
            .mock("DELETE", "/attachment/2")
            .with_status(204)
            .create();
        let client = Client::for_tests(&server.url());
        let found = discover(&client, "A-1").unwrap();
        assert_eq!(found[0].filename, "a.txt");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.txt");
        assert_eq!(download(&client, "1", &dest).unwrap(), 3);
        let up = dir.path().join("up.txt");
        std::fs::write(&up, "xyz").unwrap();
        let uploaded = upload(&client, "A-1", &up).unwrap();
        assert_eq!(uploaded[0].id, "2");
        delete(&client, "2").unwrap();
        del.assert();
    }
}
