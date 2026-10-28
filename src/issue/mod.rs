mod comment;
mod create;
mod edit;
mod search;
mod transition;
mod view;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::path::Path;

use crate::adf;
use crate::cli::IssueCommand;
use crate::context::Ctx;
use crate::output::{date, is_date};
use crate::prompt;

pub fn run(ctx: &Ctx, cmd: IssueCommand) -> Result<()> {
    match cmd {
        IssueCommand::View(args) => view::run(ctx, args),
        IssueCommand::Search(args) => search::run(ctx, args),
        IssueCommand::Create(args) => create::run(ctx, args),
        IssueCommand::Edit(args) => edit::run(ctx, args),
        IssueCommand::Transition(args) => transition::run(ctx, args),
        IssueCommand::Comment(cmd) => comment::run(ctx, cmd),
    }
}

/// A description or comment body as ADF, from a file, the argument, or stdin.
pub fn body(text: Option<&str>, file: Option<&Path>, force_adf: bool, what: &str) -> Result<Value> {
    let raw = prompt::read_body_or_file(text, file, what)?;
    adf::body(&raw, force_adf)
}

/// A description from a `--from-json` spec: Markdown text or an ADF document.
pub fn spec_description(spec: &Map<String, Value>, force_adf: bool) -> Result<Option<Value>> {
    match spec.get("description") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => adf::body(text, force_adf).map(Some),
        Some(v) if adf::is_doc(v) => Ok(Some(v.clone())),
        Some(_) => bail!("description in --from-json must be Markdown text or an ADF document"),
    }
}

/// A JSON object from `path`, in acli's `--generate-json` layout.
pub fn load_spec(path: &Path) -> Result<Map<String, Value>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))?;
    match v {
        Value::Object(map) => Ok(map),
        _ => bail!("{} must hold a JSON object", path.display()),
    }
}

/// A string from a spec, trimmed and non-empty.
pub fn spec_string(spec: &Map<String, Value>, key: &str) -> Option<String> {
    spec.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Strings from a spec's array.
pub fn spec_strings(spec: &Map<String, Value>, key: &str) -> Vec<String> {
    spec.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A field value as a person reads it: names of people and options, dates in local time,
/// lists joined, and ADF as Markdown.
pub fn field_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => {
            if is_date(s) {
                date(s)
            } else {
                s.clone()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a
            .iter()
            .map(field_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(o) => {
            if adf::is_doc(v) {
                return adf::adf_to_markdown(v);
            }
            if let (Some(key), Some(summary)) = (o.get("key"), v["fields"]["summary"].as_str()) {
                return format!("{} {summary}", key.as_str().unwrap_or(""));
            }
            for name in ["displayName", "name", "value", "key"] {
                if let Some(s) = o.get(name).and_then(Value::as_str) {
                    return s.to_string();
                }
            }
            v.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn field_text_reads_common_shapes() {
        assert_eq!(field_text(&Value::Null), "");
        assert_eq!(field_text(&json!("plain")), "plain");
        assert_eq!(field_text(&json!(3)), "3");
        assert_eq!(
            field_text(&json!({"displayName": "Ann", "name": "ann"})),
            "Ann"
        );
        assert_eq!(field_text(&json!({"name": "Bug"})), "Bug");
        assert_eq!(
            field_text(&json!([{"name": "a"}, {"name": "b"}, "c"])),
            "a, b, c"
        );
        assert_eq!(
            field_text(&json!({"key": "P-1", "fields": {"summary": "Parent"}})),
            "P-1 Parent"
        );
        assert_eq!(
            field_text(&json!({"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "desc"}]}
            ]})),
            "desc"
        );
        assert_eq!(field_text(&json!({"odd": true})), r#"{"odd":true}"#);
    }

    #[test]
    fn specs_read_strings_arrays_and_descriptions() {
        let spec: Map<String, Value> = serde_json::from_value(json!({
            "summary": " s ", "labels": ["a", "b"], "description": "# md"
        }))
        .unwrap();
        assert_eq!(spec_string(&spec, "summary").as_deref(), Some("s"));
        assert_eq!(spec_string(&spec, "missing"), None);
        assert_eq!(spec_strings(&spec, "labels"), ["a", "b"]);
        let desc = spec_description(&spec, false).unwrap().unwrap();
        assert_eq!(desc["content"][0]["type"], "heading");
        assert!(spec_description(&spec, true).is_err());
        let bad: Map<String, Value> = serde_json::from_value(json!({"description": 5})).unwrap();
        assert!(spec_description(&bad, false).is_err());
        assert!(spec_description(&Map::new(), false).unwrap().is_none());
    }

    #[test]
    fn specs_load_from_json_objects_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.json");
        std::fs::write(&path, r#"{"summary": "x"}"#).unwrap();
        assert_eq!(load_spec(&path).unwrap()["summary"], "x");
        std::fs::write(&path, "[]").unwrap();
        assert!(load_spec(&path).is_err());
        assert!(load_spec(&dir.path().join("missing.json")).is_err());
    }
}
