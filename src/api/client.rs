use anyhow::{Context, Result, bail};
use reqwest::Method;
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::header::{ACCEPT, LOCATION};
use reqwest::redirect::Policy;
use serde_json::Value;
use std::path::Path;

use crate::credentials::{Auth, Credentials};

/// Install the TLS crypto provider once, before any client is built. The ring provider builds
/// without the C toolchains aws-lc-rs needs.
pub fn init_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// REST client for one Jira login.
pub struct Client {
    http: Http,
    /// Stops at redirects, for attachment content whose signed URL rejects the token.
    direct: Http,
    auth: Auth,
    api: String,
}

impl Client {
    pub fn new(creds: &Credentials) -> Result<Self> {
        init_tls();
        let agent = concat!("jira-arriba/", env!("CARGO_PKG_VERSION"));
        let http = Http::builder()
            .user_agent(agent)
            .build()
            .context("Failed to create HTTP client")?;
        let direct = Http::builder()
            .user_agent(agent)
            .redirect(Policy::none())
            .build()
            .context("Failed to create HTTP client")?;
        Ok(Self {
            http,
            direct,
            auth: creds.auth.clone(),
            api: creds.api.trim_end_matches('/').to_string(),
        })
    }

    /// A client for the API served at `base`, such as a local mock server in tests.
    #[cfg(test)]
    pub fn for_tests(base: &str) -> Self {
        init_tls();
        // The test server is local, so skip any proxy the environment configures.
        Self {
            http: Http::builder().no_proxy().build().unwrap(),
            direct: Http::builder()
                .no_proxy()
                .redirect(Policy::none())
                .build()
                .unwrap(),
            auth: Auth::Bearer("test-token".to_string()),
            api: base.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("https://") || path.starts_with("http://") {
            path.to_string()
        } else {
            format!("{}/{}", self.api, path.trim_start_matches('/'))
        }
    }

    fn authed(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.auth {
            Auth::Bearer(token) => req.bearer_auth(token),
            Auth::Basic { email, token } => req.basic_auth(email, Some(token)),
        }
    }

    fn request(&self, http: &Http, method: Method, url: &str) -> RequestBuilder {
        self.authed(http.request(method, url))
            .header(ACCEPT, "application/json")
    }

    fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Response> {
        let url = self.url(path);
        let mut req = self.request(&self.http, method.clone(), &url);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .with_context(|| format!("{method} {path} failed"))?;
        check(resp, &method, path)
    }

    /// GET a path relative to the API root, or an absolute URL.
    pub fn get(&self, path: &str) -> Result<Value> {
        parse_json(self.send(Method::GET, path, &[], None)?)
    }

    /// GET with query parameters, encoded for the URL.
    pub fn get_query(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        parse_json(self.send(Method::GET, path, query, None)?)
    }

    /// GET that maps 404 to None.
    pub fn get_opt(&self, path: &str) -> Result<Option<Value>> {
        let url = self.url(path);
        let resp = self
            .request(&self.http, Method::GET, &url)
            .send()
            .with_context(|| format!("GET {path} failed"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        parse_json(check(resp, &Method::GET, path)?).map(Some)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        parse_json(self.send(Method::POST, path, &[], Some(body))?)
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value> {
        parse_json(self.send(Method::PUT, path, &[], Some(body))?)
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        parse_json(self.send(Method::DELETE, path, &[], None)?)
    }

    /// Where a path redirects to, without following it.
    pub fn redirect(&self, path: &str) -> Result<String> {
        let url = self.url(path);
        let resp = self
            .request(&self.direct, Method::GET, &url)
            .send()
            .with_context(|| format!("GET {path} failed"))?;
        if !resp.status().is_redirection() {
            check(resp, &Method::GET, path)?;
            bail!("GET {path} did not redirect");
        }
        resp.headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .with_context(|| format!("GET {path} redirected without a location"))
    }

    /// Stream a URL into `dest` without credentials, as for a signed media URL. Returns the
    /// bytes written.
    pub fn download_url(&self, url: &str, dest: &Path) -> Result<u64> {
        let resp = self
            .http
            .get(url)
            .send()
            .with_context(|| format!("Failed to download {}", dest.display()))?;
        let status = resp.status();
        if !status.is_success() {
            bail!(
                "Download of {} failed with HTTP {}",
                dest.display(),
                status.as_u16()
            );
        }
        let mut resp = resp;
        let mut file = std::fs::File::create(dest)
            .with_context(|| format!("Failed to create {}", dest.display()))?;
        std::io::copy(&mut resp, &mut file)
            .with_context(|| format!("Failed to write {}", dest.display()))
    }

    /// POST a file as multipart form data, as the attachment endpoint takes it.
    pub fn upload(&self, path: &str, file: &Path) -> Result<Value> {
        let form = reqwest::blocking::multipart::Form::new()
            .file("file", file)
            .with_context(|| format!("Failed to read {}", file.display()))?;
        let url = self.url(path);
        let resp = self
            .request(&self.http, Method::POST, &url)
            // Jira refuses multipart uploads without this header, as a CSRF guard.
            .header("X-Atlassian-Token", "no-check")
            .multipart(form)
            .send()
            .with_context(|| format!("POST {path} failed"))?;
        parse_json(check(resp, &Method::POST, path)?)
    }
}

/// Fail with Jira's own messages on a non-2xx response.
fn check(resp: Response, method: &Method, path: &str) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let text = resp.text().unwrap_or_default();
    let message = error_message(&text).unwrap_or_else(|| {
        if text.trim().is_empty() {
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string()
        } else {
            text.trim().to_string()
        }
    });
    bail!("Jira API {} on {method} {path}: {message}", status.as_u16())
}

/// The messages in a Jira error body: `errorMessages` and the per-field `errors`.
fn error_message(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    let mut parts: Vec<String> = v["errorMessages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(errors) = v["errors"].as_object() {
        parts.extend(errors.iter().map(|(k, m)| {
            format!(
                "{k}: {}",
                m.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| m.to_string())
            )
        }));
    }
    if parts.is_empty() {
        return v["message"].as_str().map(str::to_string);
    }
    Some(parts.join("; "))
}

/// Parse a JSON body, mapping an empty body (such as 204) to Null.
fn parse_json(resp: Response) -> Result<Value> {
    let text = resp.text().context("Failed to read response body")?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).context("Failed to parse JSON response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};
    use serde_json::json;

    #[test]
    fn get_sends_the_token_and_parses_json() {
        let mut server = Server::new();
        let mock = server
            .mock("GET", "/issue/A-1")
            .match_header("authorization", "Bearer test-token")
            .match_header("accept", "application/json")
            .with_body(r#"{"key": "A-1"}"#)
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(client.get("issue/A-1").unwrap()["key"], "A-1");
        mock.assert();
    }

    #[test]
    fn query_parameters_are_encoded() {
        let mut server = Server::new();
        server
            .mock("GET", "/search/jql")
            .match_query(Matcher::UrlEncoded("jql".into(), "a = \"b c\"".into()))
            .with_body(r#"{"issues": []}"#)
            .create();
        let client = Client::for_tests(&server.url());
        let v = client
            .get_query("search/jql", &[("jql", "a = \"b c\"")])
            .unwrap();
        assert!(v["issues"].as_array().unwrap().is_empty());
    }

    #[test]
    fn errors_carry_jira_messages() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/NOPE-1")
            .with_status(404)
            .with_body(r#"{"errorMessages": ["Issue does not exist"], "errors": {}}"#)
            .create();
        server
            .mock("PUT", "/issue/A-1")
            .with_status(400)
            .with_body(
                r#"{"errorMessages": [], "errors": {"summary": "You must specify a summary"}}"#,
            )
            .create();
        server.mock("DELETE", "/x").with_status(403).create();
        server
            .mock("POST", "/y")
            .with_status(500)
            .with_body("plain failure")
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(
            client.get("issue/NOPE-1").unwrap_err().to_string(),
            "Jira API 404 on GET issue/NOPE-1: Issue does not exist"
        );
        assert!(client.get_opt("issue/NOPE-1").unwrap().is_none());
        assert_eq!(
            client.put("issue/A-1", &json!({})).unwrap_err().to_string(),
            "Jira API 400 on PUT issue/A-1: summary: You must specify a summary"
        );
        assert_eq!(
            client.delete("x").unwrap_err().to_string(),
            "Jira API 403 on DELETE x: Forbidden"
        );
        assert_eq!(
            client.post("y", &json!({})).unwrap_err().to_string(),
            "Jira API 500 on POST y: plain failure"
        );
    }

    #[test]
    fn empty_bodies_read_as_null() {
        let mut server = Server::new();
        server
            .mock("DELETE", "/attachment/1")
            .with_status(204)
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(client.delete("attachment/1").unwrap(), Value::Null);
    }

    #[test]
    fn redirects_are_reported_not_followed() {
        let mut server = Server::new();
        server
            .mock("GET", "/attachment/content/1")
            .with_status(303)
            .with_header("location", "https://media.example/signed")
            .create();
        server
            .mock("GET", "/attachment/content/2")
            .with_status(200)
            .with_body("{}")
            .create();
        let client = Client::for_tests(&server.url());
        assert_eq!(
            client.redirect("attachment/content/1").unwrap(),
            "https://media.example/signed"
        );
        assert_eq!(
            client
                .redirect("attachment/content/2")
                .unwrap_err()
                .to_string(),
            "GET attachment/content/2 did not redirect"
        );
    }

    #[test]
    fn downloads_stream_without_credentials() {
        let mut server = Server::new();
        server
            .mock("GET", "/signed")
            .match_header("authorization", Matcher::Missing)
            .with_body("file contents")
            .create();
        server.mock("GET", "/gone").with_status(403).create();
        let client = Client::for_tests(&server.url());
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.txt");
        let n = client
            .download_url(&format!("{}/signed", server.url()), &dest)
            .unwrap();
        assert_eq!(n, 13);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "file contents");
        let err = client
            .download_url(&format!("{}/gone", server.url()), &dir.path().join("b"))
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 403"), "{err}");
    }

    #[test]
    fn uploads_send_multipart_with_the_csrf_header() {
        let mut server = Server::new();
        server
            .mock("POST", "/issue/A-1/attachments")
            .match_header("x-atlassian-token", "no-check")
            .match_header("content-type", Matcher::Regex("multipart/form-data".into()))
            .match_body(Matcher::Regex("name=\"file\"; filename=\"log.txt\"".into()))
            .with_body(r#"[{"id": "9", "filename": "log.txt"}]"#)
            .create();
        let client = Client::for_tests(&server.url());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        std::fs::write(&path, "hello").unwrap();
        let v = client.upload("issue/A-1/attachments", &path).unwrap();
        assert_eq!(v[0]["id"], "9");
    }
}
