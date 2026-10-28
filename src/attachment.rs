use anyhow::{Context, Result, bail};
use glob::{MatchOptions, Pattern};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::api::attachments::{self, Attachment};
use crate::api::issues::parse_key;
use crate::cli::{AttachmentCommand, AttachmentDeleteArgs, DownloadArgs, KeyArg, UploadArgs};
use crate::context::Ctx;
use crate::output::{date, print_json, size, table};
use crate::prompt;

pub fn run(ctx: &Ctx, cmd: AttachmentCommand) -> Result<()> {
    match cmd {
        AttachmentCommand::List(args) => list(ctx, args),
        AttachmentCommand::Download(args) => download(ctx, args),
        AttachmentCommand::Upload(args) => upload(ctx, args),
        AttachmentCommand::Delete(args) => delete(ctx, args),
    }
}

fn list(ctx: &Ctx, args: KeyArg) -> Result<()> {
    let key = parse_key(&args.key)?;
    let found = attachments::discover(&ctx.client, &key)?;
    if ctx.json {
        return print_json(&found);
    }
    if found.is_empty() {
        println!("No attachments on {key}.");
        return Ok(());
    }
    println!("{}", table(&rows(&found), ""));
    Ok(())
}

fn rows(found: &[Attachment]) -> Vec<Vec<String>> {
    found
        .iter()
        .map(|a| {
            vec![
                a.id.clone(),
                a.size.map(size).unwrap_or_default(),
                a.created.as_deref().map(date).unwrap_or_default(),
                a.author.clone().unwrap_or_default(),
                a.filename.clone(),
            ]
        })
        .collect()
}

/// One downloaded file, for the summary.
#[derive(Serialize)]
struct Downloaded {
    id: String,
    name: String,
    path: PathBuf,
    size: u64,
    extracted_to: Option<PathBuf>,
}

fn download(ctx: &Ctx, args: DownloadArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    let dir = args
        .dir
        .unwrap_or_else(|| std::env::temp_dir().join("jira-arriba").join(&key));
    let found = attachments::discover(&ctx.client, &key)?;
    let patterns = patterns(&args.names)?;
    let wanted: Vec<&Attachment> = found
        .iter()
        .filter(|a| matches(&patterns, &a.filename))
        .collect();
    if wanted.is_empty() {
        if ctx.json {
            return print_json(&json!({ "dir": dir, "files": [] }));
        }
        if found.is_empty() {
            println!("No attachments or inline media found on {key}.");
        } else {
            println!("No attachments on {key} match --name.");
        }
        return Ok(());
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;

    let names = unique_names(&wanted);
    let mut files = Vec::new();
    for (attachment, name) in wanted.iter().zip(names) {
        let path = dir.join(&name);
        let bytes = attachments::download(&ctx.client, &attachment.id, &path)?;
        if !ctx.json {
            println!("  {name} ({})", size(bytes));
        }
        let mut extracted_to = None;
        if !args.no_extract && name.to_lowercase().ends_with(".zip") {
            let dest = dir.join(
                name.strip_suffix(".zip")
                    .or(name.strip_suffix(".ZIP"))
                    .unwrap_or(&name),
            );
            match extract_zip(&path, &dest) {
                Ok(_) => {
                    if !ctx.json {
                        println!(
                            "  extracted {name} -> {}/",
                            dest.file_name().unwrap_or_default().to_string_lossy()
                        );
                    }
                    extracted_to = Some(dest);
                }
                Err(e) => eprintln!("  could not extract {name}: {e:#}"),
            }
        }
        files.push(Downloaded {
            id: attachment.id.clone(),
            name,
            path,
            size: bytes,
            extracted_to,
        });
    }

    if ctx.json {
        return print_json(&json!({ "dir": dir, "files": files }));
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    let extracted = files.iter().filter(|f| f.extracted_to.is_some()).count();
    println!(
        "\nDownloaded {} file(s), {} total, to {}",
        files.len(),
        size(total),
        dir.display()
    );
    if extracted > 0 {
        println!("Extracted {extracted} zip archive(s)");
    }
    Ok(())
}

fn patterns(globs: &[String]) -> Result<Vec<Pattern>> {
    globs
        .iter()
        .map(|g| Pattern::new(g).with_context(|| format!("{g} is not a valid glob")))
        .collect()
}

/// Whether a filename matches any pattern, ignoring case, or there are no patterns.
fn matches(patterns: &[Pattern], name: &str) -> bool {
    let options = MatchOptions {
        case_sensitive: false,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    patterns.is_empty() || patterns.iter().any(|p| p.matches_with(name, options))
}

/// Safe, distinct file names for the attachments: no directories, and the ID added where two
/// share a name.
fn unique_names(found: &[&Attachment]) -> Vec<String> {
    let base: Vec<String> = found
        .iter()
        .map(|a| {
            Path::new(&a.filename)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty() && n != "." && n != "..")
                .unwrap_or_else(|| format!("attachment-{}", a.id))
        })
        .collect();
    base.iter()
        .enumerate()
        .map(|(i, name)| {
            let repeated = base.iter().filter(|n| *n == name).count() > 1;
            if !repeated {
                return name.clone();
            }
            let id = &found[i].id;
            match name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{id}.{ext}"),
                _ => format!("{name}-{id}"),
            }
        })
        .collect()
}

/// Extract a zip into `dest`, skipping entries that would escape it. Returns the files written.
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<usize> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    std::fs::create_dir_all(dest)?;
    let mut written = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut target = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut target)?;
        written += 1;
    }
    Ok(written)
}

fn upload(ctx: &Ctx, args: UploadArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    for file in &args.files {
        if !file.is_file() {
            bail!("{} is not a file", file.display());
        }
    }
    let mut uploaded = Vec::new();
    for file in &args.files {
        for attachment in attachments::upload(&ctx.client, &key, file)? {
            if !ctx.json {
                println!(
                    "  {} ({}) id {}",
                    attachment.filename,
                    attachment.size.map(size).unwrap_or_default(),
                    attachment.id
                );
            }
            uploaded.push(attachment);
        }
    }
    if ctx.json {
        return print_json(&uploaded);
    }
    println!("Attached {} file(s) to {key}", uploaded.len());
    Ok(())
}

fn delete(ctx: &Ctx, args: AttachmentDeleteArgs) -> Result<()> {
    let mut targets = Vec::new();
    for id in &args.ids {
        let meta = attachments::metadata(&ctx.client, id)?
            .with_context(|| format!("Attachment {id} does not exist"))?;
        let name = meta["filename"].as_str().unwrap_or("").to_string();
        targets.push((id.clone(), name));
    }
    let names: Vec<String> = targets
        .iter()
        .map(|(id, name)| format!("{id} {name}"))
        .collect();
    let question = format!(
        "Delete {} attachment(s): {}?",
        targets.len(),
        names.join(", ")
    );
    if !prompt::confirm(&question, ctx.yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    for (id, name) in &targets {
        attachments::delete(&ctx.client, id)?;
        if !ctx.json {
            println!("Deleted {id} {name}");
        }
    }
    if ctx.json {
        return print_json(&json!({ "deleted": args.ids }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn att(id: &str, name: &str) -> Attachment {
        Attachment {
            id: id.into(),
            filename: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn names_are_sanitized_and_made_distinct() {
        let a = att("1", "log.txt");
        let b = att("2", "log.txt");
        let c = att("3", "../../etc/passwd");
        let d = att("4", "");
        let e = att("5", "noext");
        let f = att("6", "noext");
        let names = unique_names(&[&a, &b, &c, &d, &e, &f]);
        assert_eq!(
            names,
            [
                "log-1.txt",
                "log-2.txt",
                "passwd",
                "attachment-4",
                "noext-5",
                "noext-6"
            ]
        );
    }

    #[test]
    fn globs_match_case_insensitively_or_everything() {
        let p = patterns(&["*.LOG".into(), "shot*".into()]).unwrap();
        assert!(matches(&p, "app.log"));
        assert!(matches(&p, "Shot1.png"));
        assert!(!matches(&p, "notes.txt"));
        assert!(matches(&[], "anything"));
        assert!(patterns(&["[".into()]).is_err());
    }

    #[test]
    fn zips_extract_without_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.add_directory("sub", options).unwrap();
        zip.start_file("sub/inner.txt", options).unwrap();
        zip.write_all(b"inner").unwrap();
        zip.start_file("../escape.txt", options).unwrap();
        zip.write_all(b"nope").unwrap();
        zip.finish().unwrap();
        let dest = dir.path().join("a");
        assert_eq!(extract_zip(&zip_path, &dest).unwrap(), 1);
        assert_eq!(
            std::fs::read_to_string(dest.join("sub/inner.txt")).unwrap(),
            "inner"
        );
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn rows_show_size_date_author_and_name() {
        let a = Attachment {
            id: "7".into(),
            filename: "x.png".into(),
            size: Some(2048),
            author: Some("Ann".into()),
            created: Some("2026-10-26T14:25:34.396+0000".into()),
            ..Default::default()
        };
        let rows = rows(&[a]);
        assert_eq!(rows[0][0], "7");
        assert_eq!(rows[0][1], "2.0 KB");
        assert!(rows[0][2].starts_with("2026-10-26"));
        assert_eq!(rows[0][3], "Ann");
        assert_eq!(rows[0][4], "x.png");
    }
}
