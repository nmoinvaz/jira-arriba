use anyhow::Result;
use serde_json::Value;

use super::field_text;
use crate::adf;
use crate::api::issues::{self, DEFAULT_FIELDS, parse_key};
use crate::cli::ViewArgs;
use crate::context::Ctx;
use crate::output::{date, print_json, size, table};

/// Fields shown in the header, in order, with their labels.
const HEADER: &[(&str, &str)] = &[
    ("Type", "issuetype"),
    ("Status", "status"),
    ("Priority", "priority"),
    ("Resolution", "resolution"),
    ("Assignee", "assignee"),
    ("Reporter", "reporter"),
    ("Created", "created"),
    ("Updated", "updated"),
    ("Labels", "labels"),
    ("Components", "components"),
    ("Fix versions", "fixVersions"),
    ("Parent", "parent"),
];

/// Fields rendered as their own sections rather than header rows.
const SECTIONS: &[&str] = &["summary", "description", "comment", "attachment"];

pub fn run(ctx: &Ctx, args: ViewArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    let fields = if args.all {
        "*all".to_string()
    } else {
        args.fields
            .clone()
            .unwrap_or_else(|| DEFAULT_FIELDS.join(","))
    };
    let issue = issues::fetch(&ctx.client, &key, &fields, "")?;
    if ctx.json {
        return print_json(&issue);
    }
    println!("{}", render(&issue, &ctx.browse_url(&key)));
    Ok(())
}

/// The issue as text: a header of its fields, then its description, comments, and attachments.
pub fn render(issue: &Value, url: &str) -> String {
    let key = issue["key"].as_str().unwrap_or("?");
    let f = &issue["fields"];
    let mut out = String::new();
    let summary = f["summary"].as_str().unwrap_or("");
    out.push_str(&format!("{key}  {summary}\n"));

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (label, name) in HEADER {
        let text = field_text(&f[*name]);
        if !text.is_empty() {
            rows.push(vec![label.to_string(), text]);
        }
    }
    // Fields asked for beyond the usual ones, such as custom fields, in the order Jira sent them.
    let known: Vec<&str> = HEADER
        .iter()
        .map(|(_, n)| *n)
        .chain(SECTIONS.iter().copied())
        .collect();
    for (name, value) in f.as_object().into_iter().flatten() {
        if known.contains(&name.as_str()) {
            continue;
        }
        let text = field_text(value);
        if !text.is_empty() {
            rows.push(vec![name.clone(), first_line(&text)]);
        }
    }
    rows.push(vec!["URL".to_string(), url.to_string()]);
    out.push_str(&table(&rows, ""));
    out.push('\n');

    if let Some(description) = f.get("description").filter(|d| !d.is_null()) {
        let text = adf::field_markdown(description);
        if !text.trim().is_empty() {
            out.push_str(&format!("\nDescription\n\n{}\n", text.trim_end()));
        }
    }

    if let Some(comments) = f["comment"]["comments"].as_array() {
        out.push_str(&format!("\nComments ({})\n", comments.len()));
        for c in comments {
            out.push_str(&format!("\n{}\n", comment_header(c)));
            let text = adf::field_markdown(&c["body"]);
            if !text.trim().is_empty() {
                out.push_str(&format!("{}\n", text.trim_end()));
            }
        }
    }

    if let Some(attachments) = f["attachment"].as_array().filter(|a| !a.is_empty()) {
        out.push_str(&format!("\nAttachments ({})\n", attachments.len()));
        let rows: Vec<Vec<String>> = attachments
            .iter()
            .map(|a| {
                vec![
                    a["id"].as_str().unwrap_or("").to_string(),
                    a["size"].as_u64().map(size).unwrap_or_default(),
                    a["filename"].as_str().unwrap_or("").to_string(),
                ]
            })
            .collect();
        out.push_str(&table(&rows, "  "));
        out.push('\n');
    }
    out
}

/// `#id  author  date`, noting an edit.
pub fn comment_header(c: &Value) -> String {
    let id = c["id"].as_str().unwrap_or("?");
    let author = c["author"]["displayName"].as_str().unwrap_or("?");
    let created = c["created"].as_str().map(date).unwrap_or_default();
    let edited = match (c["created"].as_str(), c["updated"].as_str()) {
        (Some(a), Some(b)) if a != b => format!("  (edited {})", date(b)),
        _ => String::new(),
    };
    format!("#{id}  {author}  {created}{edited}")
}

/// The first line of a value, noting how many more there are.
fn first_line(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").to_string();
    let more = lines.count();
    if more > 0 {
        format!("{first} [+{more} lines]")
    } else {
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(text: &str) -> Value {
        json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}
        ]})
    }

    #[test]
    fn renders_header_description_comments_and_attachments() {
        let issue = json!({"key": "A-1", "fields": {
            "summary": "Crash on start",
            "issuetype": {"name": "Bug"},
            "status": {"name": "Open"},
            "priority": null,
            "assignee": {"displayName": "Ann"},
            "labels": ["x", "y"],
            "parent": {"key": "A-0", "fields": {"summary": "Epic"}},
            "customfield_1": {"value": "Opt"},
            "customfield_2": null,
            "description": doc("It crashes."),
            "comment": {"comments": [
                {"id": "5", "author": {"displayName": "Bob"}, "created": "2026-10-26T14:25:34.396+0000",
                 "updated": "2026-10-26T15:00:00.000+0000", "body": doc("Me too.")}
            ]},
            "attachment": [{"id": "9", "size": 2048, "filename": "log.txt"}]
        }});
        let text = render(&issue, "https://x/browse/A-1");
        // Column padding depends on the widest label, so compare with runs of spaces collapsed.
        let norm: String = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            norm.starts_with("A-1 Crash on start\nType Bug\nStatus Open\n"),
            "{norm}"
        );
        assert!(norm.contains("\nAssignee Ann\n"));
        assert!(norm.contains("\nLabels x, y\n"));
        assert!(norm.contains("\nParent A-0 Epic\n"));
        assert!(norm.contains("\ncustomfield_1 Opt\n"), "{norm}");
        assert!(!norm.contains("customfield_2"));
        assert!(!norm.contains("Priority"));
        assert!(norm.contains("\nURL https://x/browse/A-1\n"));
        assert!(norm.contains("\nDescription\n\nIt crashes.\n"));
        assert!(
            norm.contains("\nComments (1)\n\n#5 Bob 2026-10-26 "),
            "{norm}"
        );
        assert!(norm.contains("(edited 2026-10-26 "));
        assert!(norm.contains("Me too.\n"));
        assert!(
            norm.ends_with("\nAttachments (1)\n9 2.0 KB log.txt"),
            "{norm}"
        );
    }

    #[test]
    fn first_lines_note_the_rest() {
        assert_eq!(first_line("one"), "one");
        assert_eq!(first_line("one\ntwo\nthree"), "one [+2 lines]");
    }
}
