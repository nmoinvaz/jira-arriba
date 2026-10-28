use anyhow::Result;
use serde_json::json;

use super::body;
use super::view::comment_header;
use crate::adf;
use crate::api::comments;
use crate::api::issues::parse_key;
use crate::cli::{CommentAddArgs, CommentCommand, CommentDeleteArgs, CommentEditArgs, KeyArg};
use crate::context::Ctx;
use crate::output::print_json;
use crate::prompt;

pub fn run(ctx: &Ctx, cmd: CommentCommand) -> Result<()> {
    match cmd {
        CommentCommand::List(args) => list(ctx, args),
        CommentCommand::Add(args) => add(ctx, args),
        CommentCommand::Edit(args) => edit(ctx, args),
        CommentCommand::Delete(args) => delete(ctx, args),
    }
}

fn list(ctx: &Ctx, args: KeyArg) -> Result<()> {
    let key = parse_key(&args.key)?;
    let comments = comments::list(&ctx.client, &key)?;
    if ctx.json {
        return print_json(&comments);
    }
    if comments.is_empty() {
        println!("No comments on {key}.");
        return Ok(());
    }
    for (i, c) in comments.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", comment_header(c));
        let text = adf::field_markdown(&c["body"]);
        if !text.trim().is_empty() {
            println!("{}", text.trim_end());
        }
    }
    Ok(())
}

fn add(ctx: &Ctx, args: CommentAddArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    let doc = body(
        args.body.as_deref(),
        args.file.as_deref(),
        args.adf,
        "comment",
    )?;
    let comment = comments::add(&ctx.client, &key, &doc)?;
    let id = comment["id"].as_str().unwrap_or("?");
    if ctx.json {
        return print_json(&comment);
    }
    println!(
        "Added comment {id} to {key}\n{}",
        comment_url(ctx, &key, id)
    );
    Ok(())
}

fn edit(ctx: &Ctx, args: CommentEditArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    let doc = body(
        args.body.as_deref(),
        args.file.as_deref(),
        args.adf,
        "comment",
    )?;
    let comment = comments::edit(&ctx.client, &key, &args.id, &doc)?;
    if ctx.json {
        return print_json(&comment);
    }
    println!(
        "Updated comment {} on {key}\n{}",
        args.id,
        comment_url(ctx, &key, &args.id)
    );
    Ok(())
}

fn delete(ctx: &Ctx, args: CommentDeleteArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    let question = format!("Delete comment {} on {key}?", args.id);
    if !prompt::confirm(&question, ctx.yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    comments::delete(&ctx.client, &key, &args.id)?;
    if ctx.json {
        return print_json(&json!({ "key": key, "deleted": args.id }));
    }
    println!("Deleted comment {} from {key}", args.id);
    Ok(())
}

fn comment_url(ctx: &Ctx, key: &str, id: &str) -> String {
    format!("{}?focusedCommentId={id}", ctx.browse_url(key))
}
