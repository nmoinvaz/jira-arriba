use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use super::{body, load_spec, spec_description, spec_string, spec_strings};
use crate::api::issues::{self, parse_key};
use crate::api::users;
use crate::cli::EditArgs;
use crate::context::Ctx;
use crate::output::print_json;

pub fn run(ctx: &Ctx, args: EditArgs) -> Result<()> {
    let spec = match &args.from_json {
        Some(path) => load_spec(path)?,
        None => Map::new(),
    };
    let keys: Vec<String> = match &args.key {
        Some(key) => vec![parse_key(key)?],
        None => spec_strings(&spec, "issues")
            .iter()
            .map(|k| parse_key(k))
            .collect::<Result<_>>()?,
    };
    if keys.is_empty() {
        bail!("Pass an issue key, or list issues in --from-json");
    }
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
    let change = changes(&args, &spec, description, |who| {
        users::account_id(&ctx.client, who)
    })?;
    for key in &keys {
        issues::edit(
            &ctx.client,
            key,
            change.fields.clone(),
            change.update.clone(),
        )?;
        if !ctx.json {
            println!("Updated {key}: {}", change.names.join(", "));
        }
    }
    if ctx.json {
        return print_json(&json!({ "updated": keys, "changed": change.names }));
    }
    Ok(())
}

/// What an edit sets, as the API takes it, and the names of the fields it touches.
#[derive(Debug, Default)]
struct Change {
    fields: Map<String, Value>,
    update: Map<String, Value>,
    names: Vec<&'static str>,
}

fn changes(
    args: &EditArgs,
    spec: &Map<String, Value>,
    description: Option<Value>,
    resolve: impl Fn(&str) -> Result<String>,
) -> Result<Change> {
    let mut change = Change::default();
    if let Some(summary) = args
        .summary
        .clone()
        .or_else(|| spec_string(spec, "summary"))
    {
        change.fields.insert("summary".into(), json!(summary));
        change.names.push("summary");
    }
    if let Some(description) = description {
        change.fields.insert("description".into(), description);
        change.names.push("description");
    }
    if let Some(kind) = args
        .issue_type
        .clone()
        .or_else(|| spec_string(spec, "type"))
    {
        change
            .fields
            .insert("issuetype".into(), json!({ "name": kind }));
        change.names.push("type");
    }
    if let Some(who) = args
        .assignee
        .clone()
        .or_else(|| spec_string(spec, "assignee"))
    {
        let value = match who.trim().to_lowercase().as_str() {
            "none" | "null" | "unassigned" => Value::Null,
            _ => json!({ "accountId": resolve(&who)? }),
        };
        change.fields.insert("assignee".into(), value);
        change.names.push("assignee");
    }
    let add = if args.add_label.is_empty() {
        spec_strings(spec, "labelsToAdd")
    } else {
        args.add_label.clone()
    };
    let remove = if args.remove_label.is_empty() {
        spec_strings(spec, "labelsToRemove")
    } else {
        args.remove_label.clone()
    };
    if !add.is_empty() || !remove.is_empty() {
        let ops: Vec<Value> = add
            .iter()
            .map(|l| json!({ "add": l }))
            .chain(remove.iter().map(|l| json!({ "remove": l })))
            .collect();
        change.update.insert("labels".into(), Value::Array(ops));
        change.names.push("labels");
    }
    if change.names.is_empty() {
        bail!(
            "Nothing to change. Pass --summary, --description, --type, --assignee, or a label flag."
        );
    }
    Ok(change)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> EditArgs {
        EditArgs {
            key: Some("A-1".into()),
            summary: None,
            description: None,
            description_file: None,
            assignee: None,
            issue_type: None,
            add_label: vec![],
            remove_label: vec![],
            from_json: None,
            adf: false,
        }
    }

    fn resolve(who: &str) -> Result<String> {
        Ok(format!("id-of-{who}"))
    }

    #[test]
    fn flags_and_spec_build_fields_and_updates() {
        let spec: Map<String, Value> = serde_json::from_value(json!({
            "summary": "spec summary", "type": "Bug", "labelsToRemove": ["old"]
        }))
        .unwrap();
        let flags = EditArgs {
            summary: Some("flag summary".into()),
            assignee: Some("@me".into()),
            add_label: vec!["new".into()],
            ..args()
        };
        let change = changes(&flags, &spec, Some(json!({"type": "doc"})), resolve).unwrap();
        assert_eq!(change.fields["summary"], "flag summary");
        assert_eq!(change.fields["description"], json!({"type": "doc"}));
        assert_eq!(change.fields["issuetype"], json!({"name": "Bug"}));
        assert_eq!(change.fields["assignee"], json!({"accountId": "id-of-@me"}));
        assert_eq!(
            change.update["labels"],
            json!([{"add": "new"}, {"remove": "old"}])
        );
        assert_eq!(
            change.names,
            ["summary", "description", "type", "assignee", "labels"]
        );
    }

    #[test]
    fn none_unassigns_and_nothing_is_an_error() {
        let flags = EditArgs {
            assignee: Some("None".into()),
            ..args()
        };
        let change = changes(&flags, &Map::new(), None, resolve).unwrap();
        assert_eq!(change.fields["assignee"], Value::Null);
        assert!(changes(&args(), &Map::new(), None, resolve).is_err());
    }
}
