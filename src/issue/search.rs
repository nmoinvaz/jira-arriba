use anyhow::Result;
use serde_json::Value;

use super::field_text;
use crate::api::issues::{self, SEARCH_FIELDS};
use crate::cli::SearchArgs;
use crate::context::Ctx;
use crate::output::{print_json, table};

pub fn run(ctx: &Ctx, args: SearchArgs) -> Result<()> {
    let fields = fields(args.fields.as_deref());
    let limit = if args.all { None } else { Some(args.limit) };
    let found = issues::search(&ctx.client, &args.jql, &fields, limit)?;
    if ctx.json {
        return print_json(&found);
    }
    if found.is_empty() {
        println!("No issues match.");
        return Ok(());
    }
    println!("{}", table(&rows(&found, &fields), ""));
    Ok(())
}

/// The fields to fetch, with the long text ones last so the columns line up.
fn fields(arg: Option<&str>) -> Vec<String> {
    let mut fields: Vec<String> = match arg {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .collect(),
        None => SEARCH_FIELDS.iter().map(|f| f.to_string()).collect(),
    };
    fields.sort_by_key(|f| matches!(f.as_str(), "summary" | "description"));
    fields
}

fn rows(issues: &[Value], fields: &[String]) -> Vec<Vec<String>> {
    issues
        .iter()
        .map(|issue| {
            let mut row = vec![issue["key"].as_str().unwrap_or("").to_string()];
            row.extend(fields.iter().map(|f| {
                let text = field_text(&issue["fields"][f]);
                text.lines().next().unwrap_or("").to_string()
            }));
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fields_default_and_put_long_text_last() {
        assert_eq!(fields(None), ["issuetype", "status", "assignee", "summary"]);
        assert_eq!(
            fields(Some("summary, status,,priority")),
            ["status", "priority", "summary"]
        );
    }

    #[test]
    fn rows_take_the_first_line_of_each_field() {
        let issues = vec![json!({"key": "A-1", "fields": {
            "status": {"name": "Open"}, "summary": "one\ntwo", "assignee": null
        }})];
        let fields = fields(Some("status,assignee,summary"));
        assert_eq!(rows(&issues, &fields), vec![vec!["A-1", "Open", "", "one"]]);
    }
}
