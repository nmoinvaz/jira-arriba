//! Watching Jira for activity on your issues and raising a desktop notification for each piece.

mod startup;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration as Span, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

use crate::adf;
use crate::api::activity::{self, Comment, History};
use crate::api::{issues, users};
use crate::cli::{NotifyCommand, NotifyWatchArgs, Reason};
use crate::context::Ctx;
use crate::notify::{self, Note};

/// The issues watched without `--jql`: the ones you are on.
pub const DEFAULT_JQL: &str =
    "assignee = currentUser() OR reporter = currentUser() OR watcher = currentUser()";

/// Each poll looks this far behind the last one, since Jira indexes updates with some lag.
const OVERLAP_MINUTES: i64 = 2;

/// Tokens this close to expiring are refreshed before a poll, so a watch outlives them.
const REFRESH_MARGIN_SECS: i64 = 120;

/// More events than this in one poll make a single summary notification.
const MAX_NOTES: usize = 3;

/// Issues read in one poll at most.
const MAX_ISSUES: usize = 100;

/// Characters of a comment shown in a notification.
const SNIPPET: usize = 140;

/// `ctx` connects to Jira, so it is built only for the commands that need it.
pub fn run(ctx: impl FnOnce() -> Result<Ctx>, cmd: NotifyCommand) -> Result<()> {
    match cmd {
        NotifyCommand::Watch(args) if args.install => startup::install(&ctx()?, &args),
        NotifyCommand::Watch(args) if args.uninstall => {
            if !startup::uninstall()? {
                println!("No watch was set to start at login");
            }
            Ok(())
        }
        NotifyCommand::Watch(args) => watch(ctx()?, &args),
    }
}

/// One thing that happened on an issue.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    pub reason: Reason,
    pub key: String,
    pub summary: String,
    pub by: String,
    /// The status change, or the comment's first line. Empty for an assignment.
    pub detail: String,
    pub url: String,
    pub at: DateTime<Utc>,
}

/// What the watcher knows between polls.
struct Watch {
    me: String,
    /// Activity after this is new. None until the first poll sets the baseline.
    since: Option<DateTime<Utc>>,
    /// Activity already reported, with when it happened, so overlapping polls stay quiet.
    seen: HashMap<String, DateTime<Utc>>,
    polled: bool,
}

impl Watch {
    /// Whether `id` is new, remembering it either way.
    fn first_time(&mut self, id: String, at: DateTime<Utc>) -> bool {
        self.seen.insert(id, at).is_none()
    }

    /// Forget activity too old for any poll to find again.
    fn prune(&mut self, now: DateTime<Utc>) {
        let keep_after = now - Span::hours(1);
        self.seen.retain(|_, at| *at > keep_after);
    }
}

/// A span such as 45s, 30m, 2h, or 1d.
pub fn parse_span(text: &str) -> Result<Span> {
    let text = text.trim();
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let n: i64 = number
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .with_context(|| format!("{text} is not a span such as 30m, 2h, or 1d"))?;
    match unit {
        "s" => Ok(Span::seconds(n)),
        "m" => Ok(Span::minutes(n)),
        "h" => Ok(Span::hours(n)),
        "d" => Ok(Span::days(n)),
        _ => bail!("{text} is not a span such as 30m, 2h, or 1d"),
    }
}

fn watch(mut ctx: Ctx, args: &NotifyWatchArgs) -> Result<()> {
    if let Some(jql) = &args.jql
        && jql.to_lowercase().contains("order by")
    {
        bail!("--jql takes a filter without ORDER BY, since the watch orders by update time");
    }
    let me = users::myself(&ctx.client)?["accountId"]
        .as_str()
        .context("Could not read your account ID")?
        .to_string();
    let since = args
        .since
        .as_deref()
        .map(parse_span)
        .transpose()?
        .map(|span| Utc::now() - span);
    let mut state = Watch {
        me,
        since,
        seen: HashMap::new(),
        polled: false,
    };
    loop {
        let expiring = ctx
            .creds
            .expiry
            .is_some_and(|e| e <= Utc::now() + Span::seconds(REFRESH_MARGIN_SECS));
        if expiring && let Err(e) = ctx.refresh_login() {
            eprintln!("warning: {e:#}");
        }
        let first = !state.polled;
        match poll_once(&ctx, args, &mut state) {
            Ok(events) => announce(&ctx, &events)?,
            // A bad login or JQL shows on the first poll; later failures are likely transient.
            Err(e) if first => return Err(e),
            Err(e) => eprintln!("warning: {e:#}"),
        }
        if args.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(args.interval));
    }
}

/// Poll once and return what happened since the last poll, oldest first. The first poll only sets
/// the baseline, unless `--since` asked for history.
fn poll_once(ctx: &Ctx, args: &NotifyWatchArgs, state: &mut Watch) -> Result<Vec<Event>> {
    let now = Utc::now();
    let Some(since) = state.since else {
        state.since = Some(now);
        state.polled = true;
        return Ok(Vec::new());
    };
    // JQL takes relative times, which need no time zone, so the window is rounded up to minutes.
    let minutes = ((now - since).num_seconds() + 59) / 60 + OVERLAP_MINUTES;
    let scope = args.jql.as_deref().unwrap_or(DEFAULT_JQL);
    let jql = format!("({scope}) AND updated >= -{minutes}m ORDER BY updated ASC");
    let fields = ["summary".to_string()];
    let updated = issues::search(&ctx.client, &jql, &fields, Some(MAX_ISSUES))?;

    let mut events = Vec::new();
    for issue in &updated {
        let key = issue["key"].as_str().unwrap_or("").to_string();
        let summary = issue["fields"]["summary"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let url = ctx.browse_url(&key);
        for history in activity::changelog_since(&ctx.client, &key, since)? {
            if history.by.id == state.me
                || !state.first_time(format!("{key}/h{}", history.id), history.at)
            {
                continue;
            }
            events.extend(history_events(&history, &key, &summary, &url, &state.me));
        }
        for comment in activity::comments_since(&ctx.client, &key, since)? {
            if comment.by.id == state.me
                || !state.first_time(format!("{key}/c{}", comment.id), comment.at)
            {
                continue;
            }
            events.push(comment_event(&comment, &key, &summary, &url, &state.me));
        }
    }
    events.retain(|e| args.reason.is_empty() || args.reason.contains(&e.reason));
    events.sort_by_key(|e| e.at);
    state.since = Some(now - Span::minutes(OVERLAP_MINUTES));
    state.prune(now);
    state.polled = true;
    Ok(events)
}

/// The assignment to `me` and the status change in one changelog entry, if any.
fn history_events(history: &History, key: &str, summary: &str, url: &str, me: &str) -> Vec<Event> {
    let event = |reason, detail: String| Event {
        reason,
        key: key.to_string(),
        summary: summary.to_string(),
        by: history.by.name.clone(),
        detail,
        url: url.to_string(),
        at: history.at,
    };
    history
        .items
        .iter()
        .filter_map(|item| match item["field"].as_str() {
            Some("assignee") if item["to"] == me => Some(event(Reason::Assigned, String::new())),
            Some("status") => Some(event(
                Reason::Status,
                format!(
                    "{} → {}",
                    item["fromString"].as_str().unwrap_or("?"),
                    item["toString"].as_str().unwrap_or("?")
                ),
            )),
            _ => None,
        })
        .collect()
}

/// A comment as a mention when it names `me`, otherwise as a comment.
fn comment_event(comment: &Comment, key: &str, summary: &str, url: &str, me: &str) -> Event {
    let reason = if activity::mentions(&comment.body, me) {
        Reason::Mention
    } else {
        Reason::Comment
    };
    Event {
        reason,
        key: key.to_string(),
        summary: summary.to_string(),
        by: comment.by.name.clone(),
        detail: snippet(&adf::field_markdown(&comment.body)),
        url: format!("{url}?focusedCommentId={}", comment.id),
        at: comment.at,
    }
}

/// The first line of `text`, cut to fit a notification.
fn snippet(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= SNIPPET {
        return line.to_string();
    }
    let cut: String = line.chars().take(SNIPPET - 1).collect();
    format!("{}…", cut.trim_end())
}

pub fn reason_label(reason: Reason) -> &'static str {
    match reason {
        Reason::Assigned => "assigned",
        Reason::Status => "status",
        Reason::Comment => "comment",
        Reason::Mention => "mention",
    }
}

/// Print a line per event and show its notification, or one summary when there are many.
fn announce(ctx: &Ctx, events: &[Event]) -> Result<()> {
    for event in events {
        if ctx.json {
            println!("{}", serde_json::to_string(event)?);
        } else {
            println!("{}", line(event));
        }
    }
    let your_work = format!("https://{}/jira/your-work", ctx.creds.site);
    for note in notes(events, &your_work) {
        notify::send(&note);
    }
    Ok(())
}

fn line(event: &Event) -> String {
    let what = match event.reason {
        Reason::Assigned => event.summary.clone(),
        Reason::Status => event.detail.clone(),
        Reason::Comment | Reason::Mention if event.detail.is_empty() => event.summary.clone(),
        Reason::Comment | Reason::Mention => event.detail.clone(),
    };
    format!(
        "{:<9} {}  {}  by {}  {}",
        reason_label(event.reason),
        event.key,
        what,
        event.by,
        event.url
    )
}

fn notes(events: &[Event], your_work: &str) -> Vec<Note> {
    if events.len() <= MAX_NOTES {
        return events.iter().map(note).collect();
    }
    let titles: Vec<String> = events
        .iter()
        .rev()
        .take(MAX_NOTES)
        .map(|e| note(e).title)
        .collect();
    let more = events.len() - titles.len();
    vec![Note::new(
        format!("{} new Jira updates", events.len()),
        format!("{}\nand {more} more", titles.join("\n")),
        Some(your_work.to_string()),
    )]
}

fn note(event: &Event) -> Note {
    let (title, body, subtitle) = match event.reason {
        Reason::Assigned => (
            format!("{} assigned to you", event.key),
            event.summary.clone(),
            event.by.clone(),
        ),
        Reason::Status => (
            format!("{}: {}", event.key, event.detail),
            event.summary.clone(),
            event.by.clone(),
        ),
        Reason::Comment => (
            format!("{}: comment from {}", event.key, event.by),
            event.detail.clone(),
            event.summary.clone(),
        ),
        Reason::Mention => (
            format!("{}: {} mentioned you", event.key, event.by),
            event.detail.clone(),
            event.summary.clone(),
        ),
    };
    Note {
        subtitle: Some(subtitle),
        // A newer note about the same issue replaces the older one.
        group: Some(format!("jira-arriba:{}", event.key)),
        ..Note::new(title, body, Some(event.url.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Client;
    use crate::credentials::{Auth, Credentials, Source};
    use mockito::{Matcher, Server};
    use serde_json::{Value, json};

    fn ctx(server: &Server, json: bool) -> Ctx {
        Ctx {
            client: Client::for_tests(&server.url()),
            creds: Credentials {
                auth: Auth::Bearer("t".into()),
                api: server.url(),
                site: "x.atlassian.net".into(),
                cloud_id: None,
                email: None,
                expiry: None,
                source: Source::Env(vec![]),
            },
            site: None,
            json,
            yes: true,
        }
    }

    fn args() -> NotifyWatchArgs {
        NotifyWatchArgs {
            interval: 60,
            reason: Vec::new(),
            jql: None,
            since: None,
            once: false,
            install: false,
            uninstall: false,
        }
    }

    fn state(me: &str, minutes_ago: Option<i64>) -> Watch {
        Watch {
            me: me.into(),
            since: minutes_ago.map(|m| Utc::now() - Span::minutes(m)),
            seen: HashMap::new(),
            polled: false,
        }
    }

    fn stamp(minutes_ago: i64) -> String {
        (Utc::now() - Span::minutes(minutes_ago)).to_rfc3339()
    }

    fn person(id: &str, name: &str) -> Value {
        json!({"accountId": id, "displayName": name})
    }

    fn text(t: &str) -> Value {
        json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": t}]}
        ]})
    }

    /// An issue with an assignment and a status change by Ann, a change by me, and comments.
    fn mock_activity(server: &mut Server) {
        server
            .mock("POST", "/search/jql")
            .match_body(Matcher::PartialJson(json!({"fields": ["summary"]})))
            .with_body(
                json!({"issues": [{"key": "A-1", "fields": {"summary": "Crash"}}], "isLast": true})
                    .to_string(),
            )
            .create();
        server
            .mock("GET", "/issue/A-1/changelog")
            .match_query(Matcher::UrlEncoded("maxResults".into(), "1".into()))
            .with_body(r#"{"total": 2}"#)
            .create();
        server
            .mock("GET", "/issue/A-1/changelog")
            .match_query(Matcher::UrlEncoded("maxResults".into(), "50".into()))
            .with_body(
                json!({"values": [
                    {"id": "h1", "created": stamp(5), "author": person("ann", "Ann"), "items": [
                        {"field": "assignee", "to": "me", "toString": "Me"},
                        {"field": "status", "fromString": "Open", "toString": "In Progress"},
                        {"field": "description"}
                    ]},
                    {"id": "h2", "created": stamp(4), "author": person("me", "Me"), "items": [
                        {"field": "status", "fromString": "In Progress", "toString": "Done"}
                    ]}
                ]})
                .to_string(),
            )
            .create();
        server
            .mock("GET", "/issue/A-1/comment")
            .match_query(Matcher::Any)
            .with_body(
                json!({"comments": [
                    {"id": "c3", "created": stamp(1), "author": person("ann", "Ann"), "body": {"type": "doc", "content": [
                        {"type": "paragraph", "content": [
                            {"type": "mention", "attrs": {"id": "me", "text": "@Me"}},
                            {"type": "text", "text": " can you look?"}
                        ]}
                    ]}},
                    {"id": "c2", "created": stamp(3), "author": person("bob", "Bob"), "body": text("Me too.")},
                    {"id": "c1", "created": stamp(2), "author": person("me", "Me"), "body": text("Mine.")}
                ]})
                .to_string(),
            )
            .create();
    }

    #[test]
    fn first_poll_sets_the_baseline_without_asking_jira() {
        let server = Server::new();
        let mut state = state("me", None);
        assert!(
            poll_once(&ctx(&server, false), &args(), &mut state)
                .unwrap()
                .is_empty()
        );
        assert!(state.since.is_some());
        assert!(state.polled);
    }

    #[test]
    fn polls_report_others_activity_once_in_order() {
        let mut server = Server::new();
        mock_activity(&mut server);
        let ctx = ctx(&server, false);
        let mut state = state("me", Some(10));
        let events = poll_once(&ctx, &args(), &mut state).unwrap();
        let summary: Vec<(Reason, &str, &str)> = events
            .iter()
            .map(|e| (e.reason, e.by.as_str(), e.detail.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (Reason::Assigned, "Ann", ""),
                (Reason::Status, "Ann", "Open → In Progress"),
                (Reason::Comment, "Bob", "Me too."),
                (Reason::Mention, "Ann", "@Me can you look?"),
            ]
        );
        assert_eq!(events[0].url, "https://x.atlassian.net/browse/A-1");
        assert_eq!(
            events[3].url,
            "https://x.atlassian.net/browse/A-1?focusedCommentId=c3"
        );
        assert_eq!(events[0].summary, "Crash");
        // The same activity on the next poll is already seen.
        assert!(poll_once(&ctx, &args(), &mut state).unwrap().is_empty());
    }

    #[test]
    fn reasons_filter_the_events() {
        let mut server = Server::new();
        mock_activity(&mut server);
        let filtered = NotifyWatchArgs {
            reason: vec![Reason::Mention, Reason::Assigned],
            ..args()
        };
        let events =
            poll_once(&ctx(&server, false), &filtered, &mut state("me", Some(10))).unwrap();
        let reasons: Vec<Reason> = events.iter().map(|e| e.reason).collect();
        assert_eq!(reasons, [Reason::Assigned, Reason::Mention]);
    }

    #[test]
    fn announce_prints_lines_or_json_and_sends_notes() {
        let mut server = Server::new();
        mock_activity(&mut server);
        let events = poll_once(&ctx(&server, false), &args(), &mut state("me", Some(10))).unwrap();
        announce(&ctx(&server, false), &events).unwrap();
        // Four events make one summary note.
        let sent = notify::sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].title, "4 new Jira updates");
        assert_eq!(
            sent[0].body,
            "A-1: Ann mentioned you\nA-1: comment from Bob\nA-1: Open → In Progress\nand 1 more"
        );
        assert_eq!(
            sent[0].url.as_deref(),
            Some("https://x.atlassian.net/jira/your-work")
        );

        announce(&ctx(&server, true), &events[..1]).unwrap();
        let sent = notify::sent();
        assert_eq!(sent[0].title, "A-1 assigned to you");
        assert_eq!(sent[0].body, "Crash");
        assert_eq!(sent[0].subtitle.as_deref(), Some("Ann"));
        assert_eq!(sent[0].group.as_deref(), Some("jira-arriba:A-1"));
        assert_eq!(
            line(&events[1]),
            "status    A-1  Open → In Progress  by Ann  https://x.atlassian.net/browse/A-1"
        );
        assert_eq!(
            line(&events[0]),
            "assigned  A-1  Crash  by Ann  https://x.atlassian.net/browse/A-1"
        );
    }

    #[test]
    fn spans_parse_and_snippets_cut() {
        assert_eq!(parse_span("45s").unwrap(), Span::seconds(45));
        assert_eq!(parse_span(" 30m ").unwrap(), Span::minutes(30));
        assert_eq!(parse_span("2h").unwrap(), Span::hours(2));
        assert_eq!(parse_span("1d").unwrap(), Span::days(1));
        for bad in ["", "m", "0h", "5x", "abc"] {
            assert!(parse_span(bad).is_err(), "{bad}");
        }
        assert_eq!(snippet("\n\n  first line  \nsecond"), "first line");
        let long = "x".repeat(200);
        let cut = snippet(&long);
        assert_eq!(cut.chars().count(), SNIPPET);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn seen_entries_are_pruned_after_an_hour() {
        let mut state = state("me", None);
        let now = Utc::now();
        assert!(state.first_time("old".into(), now - Span::hours(2)));
        assert!(state.first_time("new".into(), now));
        assert!(!state.first_time("new".into(), now));
        state.prune(now);
        assert!(state.first_time("old".into(), now));
    }
}
