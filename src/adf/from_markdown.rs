//! Markdown to ADF, built from pulldown-cmark's events with a stack of open nodes.

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Map, Value, json};

/// An ADF document for `markdown`, with GitHub tables, task lists, strikethrough, and alerts.
pub fn markdown_to_adf(markdown: &str) -> Value {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut builder = Builder::new();
    for event in Parser::new_ext(markdown, options) {
        builder.event(event);
    }
    builder.finish()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Doc,
    Paragraph,
    Heading,
    CodeBlock,
    Blockquote,
    Panel,
    BulletList,
    OrderedList,
    ListItem,
    Table,
    TableRow,
    TableCell,
    TableHeader,
    /// Collects an image's alt text until it ends.
    Image,
}

impl Kind {
    /// Whether text goes straight into the node.
    fn inline(self) -> bool {
        matches!(
            self,
            Kind::Paragraph | Kind::Heading | Kind::CodeBlock | Kind::Image
        )
    }
}

struct Node {
    kind: Kind,
    attrs: Map<String, Value>,
    content: Vec<Value>,
    /// A paragraph opened for text that Markdown put directly in a block, as in a tight list.
    implicit: bool,
    /// A list item's task box, when it has one.
    task: Option<bool>,
    /// An image's alt text.
    alt: String,
}

impl Node {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            attrs: Map::new(),
            content: Vec::new(),
            implicit: false,
            task: None,
            alt: String::new(),
        }
    }
}

struct Builder {
    stack: Vec<Node>,
    marks: Vec<Value>,
    /// Depth inside metadata blocks, whose text is dropped.
    skip: usize,
    in_table_head: bool,
    /// Task items and lists need unique local IDs.
    local_ids: u32,
}

impl Builder {
    fn new() -> Self {
        Self {
            stack: vec![Node::new(Kind::Doc)],
            marks: Vec::new(),
            skip: 0,
            in_table_head: false,
            local_ids: 0,
        }
    }

    fn top(&mut self) -> &mut Node {
        self.stack
            .last_mut()
            .expect("the document stays on the stack")
    }

    fn next_local_id(&mut self) -> String {
        self.local_ids += 1;
        self.local_ids.to_string()
    }

    /// Open a block node, ending any paragraph that text opened on its own.
    fn open(&mut self, kind: Kind) -> &mut Node {
        self.close_implicit();
        self.stack.push(Node::new(kind));
        self.top()
    }

    fn close_implicit(&mut self) {
        let top = self.top();
        if top.kind == Kind::Paragraph && top.implicit {
            self.close();
        }
    }

    /// Finish the top node and add it to its parent.
    fn close(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let node = self.stack.pop().expect("checked above");
        let value = self.value(node);
        self.top().content.push(value);
    }

    /// Close the open node of `kind`, and anything left open inside it.
    fn close_kind(&mut self, kind: Kind) {
        self.close_implicit();
        if !self.stack.iter().any(|n| n.kind == kind) {
            return;
        }
        while self.top().kind != kind {
            self.close();
        }
        self.close();
    }

    /// Make sure text has a node to go into.
    fn ensure_inline(&mut self) {
        if !self.top().kind.inline() {
            let mut p = Node::new(Kind::Paragraph);
            p.implicit = true;
            self.stack.push(p);
        }
    }

    fn push_inline(&mut self, value: Value) {
        self.ensure_inline();
        self.top().content.push(value);
    }

    fn push_text(&mut self, text: &str, marks: Vec<Value>) {
        if text.is_empty() {
            return;
        }
        if self.top().kind == Kind::Image {
            self.top().alt.push_str(text);
            return;
        }
        self.ensure_inline();
        let marks_value = if marks.is_empty() {
            Value::Null
        } else {
            Value::Array(marks)
        };
        let content = &mut self.top().content;
        if let Some(last) = content.last_mut()
            && last["type"] == "text"
            && last["marks"] == marks_value
            && let Some(existing) = last["text"].as_str()
        {
            let joined = format!("{existing}{text}");
            last["text"] = Value::String(joined);
            return;
        }
        let mut node = json!({ "type": "text", "text": text });
        if !marks_value.is_null() {
            node["marks"] = marks_value;
        }
        content.push(node);
    }

    fn pop_mark(&mut self, kind: &str) {
        if let Some(i) = self.marks.iter().rposition(|m| m["type"] == kind) {
            self.marks.remove(i);
        }
    }

    /// Marks for inline code, which ADF lets combine with links alone.
    fn code_marks(&self) -> Vec<Value> {
        let mut marks = vec![json!({ "type": "code" })];
        marks.extend(self.marks.iter().filter(|m| m["type"] == "link").cloned());
        marks
    }

    fn event(&mut self, event: Event) {
        if self.skip > 0 {
            if let Event::End(TagEnd::MetadataBlock(_)) = event {
                self.skip -= 1;
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                let marks = self.marks.clone();
                self.push_text(&t, marks);
            }
            Event::Code(c) => {
                let marks = self.code_marks();
                self.push_text(&c, marks);
            }
            Event::InlineMath(m) => {
                let marks = self.marks.clone();
                self.push_text(&format!("${m}$"), marks);
            }
            Event::DisplayMath(m) => {
                let marks = self.marks.clone();
                self.push_text(&format!("$${m}$$"), marks);
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let marks = self.marks.clone();
                self.push_text(&h, marks);
            }
            Event::FootnoteReference(r) => {
                let marks = self.marks.clone();
                self.push_text(&format!("[^{r}]"), marks);
            }
            Event::SoftBreak => {
                let marks = self.marks.clone();
                self.push_text(" ", marks);
            }
            Event::HardBreak => self.push_inline(json!({ "type": "hardBreak" })),
            Event::Rule => {
                self.close_implicit();
                self.top().content.push(json!({ "type": "rule" }));
            }
            Event::TaskListMarker(checked) => {
                if let Some(item) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|n| n.kind == Kind::ListItem)
                {
                    item.task = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.open(Kind::Paragraph);
            }
            Tag::Heading { level, .. } => {
                self.open(Kind::Heading)
                    .attrs
                    .insert("level".into(), json!(level as u8));
            }
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                let node = self.open(Kind::CodeBlock);
                if !language.is_empty() {
                    node.attrs.insert("language".into(), json!(language));
                }
            }
            Tag::BlockQuote(None) => {
                self.open(Kind::Blockquote);
            }
            Tag::BlockQuote(Some(kind)) => {
                let panel = match kind {
                    BlockQuoteKind::Note => "info",
                    BlockQuoteKind::Tip => "success",
                    BlockQuoteKind::Important => "note",
                    BlockQuoteKind::Warning => "warning",
                    BlockQuoteKind::Caution => "error",
                };
                self.open(Kind::Panel)
                    .attrs
                    .insert("panelType".into(), json!(panel));
            }
            Tag::List(Some(start)) => {
                self.open(Kind::OrderedList)
                    .attrs
                    .insert("order".into(), json!(start));
            }
            Tag::List(None) => {
                self.open(Kind::BulletList);
            }
            Tag::Item => {
                self.open(Kind::ListItem);
            }
            Tag::Table(_) => {
                self.open(Kind::Table);
            }
            Tag::TableHead => {
                self.open(Kind::TableRow);
                self.in_table_head = true;
            }
            Tag::TableRow => {
                self.open(Kind::TableRow);
            }
            Tag::TableCell => {
                let kind = if self.in_table_head {
                    Kind::TableHeader
                } else {
                    Kind::TableCell
                };
                self.open(kind);
            }
            Tag::Emphasis => self.marks.push(json!({ "type": "em" })),
            Tag::Strong => self.marks.push(json!({ "type": "strong" })),
            Tag::Strikethrough => self.marks.push(json!({ "type": "strike" })),
            Tag::Superscript => self
                .marks
                .push(json!({ "type": "subsup", "attrs": { "type": "sup" } })),
            Tag::Subscript => self
                .marks
                .push(json!({ "type": "subsup", "attrs": { "type": "sub" } })),
            Tag::Link {
                dest_url, title, ..
            } => {
                let mut attrs = json!({ "href": dest_url.to_string() });
                if !title.is_empty() {
                    attrs["title"] = json!(title.to_string());
                }
                self.marks.push(json!({ "type": "link", "attrs": attrs }));
            }
            Tag::Image { dest_url, .. } => {
                self.ensure_inline();
                let mut node = Node::new(Kind::Image);
                node.attrs.insert("url".into(), json!(dest_url.to_string()));
                self.stack.push(node);
            }
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
            Tag::MetadataBlock(_) => self.skip += 1,
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.close_kind(Kind::Paragraph),
            TagEnd::Heading(_) => self.close_kind(Kind::Heading),
            TagEnd::CodeBlock => self.close_kind(Kind::CodeBlock),
            TagEnd::BlockQuote(None) => self.close_kind(Kind::Blockquote),
            TagEnd::BlockQuote(Some(_)) => self.close_kind(Kind::Panel),
            TagEnd::List(true) => self.close_kind(Kind::OrderedList),
            TagEnd::List(false) => self.close_kind(Kind::BulletList),
            TagEnd::Item => self.close_kind(Kind::ListItem),
            TagEnd::Table => self.close_kind(Kind::Table),
            TagEnd::TableHead => {
                self.close_kind(Kind::TableRow);
                self.in_table_head = false;
            }
            TagEnd::TableRow => self.close_kind(Kind::TableRow),
            TagEnd::TableCell => {
                let kind = if self.in_table_head {
                    Kind::TableHeader
                } else {
                    Kind::TableCell
                };
                self.close_kind(kind);
            }
            TagEnd::Emphasis => self.pop_mark("em"),
            TagEnd::Strong => self.pop_mark("strong"),
            TagEnd::Strikethrough => self.pop_mark("strike"),
            TagEnd::Superscript | TagEnd::Subscript => self.pop_mark("subsup"),
            TagEnd::Link => self.pop_mark("link"),
            TagEnd::Image => {
                if self.top().kind == Kind::Image {
                    let node = self.stack.pop().expect("image is on top");
                    let image = json!({
                        "type": "__image",
                        "url": node.attrs.get("url").cloned().unwrap_or(Value::Null),
                        "alt": node.alt,
                    });
                    self.push_inline(image);
                }
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => self.close_implicit(),
            TagEnd::MetadataBlock(_) => {}
        }
    }

    /// The ADF node for a finished builder node.
    fn value(&mut self, node: Node) -> Value {
        let Node {
            kind,
            attrs,
            mut content,
            task,
            ..
        } = node;
        match kind {
            Kind::Doc | Kind::Image => unreachable!("never closed as a value"),
            Kind::Paragraph => paragraph(content),
            Kind::Heading => json!({
                "type": "heading",
                "attrs": attrs,
                "content": resolve_images(content),
            }),
            Kind::CodeBlock => {
                if let Some(last) = content.last_mut()
                    && let Some(text) = last["text"].as_str()
                    && let Some(trimmed) = text.strip_suffix('\n')
                {
                    last["text"] = Value::String(trimmed.to_string());
                }
                let mut v = json!({ "type": "codeBlock", "content": content });
                if !attrs.is_empty() {
                    v["attrs"] = Value::Object(attrs);
                }
                v
            }
            Kind::Blockquote => json!({ "type": "blockquote", "content": non_empty(content) }),
            Kind::Panel => json!({
                "type": "panel",
                "attrs": attrs,
                "content": non_empty(content),
            }),
            Kind::BulletList | Kind::OrderedList => self.list(kind, attrs, content),
            Kind::ListItem => match task {
                Some(checked) => json!({
                    "type": "taskItem",
                    "attrs": {
                        "localId": self.next_local_id(),
                        "state": if checked { "DONE" } else { "TODO" },
                    },
                    "content": inline_of(content),
                }),
                None => json!({ "type": "listItem", "content": non_empty(content) }),
            },
            Kind::Table => json!({
                "type": "table",
                "attrs": { "isNumberColumnEnabled": false, "layout": "default" },
                "content": content,
            }),
            Kind::TableRow => json!({ "type": "tableRow", "content": content }),
            Kind::TableCell => json!({ "type": "tableCell", "content": non_empty(content) }),
            Kind::TableHeader => json!({ "type": "tableHeader", "content": non_empty(content) }),
        }
    }

    /// A list, which becomes a task list when every item has a task box.
    fn list(&mut self, kind: Kind, attrs: Map<String, Value>, content: Vec<Value>) -> Value {
        if !content.is_empty() && content.iter().all(|c| c["type"] == "taskItem") {
            return json!({
                "type": "taskList",
                "attrs": { "localId": self.next_local_id() },
                "content": content,
            });
        }
        let content: Vec<Value> = content.into_iter().map(untask).collect();
        let mut v = json!({
            "type": if kind == Kind::OrderedList { "orderedList" } else { "bulletList" },
            "content": content,
        });
        if kind == Kind::OrderedList && !attrs.is_empty() {
            v["attrs"] = Value::Object(attrs);
        }
        v
    }

    fn finish(mut self) -> Value {
        while self.stack.len() > 1 {
            self.close();
        }
        let doc = self.stack.pop().expect("the document");
        json!({ "type": "doc", "version": 1, "content": doc.content })
    }
}

/// A paragraph, or a media block when an image stood alone in it.
fn paragraph(content: Vec<Value>) -> Value {
    if content.len() == 1 && content[0]["type"] == "__image" {
        let image = &content[0];
        return json!({
            "type": "mediaSingle",
            "attrs": { "layout": "center" },
            "content": [{
                "type": "media",
                "attrs": { "type": "external", "url": image["url"], "alt": image["alt"] },
            }],
        });
    }
    json!({ "type": "paragraph", "content": resolve_images(content) })
}

/// Images among inline nodes become links, since ADF has no inline image for a URL.
fn resolve_images(content: Vec<Value>) -> Vec<Value> {
    content
        .into_iter()
        .map(|node| {
            if node["type"] != "__image" {
                return node;
            }
            let url = node["url"].as_str().unwrap_or("").to_string();
            let alt = node["alt"].as_str().unwrap_or("").to_string();
            json!({
                "type": "text",
                "text": if alt.is_empty() { url.clone() } else { alt },
                "marks": [{ "type": "link", "attrs": { "href": url } }],
            })
        })
        .collect()
}

/// Blocks, or an empty paragraph where ADF requires at least one node.
fn non_empty(content: Vec<Value>) -> Vec<Value> {
    if content.is_empty() {
        vec![json!({ "type": "paragraph", "content": [] })]
    } else {
        content
    }
}

/// The inline nodes of a list item's paragraphs, joined by hard breaks.
fn inline_of(content: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for block in content {
        if block["type"] != "paragraph" {
            continue;
        }
        if let Some(inline) = block["content"].as_array() {
            if !out.is_empty() && !inline.is_empty() {
                out.push(json!({ "type": "hardBreak" }));
            }
            out.extend(inline.iter().cloned());
        }
    }
    out
}

/// A task item back into a list item with its box spelled out, for lists that mix the two.
fn untask(item: Value) -> Value {
    if item["type"] != "taskItem" {
        return item;
    }
    let done = item["attrs"]["state"] == "DONE";
    let mut inline = vec![json!({ "type": "text", "text": if done { "[x] " } else { "[ ] " } })];
    inline.extend(item["content"].as_array().into_iter().flatten().cloned());
    json!({
        "type": "listItem",
        "content": [{ "type": "paragraph", "content": inline }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(md: &str) -> Vec<Value> {
        markdown_to_adf(md)["content"].as_array().unwrap().clone()
    }

    fn text(t: &str) -> Value {
        json!({ "type": "text", "text": t })
    }

    #[test]
    fn empty_input_is_an_empty_document() {
        assert_eq!(
            markdown_to_adf(""),
            json!({ "type": "doc", "version": 1, "content": [] })
        );
    }

    #[test]
    fn paragraphs_merge_runs_and_keep_marks() {
        let blocks =
            doc("Plain *em* **strong** ~~gone~~ `code` [link](https://x.y \"T\")\nnext line");
        assert_eq!(blocks.len(), 1);
        let content = blocks[0]["content"].as_array().unwrap();
        assert_eq!(content[0], text("Plain "));
        assert_eq!(content[1]["marks"], json!([{ "type": "em" }]));
        assert_eq!(content[3]["marks"], json!([{ "type": "strong" }]));
        assert_eq!(content[5]["marks"], json!([{ "type": "strike" }]));
        assert_eq!(content[7]["marks"], json!([{ "type": "code" }]));
        assert_eq!(
            content[9]["marks"],
            json!([{ "type": "link", "attrs": { "href": "https://x.y", "title": "T" } }])
        );
        assert_eq!(content[10], text(" next line"));
    }

    #[test]
    fn headings_code_blocks_rules_and_breaks() {
        let blocks =
            doc("## Title\n\n```rust\nfn main() {}\n```\n\n    indented\n\n---\n\nline  \nbreak");
        assert_eq!(
            blocks[0],
            json!({ "type": "heading", "attrs": { "level": 2 }, "content": [text("Title")] })
        );
        assert_eq!(
            blocks[1],
            json!({ "type": "codeBlock", "attrs": { "language": "rust" }, "content": [text("fn main() {}")] })
        );
        assert_eq!(
            blocks[2],
            json!({ "type": "codeBlock", "content": [text("indented")] })
        );
        assert_eq!(blocks[3], json!({ "type": "rule" }));
        assert_eq!(
            blocks[4]["content"],
            json!([text("line"), { "type": "hardBreak" }, text("break")])
        );
    }

    #[test]
    fn lists_nest_and_number() {
        let blocks = doc("- a\n- b\n  - c\n\n3. three\n4. four");
        assert_eq!(blocks[0]["type"], "bulletList");
        let items = blocks[0]["content"].as_array().unwrap();
        assert_eq!(
            items[0],
            json!({ "type": "listItem", "content": [{ "type": "paragraph", "content": [text("a")] }] })
        );
        assert_eq!(items[1]["content"][1]["type"], "bulletList");
        assert_eq!(blocks[1]["type"], "orderedList");
        assert_eq!(blocks[1]["attrs"], json!({ "order": 3 }));
        assert_eq!(blocks[1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn task_lists_become_task_items_or_spelled_boxes() {
        let blocks = doc("- [ ] todo\n- [x] done");
        assert_eq!(blocks[0]["type"], "taskList");
        let items = blocks[0]["content"].as_array().unwrap();
        assert_eq!(items[0]["attrs"]["state"], "TODO");
        assert_eq!(items[1]["attrs"]["state"], "DONE");
        assert_eq!(items[1]["content"], json!([text("done")]));
        assert_ne!(items[0]["attrs"]["localId"], items[1]["attrs"]["localId"]);

        let mixed = doc("- [x] done\n- plain");
        assert_eq!(mixed[0]["type"], "bulletList");
        assert_eq!(
            mixed[0]["content"][0]["content"][0]["content"],
            json!([text("[x] "), text("done")])
        );
    }

    #[test]
    fn quotes_and_alerts() {
        let blocks = doc("> quoted\n\n> [!WARNING]\n> careful");
        assert_eq!(
            blocks[0],
            json!({ "type": "blockquote", "content": [{ "type": "paragraph", "content": [text("quoted")] }] })
        );
        assert_eq!(blocks[1]["type"], "panel");
        assert_eq!(blocks[1]["attrs"], json!({ "panelType": "warning" }));
        assert_eq!(blocks[1]["content"][0]["content"], json!([text("careful")]));
    }

    #[test]
    fn tables_have_header_and_body_rows() {
        let blocks = doc("| a | b |\n| --- | --- |\n| 1 | |");
        assert_eq!(blocks[0]["type"], "table");
        let rows = blocks[0]["content"].as_array().unwrap();
        assert_eq!(rows[0]["content"][0]["type"], "tableHeader");
        assert_eq!(
            rows[0]["content"][0]["content"][0]["content"],
            json!([text("a")])
        );
        assert_eq!(rows[1]["content"][0]["type"], "tableCell");
        assert_eq!(
            rows[1]["content"][1]["content"],
            json!([{ "type": "paragraph", "content": [] }])
        );
    }

    #[test]
    fn images_become_media_or_links() {
        let blocks = doc("![shot](https://x.y/a.png)\n\nsee ![alt](https://x.y/b.png) here");
        assert_eq!(blocks[0]["type"], "mediaSingle");
        assert_eq!(
            blocks[0]["content"][0]["attrs"],
            json!({ "type": "external", "url": "https://x.y/a.png", "alt": "shot" })
        );
        let inline = blocks[1]["content"].as_array().unwrap();
        assert_eq!(inline[1]["text"], "alt");
        assert_eq!(inline[1]["marks"][0]["attrs"]["href"], "https://x.y/b.png");
    }

    #[test]
    fn code_keeps_only_link_marks() {
        let blocks = doc("**[`x`](https://x.y)**");
        let marks = blocks[0]["content"][0]["marks"].as_array().unwrap();
        let kinds: Vec<&str> = marks.iter().map(|m| m["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["code", "link"]);
    }

    #[test]
    fn html_stays_as_text() {
        let blocks = doc("a <b>bold</b> c");
        assert_eq!(blocks[0]["content"], json!([text("a <b>bold</b> c")]));
    }
}
