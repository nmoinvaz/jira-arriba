use anyhow::{Context, Result, bail};
use dialoguer::Confirm;
use std::io::{IsTerminal, Read};
use std::path::Path;

/// True when a person can answer prompts. Prompts render on stderr so stdout stays clean.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Ask a yes/no question. `yes` answers it up front, and without a terminal it is required.
pub fn confirm(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !interactive() {
        bail!("{prompt} Pass --yes to confirm without a prompt.");
    }
    Ok(Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()?)
}

/// Text from the argument, or from stdin for `-` or a piped stdin. `what` names it in errors.
pub fn read_body(arg: Option<&str>, what: &str) -> Result<String> {
    let body = match arg {
        Some("-") => read_stdin()?,
        Some(text) => text.to_string(),
        None if std::io::stdin().is_terminal() => {
            bail!("Pass the {what} as an argument, or pipe it on stdin")
        }
        None => read_stdin()?,
    };
    let body = body.trim_end_matches(['\n', '\r']).to_string();
    if body.trim().is_empty() {
        bail!("The {what} is empty");
    }
    Ok(body)
}

/// Text from `file` when given, otherwise from the argument or stdin as [`read_body`] does.
pub fn read_body_or_file(arg: Option<&str>, file: Option<&Path>, what: &str) -> Result<String> {
    match file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            if text.trim().is_empty() {
                bail!("The {what} in {} is empty", path.display());
            }
            Ok(text)
        }
        None => read_body(arg, what),
    }
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .context("Failed to read stdin")?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yes_confirms_without_asking() {
        assert!(confirm("Delete it?", true).unwrap());
    }

    #[test]
    fn body_argument_loses_trailing_newlines_only() {
        assert_eq!(
            read_body(Some("  hi\nthere\r\n\n"), "body").unwrap(),
            "  hi\nthere"
        );
    }

    #[test]
    fn blank_body_argument_is_rejected() {
        let err = read_body(Some(" \n"), "comment").unwrap_err();
        assert_eq!(err.to_string(), "The comment is empty");
    }

    #[test]
    fn file_wins_over_argument() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("body.md");
        std::fs::write(&path, "# From file\n").unwrap();
        let text = read_body_or_file(Some("arg"), Some(&path), "comment").unwrap();
        assert_eq!(text, "# From file\n");
        std::fs::write(&path, "\n").unwrap();
        assert!(read_body_or_file(Some("arg"), Some(&path), "comment").is_err());
    }
}
