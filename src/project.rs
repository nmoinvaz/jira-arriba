use anyhow::Result;

use crate::api::projects;
use crate::cli::{ProjectCommand, ProjectListArgs};
use crate::context::Ctx;
use crate::output::{print_json, table};

pub fn run(ctx: &Ctx, cmd: ProjectCommand) -> Result<()> {
    match cmd {
        ProjectCommand::List(args) => list(ctx, args),
    }
}

fn list(ctx: &Ctx, args: ProjectListArgs) -> Result<()> {
    let limit = if args.all { None } else { Some(args.limit) };
    let projects = projects::list(&ctx.client, args.query.as_deref(), limit)?;
    if ctx.json {
        return print_json(&projects);
    }
    if projects.is_empty() {
        println!("No projects found.");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = projects
        .iter()
        .map(|p| {
            vec![
                p["key"].as_str().unwrap_or("").to_string(),
                p["projectTypeKey"].as_str().unwrap_or("").to_string(),
                p["name"].as_str().unwrap_or("").to_string(),
            ]
        })
        .collect();
    println!("{}", table(&rows, ""));
    Ok(())
}
