//! The Jira icon on notifications. macOS draws a notification's icon from the sending app's
//! bundle, so a copy of terminal-notifier carries it. The other notifiers take an image path.

#![cfg_attr(test, allow(dead_code))]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};

/// The icon, 512 px square, for notifiers that show an image.
const PNG: &[u8] = include_bytes!("../../assets/jira.png");
/// The icon inset on Apple's app icon grid, 1024 px square, for the macOS bundle.
const MACOS_PNG: &[u8] = include_bytes!("../../assets/jira-macos.png");

const BUNDLE: &str = "jira-arriba.app";
const BUNDLE_ID: &str = "com.github.nmoinvaz.jira-arriba.notify";
const BUNDLE_NAME: &str = "jira-arriba";
const EXECUTABLE: &str = "Contents/MacOS/terminal-notifier";

/// Homebrew's bundle and the one `make install` puts in Applications, for when the watch runs
/// without the shell's PATH.
const STOCK_APPS: [&str; 3] = [
    "/opt/homebrew/opt/terminal-notifier/terminal-notifier.app",
    "/usr/local/opt/terminal-notifier/terminal-notifier.app",
    "/Applications/terminal-notifier.app",
];

/// Registers an app with Launch Services, which the notification service consults when a bundle
/// first connects. The first launch registers too, but after that lookup has already failed.
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// Where the icon is on this machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branding {
    /// The executable of a terminal-notifier copy whose bundle carries the icon.
    pub notifier: Option<PathBuf>,
    /// The icon as a PNG file.
    pub png: Option<PathBuf>,
}

/// The branding for this machine, prepared once per process. Notifications show the notifier's
/// own icon when it can't be prepared.
pub fn branding() -> &'static Branding {
    static BRANDING: OnceLock<Branding> = OnceLock::new();
    BRANDING.get_or_init(|| {
        prepare(std::env::consts::OS).unwrap_or_else(|e| {
            eprintln!("Notifications show the default icon: {e:#}");
            Branding::default()
        })
    })
}

fn prepare(os: &str) -> Result<Branding> {
    let dir = data_dir(os, |key| std::env::var_os(key))?;
    Ok(match os {
        "macos" => Branding {
            notifier: bundle(&dir, stock_app(super::find))?,
            png: None,
        },
        _ => Branding {
            notifier: None,
            png: Some(png(&dir)?),
        },
    })
}

/// Where jira-arriba keeps its files on `os`.
fn data_dir(os: &str, get: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    let var = |key: &str| get(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || var("HOME").context("HOME is not set");
    let base = match os {
        "macos" => home()?.join("Library/Application Support"),
        "windows" => var("LOCALAPPDATA").context("LOCALAPPDATA is not set")?,
        _ => match var("XDG_DATA_HOME") {
            Some(dir) => dir,
            None => home()?.join(".local/share"),
        },
    };
    Ok(base.join("jira-arriba"))
}

/// The icon written as a file, once.
fn png(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("jira.png");
    if fs::read(&path).ok().as_deref() != Some(PNG) {
        fs::create_dir_all(dir)?;
        fs::write(&path, PNG).with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(path)
}

/// The installed terminal-notifier.app, from the binary on the PATH or the usual places.
fn stock_app(find: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    find("terminal-notifier")
        .and_then(|bin| bin.canonicalize().ok())
        .and_then(|bin| app_around(&bin))
        .or_else(|| {
            STOCK_APPS
                .iter()
                .map(PathBuf::from)
                .find(|app| app.join(EXECUTABLE).is_file())
        })
}

/// The bundle a terminal-notifier binary belongs to. Homebrew installs a wrapper script in `bin`
/// beside the bundle, and `make install` links to the bundle's own executable.
fn app_around(bin: &Path) -> Option<PathBuf> {
    let beside = bin.parent()?.parent()?.join("terminal-notifier.app");
    if beside.join(EXECUTABLE).is_file() {
        return Some(beside);
    }
    bin.ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// The branded copy of `stock`, rebuilt when terminal-notifier or the icon changed.
fn bundle(dir: &Path, stock: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let Some(stock) = stock else {
        return Ok(None);
    };
    let app = dir.join(BUNDLE);
    let executable = app.join(EXECUTABLE);
    let stamp_path = dir.join("notifier.stamp");
    let stamp = stamp(&stock)?;
    if executable.is_file() && fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp) {
        return Ok(Some(executable));
    }
    build(&stock, dir, &app).with_context(|| format!("Failed to build {}", app.display()))?;
    fs::write(&stamp_path, stamp)?;
    Ok(Some(executable))
}

/// What a copy was built from: the stock executable's size and modification time, and the icon.
fn stamp(stock: &Path) -> Result<String> {
    let executable = stock.join(EXECUTABLE);
    let meta = fs::metadata(&executable)
        .with_context(|| format!("Failed to read {}", executable.display()))?;
    let modified = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(format!(
        "1 {} {modified} {:016x}\n",
        meta.len(),
        fnv(MACOS_PNG)
    ))
}

/// FNV-1a, enough to notice a changed icon.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// terminal-notifier's `make icon`: copy the bundle, swap the icon, rename it, and sign it ad hoc.
/// The copy is assembled beside `app` and moved into place once complete.
fn build(stock: &Path, dir: &Path, app: &Path) -> Result<()> {
    let fresh = dir.join("jira-arriba.app.part");
    remove(&fresh)?;
    copy_dir(stock, &fresh)?;
    let resources = fresh.join("Contents/Resources");
    fs::write(resources.join("icon.icns"), icns(MACOS_PNG))?;
    let _ = fs::remove_file(resources.join("Terminal.icns"));
    let plist = fresh.join("Contents/Info.plist");
    for (key, value) in [
        ("CFBundleIconFile", "icon"),
        ("CFBundleName", BUNDLE_NAME),
        ("CFBundleIdentifier", BUNDLE_ID),
    ] {
        run(
            "/usr/bin/plutil",
            [
                OsStr::new("-replace"),
                OsStr::new(key),
                OsStr::new("-string"),
                OsStr::new(value),
                plist.as_os_str(),
            ],
        )?;
    }
    run(
        "/usr/bin/codesign",
        [
            OsStr::new("--force"),
            OsStr::new("--sign"),
            OsStr::new("-"),
            fresh.as_os_str(),
        ],
    )?;
    remove(app)?;
    fs::rename(&fresh, app)?;
    let _ = Command::new(LSREGISTER).arg("-f").arg(app).output();
    Ok(())
}

/// An icns holding `png` as its one 1024 px image, which macOS scales to each size it draws.
fn icns(png: &[u8]) -> Vec<u8> {
    let element = (8 + png.len()) as u32;
    let mut out = Vec::with_capacity(16 + png.len());
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(8 + element).to_be_bytes());
    out.extend_from_slice(b"ic10");
    out.extend_from_slice(&element.to_be_bytes());
    out.extend_from_slice(png);
    out
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn remove(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => Ok(other?),
    }
}

fn run(program: &str, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn data_dir_follows_each_platform() {
        let home = &[("HOME", "/home/me")];
        assert_eq!(
            data_dir("macos", env(home)).unwrap(),
            Path::new("/home/me/Library/Application Support/jira-arriba")
        );
        assert_eq!(
            data_dir("linux", env(home)).unwrap(),
            Path::new("/home/me/.local/share/jira-arriba")
        );
        assert_eq!(
            data_dir(
                "linux",
                env(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")])
            )
            .unwrap(),
            Path::new("/data/jira-arriba")
        );
        assert_eq!(
            data_dir(
                "windows",
                env(&[("LOCALAPPDATA", "C:\\Users\\me\\AppData\\Local")])
            )
            .unwrap(),
            PathBuf::from("C:\\Users\\me\\AppData\\Local").join("jira-arriba")
        );
        assert!(data_dir("windows", env(&[("LOCALAPPDATA", "")])).is_err());
    }

    #[test]
    fn finds_the_bundle_beside_a_wrapper_or_around_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let cellar = dir.path().join("Cellar/terminal-notifier/3.1.0");
        let app = cellar.join("terminal-notifier.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join(EXECUTABLE), "").unwrap();
        fs::create_dir_all(cellar.join("bin")).unwrap();
        fs::write(cellar.join("bin/terminal-notifier"), "#!/bin/bash\n").unwrap();
        assert_eq!(
            app_around(&cellar.join("bin/terminal-notifier")),
            Some(app.clone())
        );
        assert_eq!(app_around(&app.join(EXECUTABLE)), Some(app));
        assert_eq!(app_around(Path::new("/usr/bin/terminal-notifier")), None);
    }

    #[test]
    fn stamp_follows_the_stock_executable() {
        let dir = tempfile::tempdir().unwrap();
        let stock = dir.path().join("terminal-notifier.app");
        fs::create_dir_all(stock.join("Contents/MacOS")).unwrap();
        fs::write(stock.join(EXECUTABLE), "one").unwrap();
        let before = stamp(&stock).unwrap();
        assert!(before.starts_with("1 3 "));
        fs::write(stock.join(EXECUTABLE), "three").unwrap();
        assert_ne!(stamp(&stock).unwrap(), before);
        assert!(stamp(&dir.path().join("missing.app")).is_err());
    }

    #[test]
    fn icns_wraps_the_png_in_one_element() {
        let icns = icns(b"png!");
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()), 20);
        assert_eq!(&icns[8..12], b"ic10");
        assert_eq!(u32::from_be_bytes(icns[12..16].try_into().unwrap()), 12);
        assert_eq!(&icns[16..], b"png!");
    }

    #[test]
    fn fnv_matches_the_reference_values() {
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn png_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(&dir.path().join("jira-arriba")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), PNG);
        fs::write(&path, "stale").unwrap();
        png(&dir.path().join("jira-arriba")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), PNG);
    }
}
