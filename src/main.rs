mod adf;
mod api;
mod attachment;
mod auth;
mod cli;
mod context;
mod credentials;
mod issue;
mod notifications;
mod notify;
mod output;
mod project;
mod prompt;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};
use context::Ctx;

fn main() -> Result<()> {
    // Exit quietly when stdout closes early, as when piped into head, instead of panicking.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    api::init_tls();
    let cli = Cli::parse();

    // Built on first use, so commands that never reach Jira need no login.
    let ctx = || Ctx::new(cli.site.clone(), cli.json, cli.yes);

    match cli.command {
        Command::Auth(cmd) => auth::run(cli.site.as_deref(), cli.json, cli.yes, cmd),
        Command::Issue(cmd) => issue::run(&ctx()?, cmd),
        Command::Attachment(cmd) => attachment::run(&ctx()?, cmd),
        Command::Project(cmd) => project::run(&ctx()?, cmd),
        Command::Adf(cmd) => adf::run(cmd),
        Command::Notify(cmd) => notifications::run(ctx, cmd),
    }
}
