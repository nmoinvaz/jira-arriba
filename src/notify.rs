//! Native desktop notifications through the platform's own notifier.

mod icon;

use std::path::{Path, PathBuf};

use icon::Branding;

/// One desktop notification.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub title: String,
    pub body: String,
    /// Opened on click where the notifier supports it, otherwise appended to the body.
    pub url: Option<String>,
    /// Shown under the title where the notifier supports it.
    pub subtitle: Option<String>,
    /// Replaces the last note of the same group where the notifier supports it.
    pub group: Option<String>,
}

impl Note {
    pub fn new(title: impl Into<String>, body: impl Into<String>, url: Option<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            url,
            ..Self::default()
        }
    }

    /// The body with the URL on its own line, for notifiers that can't open links.
    fn body_with_url(&self) -> String {
        match &self.url {
            Some(url) if self.body.is_empty() => url.clone(),
            Some(url) => format!("{}\n{url}", self.body),
            None => self.body.clone(),
        }
    }
}

/// A notifier command: program, arguments, and environment variables.
#[derive(Debug, PartialEq)]
struct Invocation {
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(&'static str, String)>,
}

/// How long notify-send asks to show a banner, six seconds past the five most desktops use.
const LINUX_EXPIRE_MS: u32 = 11_000;

/// Shows a toast through the WinRT API. Title, body, and icon come from the environment so they
/// need no quoting. The icon takes the image slot of the template that has one, and the long
/// duration keeps the toast up for 25 seconds rather than 7.
const WINDOWS_TOAST: &str = "\
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
$type = if ($env:ARRIBA_NOTE_ICON) { 'ToastImageAndText02' } else { 'ToastText02' }
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]$type)
$text = $template.GetElementsByTagName('text')
$text.Item(0).AppendChild($template.CreateTextNode($env:ARRIBA_NOTE_TITLE)) | Out-Null
$text.Item(1).AppendChild($template.CreateTextNode($env:ARRIBA_NOTE_BODY)) | Out-Null
if ($env:ARRIBA_NOTE_ICON) { $template.GetElementsByTagName('image').Item(0).SetAttribute('src', $env:ARRIBA_NOTE_ICON) }
$template.DocumentElement.SetAttribute('duration', 'long')
$toast = [Windows.UI.Notifications.ToastNotification]::new($template)
$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show($toast)";

/// The command that shows `note` on `os`, preferring a notifier `has` reports as installed, with
/// the icon where `branding` keeps it.
fn invocation(
    os: &str,
    note: &Note,
    has: impl Fn(&str) -> bool,
    branding: &Branding,
) -> Option<Invocation> {
    match os {
        "macos" if branding.notifier.is_some() || has("terminal-notifier") => {
            let mut args = vec![
                "-title".into(),
                note.title.clone(),
                "-message".into(),
                note.body.clone(),
                "-group".into(),
                note.group.clone().unwrap_or_else(|| "jira-arriba".into()),
            ];
            if let Some(subtitle) = &note.subtitle {
                args.extend(["-subtitle".into(), subtitle.clone()]);
            }
            if let Some(url) = &note.url {
                args.extend(["-open".into(), url.clone()]);
            }
            Some(Invocation {
                program: branding
                    .notifier
                    .clone()
                    .unwrap_or_else(|| "terminal-notifier".into()),
                args,
                env: Vec::new(),
            })
        }
        // Passing the text as script arguments avoids escaping it into AppleScript.
        "macos" => Some(Invocation {
            program: "osascript".into(),
            args: vec![
                "-e".into(),
                "on run argv".into(),
                "-e".into(),
                "display notification (item 2 of argv) with title (item 1 of argv)".into(),
                "-e".into(),
                "end run".into(),
                note.title.clone(),
                note.body_with_url(),
            ],
            env: Vec::new(),
        }),
        "windows" => {
            let mut env = vec![
                ("ARRIBA_NOTE_TITLE", note.title.clone()),
                ("ARRIBA_NOTE_BODY", note.body_with_url()),
            ];
            if let Some(png) = &branding.png {
                env.push(("ARRIBA_NOTE_ICON", file_url(png)));
            }
            Some(Invocation {
                program: "powershell.exe".into(),
                args: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    WINDOWS_TOAST.into(),
                ],
                env,
            })
        }
        _ if has("notify-send") => {
            let mut args = vec![
                "--app-name=jira-arriba".to_string(),
                format!("--expire-time={LINUX_EXPIRE_MS}"),
            ];
            if let Some(png) = &branding.png {
                args.push(format!("--icon={}", png.display()));
            }
            // A title or body starting with a dash would otherwise parse as an option.
            args.extend(["--".into(), note.title.clone(), note.body_with_url()]);
            Some(Invocation {
                program: "notify-send".into(),
                args,
                env: Vec::new(),
            })
        }
        _ => None,
    }
}

/// A toast takes its image as a URL.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    format!("file:///{}", path.trim_start_matches('/'))
}

/// Where `program` is on the PATH.
#[cfg_attr(test, allow(dead_code))]
fn find(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let file = dir.join(program);
        if file.is_file() {
            return Some(file);
        }
        let exe = file.with_extension("exe");
        (cfg!(windows) && exe.is_file()).then_some(exe)
    })
}

#[cfg_attr(test, allow(dead_code))]
fn on_path(program: &str) -> bool {
    find(program).is_some()
}

/// Why a notifier didn't show a note.
#[cfg_attr(test, allow(dead_code))]
struct Failure {
    program: PathBuf,
    code: Option<i32>,
    detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let program = self.program.display();
        match (self.code, self.detail.as_str()) {
            (_, detail) if !detail.is_empty() => write!(f, "{program}: {detail}"),
            (Some(code), _) => write!(f, "{program} exited with {code}"),
            (None, _) => write!(f, "{program} was killed"),
        }
    }
}

/// terminal-notifier's exit code when notifications for its bundle aren't allowed.
#[cfg(not(test))]
const NOT_AUTHORIZED: i32 = 3;

#[cfg(not(test))]
fn execute(inv: &Invocation) -> Result<(), Failure> {
    use std::process::{Command, Stdio};
    let output = Command::new(&inv.program)
        .args(&inv.args)
        .envs(inv.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(|e| Failure {
            program: inv.program.clone(),
            code: None,
            detail: e.to_string(),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Failure {
        program: inv.program.clone(),
        code: output.status.code(),
        detail: stderr.lines().next().unwrap_or_default().trim().to_string(),
    })
}

#[cfg(not(test))]
fn show(note: &Note) -> Result<(), Failure> {
    use std::sync::Once;
    static HINTED: Once = Once::new();
    let os = std::env::consts::OS;
    let branding = icon::branding();
    let Some(inv) = invocation(os, note, on_path, branding) else {
        return Err(Failure {
            program: "notifier".into(),
            code: None,
            detail: "No desktop notifier found (install notify-send or terminal-notifier)".into(),
        });
    };
    match execute(&inv) {
        // macOS refuses the branded copy until its notifications are allowed. The stock notifier
        // shows the note meanwhile.
        Err(failure) if failure.code == Some(NOT_AUTHORIZED) && branding.notifier.is_some() => {
            HINTED.call_once(|| {
                eprintln!(
                    "Allow jira-arriba under System Settings > Notifications to see the Jira icon ({failure})"
                );
            });
            let Some(stock) = invocation(os, note, on_path, &Branding::default()) else {
                return Err(failure);
            };
            execute(&stock)
        }
        result => result,
    }
}

#[cfg(test)]
thread_local! {
    static SENT: std::cell::RefCell<Vec<Note>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests record notes instead of showing them.
#[cfg(test)]
fn show(note: &Note) -> Result<(), Failure> {
    SENT.with(|s| s.borrow_mut().push(note.clone()));
    Ok(())
}

/// The notes this test thread has sent, clearing them.
#[cfg(test)]
pub fn sent() -> Vec<Note> {
    SENT.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// Show `note`, or when the notifier can't, ring the terminal bell and print it to stderr.
pub fn send(note: &Note) {
    if let Err(failure) = show(note) {
        eprintln!(
            "\x07Notification not shown ({failure}): {}: {}",
            note.title,
            note.body_with_url()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(url: Option<&str>) -> Note {
        Note::new(
            "D4T-1 assigned to you",
            "Crash on start",
            url.map(String::from),
        )
    }

    fn plain() -> Branding {
        Branding::default()
    }

    fn branded() -> Branding {
        Branding {
            notifier: Some("/apps/jira-arriba.app/Contents/MacOS/terminal-notifier".into()),
            png: Some("/data/jira.png".into()),
        }
    }

    #[test]
    fn macos_prefers_terminal_notifier_with_a_click_url() {
        let inv = invocation(
            "macos",
            &note(Some("https://x/1")),
            |p| p == "terminal-notifier",
            &plain(),
        )
        .unwrap();
        assert_eq!(inv.program, Path::new("terminal-notifier"));
        assert!(inv.args.ends_with(&["-open".into(), "https://x/1".into()]));
        assert_eq!(inv.args[4..6], ["-group", "jira-arriba"]);

        let styled = Note {
            subtitle: Some("Ann".into()),
            group: Some("jira-arriba:D4T-1".into()),
            ..note(None)
        };
        let inv = invocation("macos", &styled, |p| p == "terminal-notifier", &plain()).unwrap();
        assert_eq!(
            inv.args[4..],
            ["-group", "jira-arriba:D4T-1", "-subtitle", "Ann"]
        );
    }

    #[test]
    fn macos_runs_the_branded_copy_without_needing_the_path() {
        let inv = invocation("macos", &note(None), |_| false, &branded()).unwrap();
        assert_eq!(
            inv.program,
            Path::new("/apps/jira-arriba.app/Contents/MacOS/terminal-notifier")
        );
        assert_eq!(inv.args[..2], ["-title", "D4T-1 assigned to you"]);
    }

    #[test]
    fn macos_falls_back_to_osascript_with_text_as_arguments() {
        let inv = invocation("macos", &note(Some("https://x/1")), |_| false, &plain()).unwrap();
        assert_eq!(inv.program, Path::new("osascript"));
        assert_eq!(
            inv.args[inv.args.len() - 2..],
            [
                "D4T-1 assigned to you".to_string(),
                "Crash on start\nhttps://x/1".to_string()
            ]
        );
    }

    #[test]
    fn linux_uses_notify_send_when_installed() {
        let inv = invocation("linux", &note(None), |p| p == "notify-send", &plain()).unwrap();
        assert_eq!(inv.program, Path::new("notify-send"));
        assert_eq!(
            inv.args,
            [
                "--app-name=jira-arriba",
                "--expire-time=11000",
                "--",
                "D4T-1 assigned to you",
                "Crash on start"
            ]
        );
        let inv = invocation("linux", &note(None), |p| p == "notify-send", &branded()).unwrap();
        assert_eq!(inv.args[2], "--icon=/data/jira.png");
        assert_eq!(
            invocation("linux", &note(None), |_| false, &branded()),
            None
        );
    }

    #[test]
    fn windows_passes_text_and_icon_through_the_environment() {
        let inv = invocation("windows", &note(Some("https://x/1")), |_| false, &plain()).unwrap();
        assert_eq!(inv.program, Path::new("powershell.exe"));
        assert!(!inv.args.iter().any(|a| a.contains("Crash on start")));
        assert_eq!(
            inv.env,
            [
                ("ARRIBA_NOTE_TITLE", "D4T-1 assigned to you".to_string()),
                (
                    "ARRIBA_NOTE_BODY",
                    "Crash on start\nhttps://x/1".to_string()
                ),
            ]
        );
        let inv = invocation("windows", &note(None), |_| false, &branded()).unwrap();
        assert_eq!(
            inv.env[2],
            ("ARRIBA_NOTE_ICON", "file:///data/jira.png".into())
        );
        assert_eq!(
            file_url(Path::new(
                "C:\\Users\\me\\AppData\\Local\\jira-arriba\\jira.png"
            )),
            "file:///C:/Users/me/AppData/Local/jira-arriba/jira.png"
        );
    }

    #[test]
    fn records_notes_in_tests() {
        send(&note(None));
        assert_eq!(sent(), [note(None)]);
        assert!(sent().is_empty());
    }
}
