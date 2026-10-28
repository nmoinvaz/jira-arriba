//! Reading secrets that acli stored through go-keyring, in whichever store the platform has.

use anyhow::{Context, Result, bail};
use base64::Engine;
use std::io::Read;

/// go-keyring prefixes secrets it had to encode to survive the macOS keychain.
const BASE64_PREFIX: &str = "go-keyring-base64:";
const HEX_PREFIX: &str = "go-keyring-encoded:";

/// The secret stored under `service` and `account`, or None when there is none.
pub fn read(service: &str, account: &str) -> Result<Option<Vec<u8>>> {
    read_platform(service, account)
}

#[cfg(target_os = "macos")]
fn read_platform(service: &str, account: &str) -> Result<Option<Vec<u8>>> {
    // go-keyring stores through the security tool, so reading through it too keeps the
    // keychain from prompting for a binary it has not seen.
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .context("Failed to run security")?;
    if !output.status.success() {
        // errSecItemNotFound
        if output.status.code() == Some(44) {
            return Ok(None);
        }
        bail!(
            "security find-generic-password failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(Some(trim_newline(output.stdout)))
}

#[cfg(target_os = "linux")]
fn read_platform(service: &str, account: &str) -> Result<Option<Vec<u8>>> {
    // go-keyring files secrets in the Secret Service under these two attributes.
    let output = std::process::Command::new("secret-tool")
        .args(["lookup", "service", service, "username", account])
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("secret-tool is not installed, and acli's login lives in the Secret Service")
        }
        Err(e) => return Err(e).context("Failed to run secret-tool"),
    };
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    Ok(Some(output.stdout))
}

#[cfg(windows)]
fn read_platform(service: &str, account: &str) -> Result<Option<Vec<u8>>> {
    use std::collections::HashMap;
    // go-keyring names Windows credentials "service:user".
    let target = format!("{service}:{account}");
    let store = windows_native_keyring_store::Store::new()
        .context("Failed to open the Windows credential store")?;
    keyring_core::set_default_store(store);
    let entry = keyring_core::Entry::new_with_modifiers(
        service,
        account,
        &HashMap::from([("target", target.as_str())]),
    )
    .context("Failed to describe the credential")?;
    match entry.get_secret() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("Failed to read the credential"),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn read_platform(_service: &str, _account: &str) -> Result<Option<Vec<u8>>> {
    bail!("Reading acli's login is not supported on this platform")
}

#[cfg(target_os = "macos")]
fn trim_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    while bytes.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        bytes.pop();
    }
    bytes
}

/// The bytes acli stored: undo go-keyring's encoding, then acli's gzip.
pub fn decode(raw: &[u8]) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(raw).ok().map(str::trim);
    let bytes = match text {
        Some(t) if t.starts_with(BASE64_PREFIX) => base64::engine::general_purpose::STANDARD
            .decode(&t[BASE64_PREFIX.len()..])
            .context("The stored login is not valid base64")?,
        Some(t) if t.starts_with(HEX_PREFIX) => unhex(&t[HEX_PREFIX.len()..])?,
        _ => raw.to_vec(),
    };
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut out)
            .context("The stored login is not valid gzip")?;
        return Ok(out);
    }
    Ok(bytes)
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        bail!("The stored login is not valid hex");
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&text[i..i + 2], 16).context("The stored login is not valid hex")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn base64_gzip_blob_decodes() {
        let packed = gzip(br#"{"access_token":"x"}"#);
        let stored = format!(
            "{BASE64_PREFIX}{}\n",
            base64::engine::general_purpose::STANDARD.encode(&packed)
        );
        assert_eq!(
            decode(stored.as_bytes()).unwrap(),
            br#"{"access_token":"x"}"#
        );
    }

    #[test]
    fn hex_and_raw_blobs_decode() {
        let packed = gzip(b"hello");
        let hex: String = packed.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            decode(format!("{HEX_PREFIX}{hex}").as_bytes()).unwrap(),
            b"hello"
        );
        assert_eq!(decode(&packed).unwrap(), b"hello");
        assert_eq!(decode(b"plain").unwrap(), b"plain");
        assert!(decode(format!("{HEX_PREFIX}abc").as_bytes()).is_err());
        assert!(decode(format!("{BASE64_PREFIX}!!").as_bytes()).is_err());
    }
}
