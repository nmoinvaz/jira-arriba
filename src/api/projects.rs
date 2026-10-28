use anyhow::Result;
use serde_json::Value;

use super::Client;

/// Projects the login can see, filtered by `query` against key and name, up to `limit`.
pub fn list(client: &Client, query: Option<&str>, limit: Option<usize>) -> Result<Vec<Value>> {
    let mut projects = Vec::new();
    let mut start = 0usize;
    loop {
        let remaining = limit.map(|l| l.saturating_sub(projects.len()));
        if remaining == Some(0) {
            break;
        }
        let page_size = remaining.unwrap_or(50).min(50).to_string();
        let start_text = start.to_string();
        let mut query_params = vec![
            ("startAt", start_text.as_str()),
            ("maxResults", page_size.as_str()),
            ("orderBy", "key"),
        ];
        if let Some(q) = query {
            query_params.push(("query", q));
        }
        let page = client.get_query("project/search", &query_params)?;
        let values = page["values"].as_array().cloned().unwrap_or_default();
        let count = values.len();
        projects.extend(values);
        if page["isLast"].as_bool().unwrap_or(true) || count == 0 {
            break;
        }
        start += count;
    }
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn pages_until_the_last_or_the_limit() {
        let mut server = Server::new();
        server
            .mock("GET", "/project/search")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("startAt".into(), "0".into()),
                Matcher::UrlEncoded("query".into(), "d".into()),
            ]))
            .with_body(r#"{"values": [{"key": "D1"}, {"key": "D2"}], "isLast": false}"#)
            .create();
        server
            .mock("GET", "/project/search")
            .match_query(Matcher::UrlEncoded("startAt".into(), "2".into()))
            .with_body(r#"{"values": [{"key": "D3"}], "isLast": true}"#)
            .create();
        let client = Client::for_tests(&server.url());
        let all = list(&client, Some("d"), None).unwrap();
        assert_eq!(all.len(), 3);
        let some = list(&client, Some("d"), Some(2)).unwrap();
        assert_eq!(some.len(), 2);
    }
}
