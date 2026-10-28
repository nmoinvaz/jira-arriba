use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use super::{body, load_spec, spec_description, spec_string, spec_strings};
use crate::api::issues::{self, parse_key};
use crate::api::users;
use crate::cli::CreateArgs;
use crate::context::Ctx;
use crate::output::print_json;

pub fn run(ctx: &Ctx, args: CreateArgs) -> Result<()> {
    let spec = match &args.from_json {
        Some(path) => load_spec(path)?,
        None => Map::new(),
    };
    let description = if args.description.is_some() || args.description_file.is_some() {
        Some(body(
            args.description.as_deref(),
            args.description_file.as_deref(),
            args.adf,
            "description",
        )?)
    } else {
        spec_description(&spec, args.adf)?
    };
    let fields = fields(&args, &spec, description, |who| {
        users::account_id(&ctx.client, who)
    })?;
    let summary = fields["summary"].as_str().unwrap_or("").to_string();
    let created = issues::create(&ctx.client, fields)?;
    let key = created["key"].as_str().unwrap_or("?").to_string();
    let url = ctx.browse_url(&key);
    if ctx.json {
        return print_json(&json!({ "id": created["id"], "key": key, "url": url }));
    }
    println!("Created {key}: {summary}\n{url}");
    Ok(())
}

/// The fields to create with, from the flags over the spec. `resolve` turns a person into an
/// account ID.
fn fields(
    args: &CreateArgs,
    spec: &Map<String, Value>,
    description: Option<Value>,
    resolve: impl Fn(&str) -> Result<String>,
) -> Result<Map<String, Value>> {
    let project = args
        .project
        .clone()
        .or_else(|| spec_string(spec, "projectKey"))
        .context("Pass --project, or projectKey in --from-json")?;
    let issue_type = args
        .issue_type
        .clone()
        .or_else(|| spec_string(spec, "type"))
        .context("Pass --type, or type in --from-json")?;
    let summary = args
        .summary
        .clone()
        .or_else(|| spec_string(spec, "summary"))
        .context("Pass --summary, or summary in --from-json")?;
    let mut fields = Map::new();
    fields.insert(
        "project".into(),
        json!({ "key": project.trim().to_uppercase() }),
    );
    fields.insert("issuetype".into(), json!({ "name": issue_type }));
    fields.insert("summary".into(), json!(summary));
    if let Some(description) = description {
        fields.insert("description".into(), description);
    }
    if let Some(who) = args
        .assignee
        .clone()
        .or_else(|| spec_string(spec, "assignee"))
    {
        fields.insert("assignee".into(), json!({ "accountId": resolve(&who)? }));
    }
    if let Some(who) = spec_string(spec, "reporter") {
        fields.insert("reporter".into(), json!({ "accountId": resolve(&who)? }));
    }
    let labels = if args.labels.is_empty() {
        spec_strings(spec, "labels")
    } else {
        args.labels.clone()
    };
    if !labels.is_empty() {
        fields.insert("labels".into(), json!(labels));
    }
    if let Some(parent) = args
        .parent
        .clone()
        .or_else(|| spec_string(spec, "parentIssueId"))
    {
        let parent = match parse_key(&parent) {
            Ok(key) => json!({ "key": key }),
            Err(_) => json!({ "id": parent }),
        };
        fields.insert("parent".into(), parent);
    }
    if let Some(extra) = spec.get("additionalAttributes").and_then(Value::as_object) {
        for (name, value) in extra {
            fields.insert(name.clone(), value.clone());
        }
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> CreateArgs {
        CreateArgs {
            project: None,
            issue_type: None,
            summary: None,
            description: None,
            description_file: None,
            assignee: None,
            labels: vec![],
            parent: None,
            from_json: None,
            adf: false,
        }
    }

    fn resolve(who: &str) -> Result<String> {
        Ok(format!("id-of-{who}"))
    }

    #[test]
    fn flags_override_the_spec() {
        let spec: Map<String, Value> = serde_json::from_value(json!({
            "projectKey": "spec", "type": "Task", "summary": "from spec",
            "assignee": "ann@x", "reporter": "bob@x", "labels": ["l1"],
            "parentIssueId": "10001",
            "additionalAttributes": {"customfield_10000": {"value": "v"}}
        }))
        .unwrap();
        let flags = CreateArgs {
            project: Some("flag".into()),
            summary: Some("from flag".into()),
            labels: vec!["a".into(), "b".into()],
            parent: Some("x.atlassian.net/browse/P-7".into()),
            ..args()
        };
        let fields = fields(&flags, &spec, Some(json!({"type": "doc"})), resolve).unwrap();
        assert_eq!(fields["project"], json!({"key": "FLAG"}));
        assert_eq!(fields["issuetype"], json!({"name": "Task"}));
        assert_eq!(fields["summary"], "from flag");
        assert_eq!(fields["description"], json!({"type": "doc"}));
        assert_eq!(fields["assignee"], json!({"accountId": "id-of-ann@x"}));
        assert_eq!(fields["reporter"], json!({"accountId": "id-of-bob@x"}));
        assert_eq!(fields["labels"], json!(["a", "b"]));
        assert_eq!(fields["parent"], json!({"key": "P-7"}));
        assert_eq!(fields["customfield_10000"], json!({"value": "v"}));

        let spec_only = fields_from(&args(), &spec);
        assert_eq!(spec_only["parent"], json!({"id": "10001"}));
        assert_eq!(spec_only["labels"], json!(["l1"]));
        assert!(!spec_only.contains_key("description"));
    }

    fn fields_from(args: &CreateArgs, spec: &Map<String, Value>) -> Map<String, Value> {
        fields(args, spec, None, resolve).unwrap()
    }

    #[test]
    fn required_fields_are_named_when_missing() {
        let err = fields(&args(), &Map::new(), None, resolve).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Pass --project, or projectKey in --from-json"
        );
        let partial = CreateArgs {
            project: Some("P".into()),
            issue_type: Some("Bug".into()),
            ..args()
        };
        let err = fields(&partial, &Map::new(), None, resolve).unwrap_err();
        assert_eq!(err.to_string(), "Pass --summary, or summary in --from-json");
    }
}
