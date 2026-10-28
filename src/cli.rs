use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "jira-arriba",
    about = "Jira workflows that acli doesn't ship",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Jira site, as in mysite.atlassian.net, when acli has several logins
    #[arg(long, global = true, value_name = "SITE")]
    pub site: Option<String>,

    /// Print machine-readable JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Skip confirmation prompts
    #[arg(short, long, global = true)]
    pub yes: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show the login in use, log in through acli, or print its token
    #[command(subcommand)]
    Auth(AuthCommand),
    /// View, search, create, edit, transition, and comment on issues
    #[command(subcommand, alias = "workitem")]
    Issue(IssueCommand),
    /// List, download, upload, and delete attachments
    #[command(subcommand)]
    Attachment(AttachmentCommand),
    /// List projects
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Convert between Markdown and Atlassian Document Format
    #[command(subcommand)]
    Adf(AdfCommand),
    /// Watch your issues and show a desktop notification for each change
    #[command(subcommand)]
    Notify(NotifyCommand),
}

#[derive(Subcommand)]
pub enum NotifyCommand {
    /// Poll your issues and show a desktop notification for each assignment, status change,
    /// comment, or mention, until stopped
    Watch(NotifyWatchArgs),
}

/// A kind of activity the watch reports.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// An issue assigned to you
    Assigned,
    /// A status change on an issue you are on
    Status,
    /// A comment on an issue you are on
    Comment,
    /// A comment that mentions you
    Mention,
}

#[derive(Args)]
pub struct NotifyWatchArgs {
    /// Seconds between polls
    #[arg(long, value_name = "SECS", default_value_t = 60, value_parser = clap::value_parser!(u64).range(10..))]
    pub interval: u64,

    /// Only this kind of activity, repeatable
    #[arg(long, value_enum, value_name = "KIND")]
    pub reason: Vec<Reason>,

    /// JQL for the issues to watch, without ORDER BY (default: ones you are assigned, reported, or
    /// watch)
    #[arg(long, value_name = "JQL")]
    pub jql: Option<String>,

    /// Also report activity from this long before the start, such as 30m or 2h
    #[arg(long, value_name = "SPAN")]
    pub since: Option<String>,

    /// Poll once and exit, with --since
    #[arg(long, requires = "since", conflicts_with_all = ["install", "uninstall"])]
    pub once: bool,

    /// Start the watch now and at every login, with the options given
    #[arg(long, conflicts_with = "uninstall")]
    pub install: bool,

    /// Stop the watch that starts at login
    #[arg(long)]
    pub uninstall: bool,
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Show which login is in use and who it belongs to
    Status,
    /// Log in through acli, in the browser by default
    Login(LoginArgs),
    /// Print the access token, or the whole Authorization header with --header
    Token(TokenArgs),
}

#[derive(Args)]
pub struct LoginArgs {
    /// Log in with the browser (default)
    #[arg(short, long)]
    pub web: bool,

    /// Email of the account, for an API token login
    #[arg(short, long, requires = "token")]
    pub email: Option<String>,

    /// Read an API token from stdin instead of using the browser, with --email and --site
    #[arg(long, requires = "email")]
    pub token: bool,
}

#[derive(Args)]
pub struct TokenArgs {
    /// Print as an Authorization header, ready for curl -H
    #[arg(long)]
    pub header: bool,
}

#[derive(Subcommand)]
pub enum IssueCommand {
    /// Show an issue with its description and comments as Markdown
    View(ViewArgs),
    /// Search issues with JQL
    Search(SearchArgs),
    /// Create an issue, with a Markdown description
    Create(CreateArgs),
    /// Edit an issue's summary, description, type, assignee, or labels
    Edit(EditArgs),
    /// Move an issue to a status, or list the statuses it can move to
    Transition(TransitionArgs),
    /// List, add, edit, and delete comments
    #[command(subcommand)]
    Comment(CommentCommand),
}

#[derive(Args)]
pub struct ViewArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Comma-separated fields to show instead of the usual set (default: summary, status,
    /// people, dates, labels, description, comments, attachments)
    #[arg(short, long, conflicts_with = "all")]
    pub fields: Option<String>,

    /// Show every field, custom fields included
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    /// JQL query, such as 'project = PROJ AND status = Open'
    #[arg(short, long, value_name = "JQL")]
    pub jql: String,

    /// Comma-separated fields to fetch and show (default: issuetype, status, assignee, summary)
    #[arg(short, long)]
    pub fields: Option<String>,

    /// Most issues to return
    #[arg(short, long, default_value_t = 50, conflicts_with = "all")]
    pub limit: usize,

    /// Return every match
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct CreateArgs {
    /// Project key
    #[arg(short, long)]
    pub project: Option<String>,

    /// Issue type, such as Bug or Task
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub issue_type: Option<String>,

    /// Summary
    #[arg(short, long)]
    pub summary: Option<String>,

    /// Description as Markdown, or as ADF JSON
    #[arg(short, long, conflicts_with = "description_file")]
    pub description: Option<String>,

    /// Read the description from a file, Markdown or ADF JSON
    #[arg(long, value_name = "FILE")]
    pub description_file: Option<PathBuf>,

    /// Assignee by email, account ID, or @me
    #[arg(short, long)]
    pub assignee: Option<String>,

    /// Label to add, repeatable or comma-separated
    #[arg(short, long = "label", value_name = "LABEL", value_delimiter = ',')]
    pub labels: Vec<String>,

    /// Parent issue key, for sub-tasks and stories under an epic
    #[arg(long, value_name = "KEY")]
    pub parent: Option<String>,

    /// Read the issue from a JSON file in acli's --generate-json layout; flags override its fields
    #[arg(long, value_name = "FILE")]
    pub from_json: Option<PathBuf>,

    /// Treat the description as ADF JSON rather than detecting it
    #[arg(long)]
    pub adf: bool,
}

#[derive(Args)]
pub struct EditArgs {
    /// Issue key, such as PROJ-123, or its URL (default: the issues in --from-json)
    pub key: Option<String>,

    /// New summary
    #[arg(short, long)]
    pub summary: Option<String>,

    /// New description as Markdown, or as ADF JSON
    #[arg(short, long, conflicts_with = "description_file")]
    pub description: Option<String>,

    /// Read the new description from a file, Markdown or ADF JSON
    #[arg(long, value_name = "FILE")]
    pub description_file: Option<PathBuf>,

    /// New assignee by email, account ID, @me, or none to unassign
    #[arg(short, long)]
    pub assignee: Option<String>,

    /// New issue type
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub issue_type: Option<String>,

    /// Label to add, repeatable or comma-separated
    #[arg(long, value_name = "LABEL", value_delimiter = ',')]
    pub add_label: Vec<String>,

    /// Label to remove, repeatable or comma-separated
    #[arg(long, value_name = "LABEL", value_delimiter = ',')]
    pub remove_label: Vec<String>,

    /// Read the edits from a JSON file in acli's --generate-json layout; flags override its fields
    #[arg(long, value_name = "FILE")]
    pub from_json: Option<PathBuf>,

    /// Treat the description as ADF JSON rather than detecting it
    #[arg(long)]
    pub adf: bool,
}

#[derive(Args)]
pub struct TransitionArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Status or transition name to move to, matched ignoring case
    #[arg(short, long, required_unless_present = "list")]
    pub status: Option<String>,

    /// List the transitions the issue can take
    #[arg(short, long)]
    pub list: bool,
}

#[derive(Subcommand)]
pub enum CommentCommand {
    /// List comments with their bodies as Markdown
    List(KeyArg),
    /// Add a comment written in Markdown, or as ADF JSON
    #[command(alias = "create")]
    Add(CommentAddArgs),
    /// Replace a comment's body
    #[command(alias = "update")]
    Edit(CommentEditArgs),
    /// Delete a comment
    Delete(CommentDeleteArgs),
}

#[derive(Args)]
pub struct KeyArg {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,
}

#[derive(Args)]
pub struct CommentAddArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Markdown or ADF JSON, or - to read stdin (default: stdin)
    pub body: Option<String>,

    /// Read the body from a file
    #[arg(short = 'F', long, value_name = "FILE", conflicts_with = "body")]
    pub file: Option<PathBuf>,

    /// Treat the body as ADF JSON rather than detecting it
    #[arg(long)]
    pub adf: bool,
}

#[derive(Args)]
pub struct CommentEditArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Comment ID
    pub id: String,

    /// Markdown or ADF JSON, or - to read stdin (default: stdin)
    pub body: Option<String>,

    /// Read the body from a file
    #[arg(short = 'F', long, value_name = "FILE", conflicts_with = "body")]
    pub file: Option<PathBuf>,

    /// Treat the body as ADF JSON rather than detecting it
    #[arg(long)]
    pub adf: bool,
}

#[derive(Args)]
pub struct CommentDeleteArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Comment ID
    pub id: String,
}

#[derive(Subcommand)]
pub enum AttachmentCommand {
    /// List an issue's attachments, including ones only the changelog or inline media mention
    List(KeyArg),
    /// Download an issue's attachments and inline media, extracting zip files
    Download(DownloadArgs),
    /// Attach files to an issue
    Upload(UploadArgs),
    /// Delete attachments by ID
    Delete(AttachmentDeleteArgs),
}

#[derive(Args)]
pub struct DownloadArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Directory to download into (default: a jira-arriba/KEY folder in the temp directory)
    #[arg(short = 'o', long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Only files matching a glob such as '*.log', repeatable
    #[arg(short, long = "name", value_name = "GLOB")]
    pub names: Vec<String>,

    /// Leave zip files as they are
    #[arg(long)]
    pub no_extract: bool,
}

#[derive(Args)]
pub struct UploadArgs {
    /// Issue key, such as PROJ-123, or its URL
    pub key: String,

    /// Files to attach
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
}

#[derive(Args)]
pub struct AttachmentDeleteArgs {
    /// Attachment IDs, as `attachment list` shows them
    #[arg(required = true, value_name = "ID")]
    pub ids: Vec<String>,
}

#[derive(Subcommand)]
pub enum ProjectCommand {
    /// List projects you can see
    List(ProjectListArgs),
}

#[derive(Args)]
pub struct ProjectListArgs {
    /// Only projects whose key or name contains this text
    #[arg(short, long)]
    pub query: Option<String>,

    /// Most projects to return
    #[arg(short, long, default_value_t = 50, conflicts_with = "all")]
    pub limit: usize,

    /// Return every project
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand)]
pub enum AdfCommand {
    /// Print Markdown as an ADF document
    #[command(name = "from-md", alias = "from-markdown")]
    FromMd(FileArg),
    /// Print an ADF document, or an issue or comment JSON holding one, as Markdown
    #[command(name = "to-md", alias = "to-markdown")]
    ToMd(FileArg),
}

#[derive(Args)]
pub struct FileArg {
    /// File to read (default: stdin)
    pub file: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
