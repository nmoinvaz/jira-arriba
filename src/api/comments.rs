use anyhow::Result;
use serde_json::{Value, json};

use super::Client;

/// Every comment on an issue, oldest first.
pub fn list(client: &Client, key: &str) -> Result<Vec<Value>> {
    let mut comments = Vec::new();
    let mut start = 0usize;
    loop {
        let start_text = start.to_string();
        let page = client.get_query(
            &format!("issue/{key}/comment"),
            &[("startAt", start_text.as_str()), ("maxResults", "100")],
        )?;
        let batch = page["comments"].as_array().cloned().unwrap_or_default();
        let count = batch.len();
        comments.extend(batch);
        start += count;
        let total = page["total"].as_u64().unwrap_or(0) as usize;
        if count == 0 || start >= total {
            break;
        }
    }
    Ok(comments)
}

/// Add a comment with an ADF `body`, returning the comment Jira created.
pub fn add(client: &Client, key: &str, body: &Value) -> Result<Value> {
    client.post(&format!("issue/{key}/comment"), &json!({ "body": body }))
}

/// Replace a comment's body with the ADF `body`.
pub fn edit(client: &Client, key: &str, id: &str, body: &Value) -> Result<Value> {
    client.put(
        &format!("issue/{key}/comment/{id}"),
        &json!({ "body": body }),
    )
}

pub fn delete(client: &Client, key: &str, id: &str) -> Result<()> {
    client.delete(&format!("issue/{key}/comment/{id}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn list_pages_through_all_comments() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/A-1/comment")
            .match_query(Matcher::UrlEncoded("startAt".into(), "0".into()))
            .with_body(r#"{"comments": [{"id": "1"}, {"id": "2"}], "total": 3}"#)
            .create();
        server
            .mock("GET", "/issue/A-1/comment")
            .match_query(Matcher::UrlEncoded("startAt".into(), "2".into()))
            .with_body(r#"{"comments": [{"id": "3"}], "total": 3}"#)
            .create();
        let client = Client::for_tests(&server.url());
        let all = list(&client, "A-1").unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[2]["id"], "3");
    }

    #[test]
    fn add_edit_and_delete_hit_the_comment_endpoints() {
        let mut server = Server::new();
        let body = json!({"type": "doc", "version": 1, "content": []});
        server
            .mock("POST", "/issue/A-1/comment")
            .match_body(Matcher::Json(json!({"body": body})))
            .with_body(r#"{"id": "7"}"#)
            .create();
        server
            .mock("PUT", "/issue/A-1/comment/7")
            .match_body(Matcher::Json(json!({"body": body})))
            .with_body(r#"{"id": "7"}"#)
            .create();
        let del = server
            .mock("DELETE", "/issue/A-1/comment/7")
            .with_status(204)
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(add(&client, "A-1", &body).unwrap()["id"], "7");
        assert_eq!(edit(&client, "A-1", "7", &body).unwrap()["id"], "7");
        delete(&client, "A-1", "7").unwrap();
        del.assert();
    }
}
