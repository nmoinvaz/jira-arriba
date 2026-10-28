# jira-arriba

[![Build](https://github.com/nmoinvaz/jira-arriba/actions/workflows/build.yml/badge.svg)](https://github.com/nmoinvaz/jira-arriba/actions/workflows/build.yml)
[![Lint](https://github.com/nmoinvaz/jira-arriba/actions/workflows/lint.yml/badge.svg)](https://github.com/nmoinvaz/jira-arriba/actions/workflows/lint.yml)
[![codecov](https://codecov.io/gh/nmoinvaz/jira-arriba/graph/badge.svg)](https://codecov.io/gh/nmoinvaz/jira-arriba)

A Jira CLI for the chores [acli](https://developer.atlassian.com/cloud/acli/) doesn't cover:
downloading attachments, writing comments and descriptions in Markdown, reading them back as
Markdown, and JQL searches that page. It reuses acli's login, so there is nothing new to set up.

Every command takes `--json`, and every prompt has a flag that answers it, so scripts and coding
agents can drive the same workflows a person runs by hand.

## Install

Download a binary from the [releases](https://github.com/nmoinvaz/jira-arriba/releases) and put
it on your `PATH`, or build it from source:

```
cargo install --git https://github.com/nmoinvaz/jira-arriba
```

## Use with Claude

Add this line to `~/.claude/CLAUDE.md` so Claude Code reaches for `jira-arriba` when a task
touches Jira:

```
* Use `jira-arriba` when interacting with Jira, for issues, JQL search, transitions, attachments, and comments or descriptions written in Markdown. Run `jira-arriba --help` for the commands.
```

## Authentication

The login comes from the environment when it is set, and from acli otherwise.

| Variables | Login |
|-----------|-------|
| `JIRA_API_TOKEN`, `JIRA_EMAIL`, `JIRA_SITE` | An [API token](https://id.atlassian.com/manage-profile/security/api-tokens), sent as basic auth to the site |
| `JIRA_ACCESS_TOKEN`, `JIRA_CLOUD_ID` | An OAuth token, sent through the Atlassian gateway; `JIRA_SITE` can stand in for the cloud ID |
| none | acli's current login, read from `~/.config/acli` and the platform keyring |

With acli, `jira-arriba auth login` opens the browser through `acli jira auth login --web`, offering
to install acli with Homebrew when it is missing, and an expired OAuth token is refreshed by running
`acli jira auth status`. Pass `--site` to pick one of
several acli logins. Reading the keyring uses `security` on macOS, `secret-tool` on Linux, and the
Windows credential store.

```
jira-arriba auth status
jira-arriba auth login
jira-arriba auth token --header     # Authorization: Bearer ..., for curl -H
```

## Usage

```
jira-arriba [options] <command>
```

Wherever a command takes an issue `KEY`, it also takes the issue's URL.

### Issues

| Command | Description |
|---------|-------------|
| `issue view <KEY>` | Show an issue with its description, comments, and attachments, rich text as Markdown |
| `issue search --jql <JQL>` | Search issues, `--limit` or `--all`, with `--fields` to pick columns |
| `issue create` | Create an issue from `--project`, `--type`, `--summary`, and a Markdown `--description` |
| `issue edit <KEY>` | Change the summary, description, type, assignee, or labels |
| `issue transition <KEY> --status <NAME>` | Move an issue, or `--list` where it can go |
| `issue comment list <KEY>` | Print the comments as Markdown |
| `issue comment add <KEY> [BODY]` | Add a comment, from the argument, `--file`, or stdin |
| `issue comment edit <KEY> <ID> [BODY]` | Replace a comment |
| `issue comment delete <KEY> <ID>` | Delete a comment, after confirming |

`issue create` and `issue edit` also take `--from-json` with a file in the layout acli's
`--generate-json` prints, and flags override its fields.

### Attachments

| Command | Description |
|---------|-------------|
| `attachment list <KEY>` | List attachments, including ones the project's field configuration hides |
| `attachment download <KEY>` | Download them all to a folder, extracting zip files, `--name '*.log'` to pick some |
| `attachment upload <KEY> <FILE>...` | Attach files |
| `attachment delete <ID>...` | Delete attachments, after confirming |

Attachments are found in the issue's attachment field, in its changelog, and in the inline images
and links of its description and comments, so files still download when the Attachment field is
hidden or when they were only ever pasted into a comment. Downloads go to a `jira-arriba/KEY` folder
in the temp directory unless `-o` names one.

### Projects and ADF

| Command | Description |
|---------|-------------|
| `project list` | List projects, `--query` to filter by key or name |
| `adf from-md [FILE]` | Print Markdown as an ADF document |
| `adf to-md [FILE]` | Print an ADF document, or an issue or comment holding one, as Markdown |

### Notifications

| Command | Description |
|---------|-------------|
| `notify watch` | Poll your issues and show a desktop notification for each assignment, status change, comment, or mention, until stopped |
| `notify watch --install` | Start the watch with the options given, now and at every login, on macOS and Linux |
| `notify watch --uninstall` | Stop the watch that starts at login, and stop starting it |

Jira has no notification feed in its API, so the watch polls JQL once a minute for issues you are
assigned, reported, or watch that changed since the last poll, then reads each one's newest changelog
entries and comments. Your own changes stay quiet. Pass `--reason mention` or any of `assigned`,
`status`, and `comment` to hear about some kinds only, `--jql` to watch a different set of issues,
and `--since 2h` to replay recent activity at start. `--once` polls one time and exits.

Notifications use `terminal-notifier` or `osascript` on macOS, `notify-send` on Linux, and a toast on
Windows, and clicking one opens the issue where the notifier supports it. `--install` registers a
LaunchAgent on macOS, logging to `~/Library/Logs/jira-arriba-notify-watch.log`, or a systemd user
service on Linux, logging to the journal.

Notifications carry the Jira icon. macOS takes a notification's icon from the app that sends it, so
the first notification builds a copy of terminal-notifier named jira-arriba under
`~/Library/Application Support/jira-arriba`, and macOS asks once to allow its notifications. Until
they are allowed, the notes come through terminal-notifier itself and the watch says where to allow
them. Linux and Windows read the icon from a PNG kept under `~/.local/share/jira-arriba` or
`%LOCALAPPDATA%\jira-arriba`. The icon comes from Atlassian's `@atlaskit/logo` package, and Jira is
a trademark of Atlassian.

### Options

| Option | Description |
|--------|-------------|
| `--site <SITE>` | Jira site, as in `mysite.atlassian.net`, when acli has several logins |
| `--json` | Print machine-readable JSON |
| `-y`, `--yes` | Skip confirmation prompts |
| `-h`, `--help` | Print help for any command |
| `-V`, `--version` | Print version information |

## Markdown and ADF

Jira stores rich text as Atlassian Document Format (ADF), and renders Markdown as literal
characters. Bodies given to `issue create`, `issue edit`, and `issue comment` are written in
Markdown and converted, and bodies that already are ADF JSON are sent as they are. Pass `--adf` to
insist on ADF.

| Markdown | ADF |
|----------|-----|
| Headings, paragraphs, hard breaks, rules | `heading`, `paragraph`, `hardBreak`, `rule` |
| `*em*`, `**strong**`, `~~strike~~`, `` `code` ``, links | marks |
| Fenced code with a language | `codeBlock` |
| Bullet, numbered, and `- [ ]` task lists, nested | `bulletList`, `orderedList`, `taskList` |
| Blockquotes | `blockquote` |
| `> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]` | `panel` |
| Tables | `table` |
| Images by URL | `mediaSingle` with external media |

Reading back, mentions, dates, statuses, emoji, and attached media render as text, and the
conversion round-trips for everything in the table.

## Examples

Read a bug report, its comments, and its attachments:

```
jira-arriba issue view PROJ-123
jira-arriba attachment download PROJ-123 -o /tmp/PROJ-123
```

Find related issues and leave a comment written in Markdown:

```
jira-arriba issue search --jql 'project = PROJ AND text ~ "crash on start" AND key != PROJ-123'
jira-arriba issue comment add PROJ-123 - <<'EOF'
Fixed in `a1b2c3d`.

- The window check ran before the buffer was sized.
- Added a regression test.
EOF
```

Create a bug with a description from a file, then move it along:

```
jira-arriba issue create -p PROJ -t Bug -s "Crash on start" --description-file bug.md
jira-arriba issue transition PROJ-124 --status "In Progress"
```

Call the REST API yourself with the same login:

```
curl -H "$(jira-arriba auth token --header)" "https://api.atlassian.com/ex/jira/$CLOUD_ID/rest/api/3/myself"
```

Hear about mentions and new assignments while you work, replaying the last hour first, then keep
that watch running from every login:

```
jira-arriba notify watch --reason mention --reason assigned --since 1h
jira-arriba notify watch --reason mention --reason assigned --install
```

## Development

```
cargo build --release
cargo test
```

## License

MIT
