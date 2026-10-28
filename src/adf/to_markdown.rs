//! ADF to Markdown, for reading descriptions and comments in a terminal and editing them.

use serde_json::Value;

/// Markdown for an ADF document, or for any ADF node's content.
pub fn adf_to_markdown(doc: &Value) -> String {
    render_blocks(children(doc))
}

fn children(node: &Value) -> &[Value] {
    node["content"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn attr<'a>(node: &'a Value, name: &str) -> &'a Value {
    &node["attrs"][name]
}

/// Blocks separated by blank lines. Empty paragraphs, which Jira adds freely around media, add
/// nothing and are dropped.
fn render_blocks(nodes: &[Value]) -> String {
    nodes
        .iter()
        .map(render_block)
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_block(node: &Value) -> String {
    let kind = node["type"].as_str().unwrap_or("");
    match kind {
        "paragraph" => render_inline(children(node)),
        "heading" => {
            let level = attr(node, "level").as_u64().unwrap_or(1).clamp(1, 6) as usize;
            format!("{} {}", "#".repeat(level), render_inline(children(node)))
        }
        "codeBlock" => {
            let code = text_of(children(node));
            let fence = if code.contains("```") { "````" } else { "```" };
            let language = attr(node, "language").as_str().unwrap_or("");
            format!("{fence}{language}\n{code}\n{fence}")
        }
        "blockquote" => quote(&render_blocks(children(node)), &[]),
        "panel" => {
            let label = match attr(node, "panelType").as_str().unwrap_or("info") {
                "note" => "IMPORTANT",
                "success" => "TIP",
                "warning" => "WARNING",
                "error" => "CAUTION",
                _ => "NOTE",
            };
            quote(&render_blocks(children(node)), &[format!("[!{label}]")])
        }
        "expand" | "nestedExpand" => {
            let title = attr(node, "title").as_str().unwrap_or("Details");
            quote(
                &render_blocks(children(node)),
                &[format!("**{title}**"), String::new()],
            )
        }
        "bulletList" => list(children(node), None),
        "orderedList" => list(
            children(node),
            Some(attr(node, "order").as_u64().unwrap_or(1)),
        ),
        "taskList" => children(node)
            .iter()
            .map(|item| {
                let done = attr(item, "state") == "DONE";
                format!(
                    "- [{}] {}",
                    if done { "x" } else { " " },
                    render_inline(children(item))
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "decisionList" => children(node)
            .iter()
            .map(|item| format!("- (decision) {}", render_inline(children(item))))
            .collect::<Vec<_>>()
            .join("\n"),
        "rule" => "---".to_string(),
        "table" => table(children(node)),
        "mediaSingle" | "mediaGroup" => children(node)
            .iter()
            .map(media)
            .collect::<Vec<_>>()
            .join("\n"),
        "media" | "mediaInline" => media(node),
        "blockCard" | "embedCard" => format!("<{}>", attr(node, "url").as_str().unwrap_or("")),
        "hardBreak" => String::new(),
        _ => {
            let inner = children(node);
            if inner.is_empty() {
                render_inline(std::slice::from_ref(node))
            } else if inner.iter().any(|c| c["type"] == "text") {
                render_inline(inner)
            } else {
                render_blocks(inner)
            }
        }
    }
}

/// Lines prefixed as a quote, after any `header` lines, with blank lines kept as bare markers.
fn quote(body: &str, header: &[String]) -> String {
    header
        .iter()
        .map(String::as_str)
        .chain(body.lines())
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn list(items: &[Value], start: Option<u64>) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let marker = match start {
                Some(n) => format!("{}. ", n + i as u64),
                None => "- ".to_string(),
            };
            let body = render_item(children(item));
            prefix_lines(&body, &marker, &" ".repeat(marker.len()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A list item's blocks, with a nested list tight against the paragraph before it.
fn render_item(blocks: &[Value]) -> String {
    let mut out = String::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            let nested_list = matches!(
                block["type"].as_str(),
                Some("bulletList" | "orderedList" | "taskList")
            );
            let after_paragraph = blocks[i - 1]["type"] == "paragraph";
            out.push_str(if nested_list && after_paragraph {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(&render_block(block));
    }
    out
}

/// `first` before the first line and `rest` before the others, leaving blank lines blank.
fn prefix_lines(text: &str, first: &str, rest: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(i, line)| match (i, line.is_empty()) {
            (0, _) => format!("{first}{line}"),
            (_, true) => String::new(),
            (_, false) => format!("{rest}{line}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table(rows: &[Value]) -> String {
    let rows: Vec<(bool, Vec<String>)> = rows
        .iter()
        .filter(|r| r["type"] == "tableRow")
        .map(|r| {
            let cells = children(r);
            let header = !cells.is_empty() && cells.iter().all(|c| c["type"] == "tableHeader");
            (header, cells.iter().map(cell).collect())
        })
        .collect();
    let Some(cols) = rows.iter().map(|(_, c)| c.len()).max().filter(|c| *c > 0) else {
        return String::new();
    };
    let (head, body) = match rows.first() {
        Some((true, cells)) => (cells.clone(), &rows[1..]),
        _ => (vec![String::new(); cols], &rows[..]),
    };
    let line = |cells: &[String]| {
        let mut padded = cells.to_vec();
        padded.resize(cols, String::new());
        format!("| {} |", padded.join(" | "))
    };
    let mut lines = vec![line(&head), format!("|{}", " --- |".repeat(cols))];
    lines.extend(body.iter().map(|(_, cells)| line(cells)));
    lines.join("\n")
}

/// A cell's blocks on one line, with the pipe escaped.
fn cell(cell: &Value) -> String {
    render_blocks(children(cell))
        .replace('\n', " ")
        .replace('|', "\\|")
        .trim()
        .to_string()
}

/// An image for a media node: its URL when external, its attachment ID otherwise.
fn media(node: &Value) -> String {
    let alt = attr(node, "alt").as_str().unwrap_or("");
    if attr(node, "type") == "external" {
        return format!("![{alt}]({})", attr(node, "url").as_str().unwrap_or(""));
    }
    let id = attr(node, "id").as_str().unwrap_or("");
    format!("![{}](media://{id})", if alt.is_empty() { id } else { alt })
}

/// The plain text of inline nodes, for code blocks.
fn text_of(nodes: &[Value]) -> String {
    nodes
        .iter()
        .map(|n| match n["type"].as_str() {
            Some("hardBreak") => "\n".to_string(),
            _ => n["text"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

fn render_inline(nodes: &[Value]) -> String {
    merge_text(nodes)
        .iter()
        .map(|node| match node["type"].as_str().unwrap_or("") {
            "text" => styled(node),
            "hardBreak" => "  \n".to_string(),
            "mention" => attr(node, "text")
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("@{}", attr(node, "id").as_str().unwrap_or("?"))),
            "emoji" => attr(node, "text")
                .as_str()
                .or(attr(node, "shortName").as_str())
                .unwrap_or("")
                .to_string(),
            "inlineCard" => format!("<{}>", attr(node, "url").as_str().unwrap_or("")),
            "date" => attr(node, "timestamp")
                .as_str()
                .and_then(|t| t.parse::<i64>().ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            "status" => format!("[{}]", attr(node, "text").as_str().unwrap_or("")),
            "media" | "mediaInline" => media(node),
            other => {
                let inner = children(node);
                if inner.is_empty() {
                    format!("[{other}]")
                } else {
                    render_inline(inner)
                }
            }
        })
        .collect()
}

/// Adjacent text nodes with the same marks as one, so wrappers do not repeat.
fn merge_text(nodes: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for node in nodes {
        if node["type"] == "text"
            && let Some(last) = out.last_mut()
            && last["type"] == "text"
            && last["marks"] == node["marks"]
        {
            let joined = format!(
                "{}{}",
                last["text"].as_str().unwrap_or(""),
                node["text"].as_str().unwrap_or("")
            );
            last["text"] = Value::String(joined);
            continue;
        }
        out.push(node.clone());
    }
    out
}

fn styled(node: &Value) -> String {
    let text = node["text"].as_str().unwrap_or("");
    let marks: Vec<&str> = node["marks"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["type"].as_str()).collect())
        .unwrap_or_default();
    let has = |kind: &str| marks.contains(&kind);
    let mut s = text.to_string();
    if has("code") {
        let ticks = if s.contains('`') { "``" } else { "`" };
        s = format!("{ticks}{s}{ticks}");
    }
    if has("em") {
        s = wrap(&s, "*", "*");
    }
    if has("strong") {
        s = wrap(&s, "**", "**");
    }
    if has("strike") {
        s = wrap(&s, "~~", "~~");
    }
    if let Some(link) = node["marks"]
        .as_array()
        .and_then(|a| a.iter().find(|m| m["type"] == "link"))
    {
        let href = link["attrs"]["href"].as_str().unwrap_or("");
        s = wrap(&s, "[", &format!("]({href})"));
    }
    s
}

/// Wrap the text inside its surrounding whitespace, which Markdown delimiters cannot enclose.
fn wrap(s: &str, open: &str, close: &str) -> String {
    let core = s.trim();
    if core.is_empty() {
        return s.to_string();
    }
    let lead = &s[..s.len() - s.trim_start().len()];
    let trail = &s[s.trim_end().len()..];
    format!("{lead}{open}{core}{close}{trail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adf::markdown_to_adf;
    use serde_json::json;

    #[test]
    fn markdown_survives_a_round_trip() {
        let md = "# Title\n\nPlain *em* **strong** ~~gone~~ `code` [link](https://x.y) text.\n\n- a\n- b\n  - c\n\n1. one\n2. two\n\n```rust\nfn main() {}\n```\n\n> quoted\n\n> [!WARNING]\n> careful\n\n| h1 | h2 |\n| --- | --- |\n| x | y |\n\n- [ ] todo\n- [x] done\n\n---\n\n![shot](https://x.y/a.png)";
        assert_eq!(adf_to_markdown(&markdown_to_adf(md)), md);
    }

    #[test]
    fn jira_nodes_render_readably() {
        let doc = json!({ "type": "doc", "version": 1, "content": [
            { "type": "paragraph", "content": [
                { "type": "mention", "attrs": { "id": "1", "text": "@Ann" } },
                { "type": "text", "text": " on " },
                { "type": "date", "attrs": { "timestamp": "1761000000000" } },
                { "type": "text", "text": " " },
                { "type": "status", "attrs": { "text": "DONE" } },
                { "type": "text", "text": " " },
                { "type": "emoji", "attrs": { "shortName": ":smile:" } },
                { "type": "text", "text": " " },
                { "type": "inlineCard", "attrs": { "url": "https://x.y" } },
                { "type": "hardBreak" },
                { "type": "text", "text": "bold", "marks": [{ "type": "strong" }] },
                { "type": "text", "text": " and bold", "marks": [{ "type": "strong" }] }
            ]},
            { "type": "mediaSingle", "content": [
                { "type": "media", "attrs": { "id": "abc", "type": "file", "alt": "log.txt" } }
            ]},
            { "type": "expand", "attrs": { "title": "More" }, "content": [
                { "type": "paragraph", "content": [{ "type": "text", "text": "hidden" }] }
            ]},
            { "type": "panel", "attrs": { "panelType": "success" }, "content": [
                { "type": "paragraph", "content": [{ "type": "text", "text": "ok" }] },
                { "type": "paragraph", "content": [{ "type": "text", "text": "fine" }] }
            ]},
            { "type": "decisionList", "content": [
                { "type": "decisionItem", "content": [{ "type": "text", "text": "ship" }] }
            ]},
            { "type": "blockCard", "attrs": { "url": "https://card" } },
            { "type": "mystery", "attrs": {} }
        ]});
        assert_eq!(
            adf_to_markdown(&doc),
            "@Ann on 2025-10-20 [DONE] :smile: <https://x.y>  \n**bold and bold**\n\n![log.txt](media://abc)\n\n> **More**\n>\n> hidden\n\n> [!TIP]\n> ok\n>\n> fine\n\n- (decision) ship\n\n<https://card>\n\n[mystery]"
        );
    }

    #[test]
    fn empty_paragraphs_add_no_blank_lines() {
        let doc = json!({ "type": "doc", "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "a" }] },
            { "type": "paragraph", "content": [] },
            { "type": "paragraph", "content": [] },
            { "type": "paragraph", "content": [{ "type": "text", "text": "b" }] }
        ]});
        assert_eq!(adf_to_markdown(&doc), "a\n\nb");
    }

    #[test]
    fn tables_without_headers_get_a_blank_one() {
        let doc = json!({ "type": "doc", "content": [{ "type": "table", "content": [
            { "type": "tableRow", "content": [
                { "type": "tableCell", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "a|b" }] }] },
                { "type": "tableCell", "content": [] }
            ]}
        ]}]});
        assert_eq!(
            adf_to_markdown(&doc),
            "|  |  |\n| --- | --- |\n| a\\|b |  |"
        );
        assert_eq!(table(&[]), "");
    }

    #[test]
    fn nested_blocks_indent_under_their_item() {
        let doc = markdown_to_adf("- first\n\n  second para\n\n  ```\n  code\n  ```\n- next");
        assert_eq!(
            adf_to_markdown(&doc),
            "- first\n\n  second para\n\n  ```\n  code\n  ```\n- next"
        );
    }

    #[test]
    fn wrapping_skips_surrounding_spaces() {
        assert_eq!(wrap(" a ", "*", "*"), " *a* ");
        assert_eq!(wrap("  ", "*", "*"), "  ");
        assert_eq!(
            styled(&json!({ "type": "text", "text": "a`b", "marks": [{ "type": "code" }] })),
            "``a`b``"
        );
    }
}
