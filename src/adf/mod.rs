//! Atlassian Document Format, the JSON Jira uses for rich text, and its conversion to and from
//! Markdown.

mod from_markdown;
mod to_markdown;

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub use from_markdown::markdown_to_adf;
pub use to_markdown::adf_to_markdown;

use crate::cli::{AdfCommand, FileArg};
use crate::output::print_json;

/// Whether a value is an ADF document.
pub fn is_doc(v: &Value) -> bool {
    v["type"] == "doc" && v["content"].is_array()
}

/// The ADF document in a JSON value: the value itself, or the `body` or `description` it holds,
/// as a comment or issue from the API does.
pub fn find_doc(v: &Value) -> Option<Value> {
    [
        v,
        &v["body"],
        &v["description"],
        &v["fields"]["description"],
    ]
    .into_iter()
    .find(|c| is_doc(c))
    .cloned()
}

/// The ADF for a body: the document `text` already is, or its Markdown converted. `force`
/// rejects anything that is not already ADF.
pub fn body(text: &str, force: bool) -> Result<Value> {
    let trimmed = text.trim();
    if trimmed.starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && let Some(doc) = find_doc(&v)
    {
        return Ok(doc);
    }
    if force {
        bail!("The body is not an ADF document");
    }
    Ok(markdown_to_adf(text))
}

/// A field that may hold ADF, plain text, or nothing, as Markdown.
pub fn field_markdown(v: &Value) -> String {
    if is_doc(v) {
        adf_to_markdown(v)
    } else {
        v.as_str().unwrap_or("").to_string()
    }
}

pub fn run(cmd: AdfCommand) -> Result<()> {
    match cmd {
        AdfCommand::FromMd(arg) => print_json(&markdown_to_adf(&read(arg)?)),
        AdfCommand::ToMd(arg) => {
            let v: Value = serde_json::from_str(&read(arg)?).context("The input is not JSON")?;
            let doc = find_doc(&v).context("The JSON holds no ADF document")?;
            println!("{}", adf_to_markdown(&doc));
            Ok(())
        }
    }
}

fn read(arg: FileArg) -> Result<String> {
    match arg.file {
        Some(path) if path.as_os_str() != "-" => std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display())),
        _ => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .context("Failed to read stdin")?;
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bodies_pass_adf_through_and_convert_markdown() {
        let doc = json!({"type": "doc", "version": 1, "content": []});
        assert_eq!(body(&doc.to_string(), false).unwrap(), doc);
        assert_eq!(body(&json!({"body": doc}).to_string(), true).unwrap(), doc);
        let md = body("hi", false).unwrap();
        assert_eq!(md["content"][0]["content"][0]["text"], "hi");
        assert!(body("hi", true).is_err());
        assert!(body("{not json", true).is_err());
    }

    #[test]
    fn docs_are_found_in_api_shapes() {
        let doc = json!({"type": "doc", "version": 1, "content": []});
        assert_eq!(
            find_doc(&json!({"fields": {"description": doc}})).unwrap(),
            doc
        );
        assert!(find_doc(&json!({"fields": {}})).is_none());
        assert_eq!(field_markdown(&json!("plain")), "plain");
        assert_eq!(field_markdown(&Value::Null), "");
    }
}
