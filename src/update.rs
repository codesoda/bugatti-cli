//! Self-update: version checking and self-replace for bugatti.
//!
//! Checks for new versions by following the GitHub `/releases/latest` redirect
//! (avoids API rate limits — one request, no token needed). Provides both a
//! manual `bugatti update` command and a passive background check after tests.

use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Production GitHub Releases URL.
const GITHUB_RELEASES_LATEST_URL: &str = "https://github.com/codesoda/bugatti-cli/releases/latest";

const USER_AGENT: &str = "bugatti-cli";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const PASSIVE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const PASSIVE_CHECK_ENV_VAR: &str = "BUGATTI_NO_UPDATE_CHECK";
const CHECKSUMS_FILENAME: &str = "checksums-sha256.txt";
const BINARY_NAME: &str = "bugatti";

/// Minimum interval between passive version checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(8 * 3600);

// ---------------------------------------------------------------------------
// Version helpers
// ---------------------------------------------------------------------------

/// Returns the compiled-in package version (from Cargo.toml at build time).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Strips an optional leading `v` or `V` from a version tag.
fn normalize_version_tag(tag: &str) -> &str {
    let trimmed = tag.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
}

/// Compares two version strings using semver.
///
/// Returns `Ordering::Less` if `local < remote` (update available).
fn compare_versions(local: &str, remote: &str) -> Result<Ordering, String> {
    let local_ver = semver::Version::parse(normalize_version_tag(local))
        .map_err(|e| format!("invalid local version '{local}': {e}"))?;
    let remote_ver = semver::Version::parse(normalize_version_tag(remote))
        .map_err(|e| format!("invalid remote version '{remote}': {e}"))?;
    Ok(local_ver.cmp(&remote_ver))
}

// ---------------------------------------------------------------------------
// Release discovery via HTTP redirect
// ---------------------------------------------------------------------------

/// Metadata for a release, constructed from the tag without API calls.
struct ReleaseMetadata {
    tag: String,
    artifact_url: String,
    checksums_url: String,
}

/// Discovers the latest release tag by following the GitHub redirect.
///
/// Sends a GET to the releases/latest URL with redirect following disabled.
/// GitHub returns a 302 with `Location: .../releases/tag/v0.4.1` — we extract
/// the tag from that header. One request, unlimited rate, no API token.
fn discover_latest_tag(url: &str, timeout: Duration) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("failed to connect to release server: {e}"))?;

    let status = response.status();
    if !status.is_redirection() {
        return Err(format!(
            "release server returned HTTP {status} — expected a redirect"
        ));
    }

    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or("redirect response missing Location header")?
        .to_str()
        .map_err(|_| "Location header is not valid UTF-8")?;

    location
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("could not extract version tag from redirect URL: {location}"))
}

/// Builds release metadata from a discovered tag.
///
/// Constructs download URLs using the naming convention from the release workflow:
/// `bugatti-{tag}-{target}.tar.gz`
fn build_release_metadata(tag: &str, repo_base: &str) -> ReleaseMetadata {
    let target = build_target();
    let download_base = format!("{repo_base}/releases/download/{tag}");
    let artifact_name = format!("bugatti-{tag}-{target}.tar.gz");

    ReleaseMetadata {
        tag: tag.to_string(),
        artifact_url: format!("{download_base}/{artifact_name}"),
        checksums_url: format!("{download_base}/{CHECKSUMS_FILENAME}"),
    }
}

/// Returns the compile-time target triple (e.g., "aarch64-apple-darwin").
fn build_target() -> &'static str {
    env!("TARGET")
}

/// Strips `/releases/latest` suffix to derive the repository base URL.
fn repo_base_from_url(url: &str) -> &str {
    url.strip_suffix("/releases/latest").unwrap_or(url)
}

/// Fetches release metadata from the given URL.
fn check_latest_version(url: &str, timeout: Duration) -> Result<ReleaseMetadata, String> {
    let tag = discover_latest_tag(url, timeout)?;
    let repo_base = repo_base_from_url(url);
    Ok(build_release_metadata(&tag, repo_base))
}

// ---------------------------------------------------------------------------
// Download helpers
// ---------------------------------------------------------------------------

/// Downloads a URL to a file in the given directory.
fn download_to_file(url: &str, dest_dir: &Path, filename: &str) -> Result<PathBuf, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("failed to download '{filename}': {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "download of '{filename}' failed: HTTP {status} from {url}"
        ));
    }

    let bytes = response
        .bytes()
        .map_err(|e| format!("failed to read response for '{filename}': {e}"))?;

    let dest_path = dest_dir.join(filename);
    std::fs::write(&dest_path, &bytes).map_err(|e| format!("failed to write '{filename}': {e}"))?;

    Ok(dest_path)
}

// ---------------------------------------------------------------------------
// Checksum verification
// ---------------------------------------------------------------------------

/// Parses a GNU coreutils-format checksums file into a map of filename → hex hash.
///
/// Expected format: `<64-hex-chars>  <filename>` (two spaces).
fn parse_checksums(content: &str) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((hash, filename)) = line.split_once("  ") else {
            return Err(format!(
                "malformed checksum line {} (expected '<hash>  <filename>'): {line}",
                i + 1
            ));
        };
        let hash = hash.trim();
        let filename = filename.trim();
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "invalid SHA256 hash on line {} (expected 64 hex chars): '{hash}'",
                i + 1
            ));
        }
        if filename.is_empty() {
            return Err(format!("empty filename on checksum line {}", i + 1));
        }
        map.insert(filename.to_string(), hash.to_lowercase());
    }
    if map.is_empty() {
        return Err("checksums file is empty".to_string());
    }
    Ok(map)
}

/// Computes the SHA256 digest of a file.
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("failed to open for checksum: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("failed to read for checksum: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Verifies that a downloaded artifact matches its expected SHA256 hash.
fn verify_checksum(
    checksums: &HashMap<String, String>,
    artifact_name: &str,
    artifact_path: &Path,
) -> Result<(), String> {
    let expected = checksums.get(artifact_name).ok_or_else(|| {
        format!(
            "checksum entry not found for '{artifact_name}'. Available: [{}]",
            checksums.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let actual = sha256_file(artifact_path)?;
    if actual != *expected {
        return Err(format!(
            "checksum verification failed for '{artifact_name}':\n  expected: {expected}\n  actual:   {actual}\n\
             The downloaded file may be corrupted. Aborting update."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------

/// Extracts the bugatti binary from a tar.gz archive.
///
/// Searches for `bugatti` at any nesting depth (the release archive contains
/// `bugatti-{tag}-{target}/bugatti`).
fn extract_binary(archive_path: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("failed to open archive: {e}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry_result in archive
        .entries()
        .map_err(|e| format!("failed to read tar entries: {e}"))?
    {
        let mut entry = entry_result.map_err(|e| format!("failed to read tar entry: {e}"))?;
        let entry_path = entry
            .path()
            .map_err(|e| format!("failed to read entry path: {e}"))?;

        let file_name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if file_name == BINARY_NAME {
            let dest = dest_dir.join(BINARY_NAME);
            entry
                .unpack(&dest)
                .map_err(|e| format!("failed to extract '{BINARY_NAME}': {e}"))?;
            return Ok(dest);
        }
    }

    Err(format!(
        "archive does not contain '{BINARY_NAME}' binary: {}",
        archive_path.display()
    ))
}

// ---------------------------------------------------------------------------
// macOS binary security
// ---------------------------------------------------------------------------

/// Removes quarantine attributes and applies ad-hoc code signature on macOS.
#[cfg(target_os = "macos")]
fn secure_binary(path: &Path) {
    use std::process::Command;

    for attr in ["com.apple.quarantine", "com.apple.provenance"] {
        let _ = Command::new("xattr").args(["-dr", attr]).arg(path).output();
    }

    let _ = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .output();
}

#[cfg(not(target_os = "macos"))]
fn secure_binary(_path: &Path) {}

// ---------------------------------------------------------------------------
// Self-replacement
// ---------------------------------------------------------------------------

/// Replaces the currently running binary with the file at `replacement_path`.
fn self_replace_binary(replacement_path: &Path) -> Result<(), String> {
    self_replace::self_replace(replacement_path).map_err(|e| {
        format!(
            "failed to replace the running binary (permissions issue?): {e}\n\
             Replacement file: {}",
            replacement_path.display()
        )
    })
}

// ---------------------------------------------------------------------------
// Confirmation prompt
// ---------------------------------------------------------------------------

/// Simple y/N confirmation prompt.
fn confirm_update(local: &str, remote: &str) -> bool {
    print!("Update bugatti v{local} → v{remote}? [y/N] ");
    let _ = io::stdout().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

// ---------------------------------------------------------------------------
// Full update orchestration
// ---------------------------------------------------------------------------

/// Runs the update command.
///
/// If `check` is true, only prints whether an update is available.
/// Otherwise, downloads, verifies, extracts, and replaces the running binary.
pub fn run_update(check: bool, yes: bool) -> Result<(), String> {
    if check {
        return run_update_check();
    }
    run_update_install(yes)
}

/// Check-only: prints whether an update is available.
fn run_update_check() -> Result<(), String> {
    let local = current_version();

    let release = match check_latest_version(GITHUB_RELEASES_LATEST_URL, REQUEST_TIMEOUT) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Warning: unable to check for updates: {e}");
            return Ok(());
        }
    };

    let remote = normalize_version_tag(&release.tag);

    match compare_versions(local, &release.tag) {
        Ok(Ordering::Less) => {
            println!("Update available: v{local} → v{remote}");
            println!("Run `bugatti update` to install it.");
        }
        Ok(_) => {
            println!("bugatti v{local} is up to date");
        }
        Err(e) => {
            eprintln!("Warning: unable to compare versions: {e}");
        }
    }

    Ok(())
}

/// Full install: download, verify, extract, secure, replace.
fn run_update_install(yes: bool) -> Result<(), String> {
    let local = current_version();

    // Step 1: Fetch release metadata
    let release = check_latest_version(GITHUB_RELEASES_LATEST_URL, REQUEST_TIMEOUT)
        .map_err(|e| format!("failed to check for latest release: {e}"))?;

    let remote = normalize_version_tag(&release.tag);

    // Step 2: Compare versions
    let ordering =
        compare_versions(local, &release.tag).map_err(|e| format!("version comparison: {e}"))?;

    if ordering != Ordering::Less {
        println!("bugatti v{local} is already up to date (latest: v{remote})");
        return Ok(());
    }

    // Step 3: Prompt for confirmation
    if !yes && !confirm_update(local, remote) {
        println!("Update cancelled.");
        return Ok(());
    }

    // Step 4: Download to temp directory
    let tmp_dir =
        tempfile::tempdir().map_err(|e| format!("failed to create temp directory: {e}"))?;

    println!("Downloading bugatti v{remote}...");

    let checksums_path =
        download_to_file(&release.checksums_url, tmp_dir.path(), CHECKSUMS_FILENAME)
            .map_err(|e| format!("failed to download checksums: {e}"))?;

    let artifact_name = release
        .artifact_url
        .rsplit('/')
        .next()
        .unwrap_or("artifact.tar.gz");
    let artifact_path = download_to_file(&release.artifact_url, tmp_dir.path(), artifact_name)
        .map_err(|e| format!("failed to download release: {e}"))?;

    // Step 5: Verify checksum
    let checksums_content = std::fs::read_to_string(&checksums_path)
        .map_err(|e| format!("failed to read checksums file: {e}"))?;
    let checksums = parse_checksums(&checksums_content)?;
    verify_checksum(&checksums, artifact_name, &artifact_path)?;

    println!("Checksum verified ✓");

    // Step 6: Extract binary
    let extract_dir = tmp_dir.path().join("extracted");
    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("failed to create extraction directory: {e}"))?;
    let new_binary = extract_binary(&artifact_path, &extract_dir)?;

    // Step 7: Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&new_binary, perms)
            .map_err(|e| format!("failed to set executable permissions: {e}"))?;
    }

    // Step 8: macOS binary security (quarantine removal + adhoc codesign)
    secure_binary(&new_binary);

    // Step 9: Replace running binary
    self_replace_binary(&new_binary)?;

    println!("Successfully updated bugatti v{local} → v{remote}");

    Ok(())
}

// ---------------------------------------------------------------------------
// Passive background version check
// ---------------------------------------------------------------------------

/// Returns the path to the last-update-check timestamp file.
fn last_update_check_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".bugatti");
    Some(dir.join("last-update-check"))
}

/// Determines whether a passive version check should run.
fn should_check_for_update() -> bool {
    // Suppress when env var is set to "1"
    if std::env::var(PASSIVE_CHECK_ENV_VAR).as_deref() == Ok("1") {
        return false;
    }

    // Suppress when not a TTY
    if !io::stdout().is_terminal() {
        return false;
    }

    // Check timestamp file
    let timestamp_path = match last_update_check_path() {
        Some(p) => p,
        None => return true, // No HOME — first run or broken env
    };

    let metadata = match std::fs::metadata(&timestamp_path) {
        Ok(m) => m,
        Err(_) => return true, // Missing file — first run
    };

    let modified = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };

    match std::time::SystemTime::now().duration_since(modified) {
        Ok(elapsed) => elapsed >= CHECK_INTERVAL,
        Err(_) => true, // Clock went backward
    }
}

/// Writes the last-check timestamp by touching the file.
fn write_last_check_timestamp() {
    if let Some(path) = last_update_check_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Write current epoch seconds
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();
        let _ = std::fs::write(&path, now);
    }
}

/// Formats the update notification message.
fn format_update_notification(current: &str, latest: &str) -> String {
    format!("\nUpdate available: v{current} → v{latest} — run `bugatti update` to install")
}

/// Performs the passive version check (called on a background thread).
fn passive_version_check() {
    if !should_check_for_update() {
        return;
    }

    // Always update timestamp after attempt (prevents retry storms)
    let check_result = check_latest_version(GITHUB_RELEASES_LATEST_URL, PASSIVE_CHECK_TIMEOUT);
    write_last_check_timestamp();

    let release = match check_result {
        Ok(r) => r,
        Err(_) => return,
    };

    let local = current_version();
    if let Ok(Ordering::Less) = compare_versions(local, &release.tag) {
        let remote = normalize_version_tag(&release.tag);
        let msg = format_update_notification(local, remote);
        eprintln!("{msg}");
    }
}

/// Spawns the passive version check on a background thread with a timeout.
///
/// Waits at most 3 seconds for the check to complete. If it doesn't finish,
/// the thread is abandoned (it dies when the process exits).
pub fn spawn_passive_check() {
    let handle = std::thread::spawn(passive_version_check);
    let _ = handle.join(); // join will wait; the HTTP timeout (3s) is the real bound
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_lowercase_v() {
        assert_eq!(normalize_version_tag("v0.3.0"), "0.3.0");
    }

    #[test]
    fn normalize_strips_uppercase_v() {
        assert_eq!(normalize_version_tag("V1.2.3"), "1.2.3");
    }

    #[test]
    fn normalize_no_prefix() {
        assert_eq!(normalize_version_tag("0.3.0"), "0.3.0");
    }

    #[test]
    fn normalize_whitespace() {
        assert_eq!(normalize_version_tag("  v0.3.0  "), "0.3.0");
    }

    #[test]
    fn compare_update_available() {
        assert_eq!(compare_versions("0.3.0", "v0.4.0").unwrap(), Ordering::Less);
    }

    #[test]
    fn compare_up_to_date() {
        assert_eq!(
            compare_versions("0.3.0", "v0.3.0").unwrap(),
            Ordering::Equal
        );
    }

    #[test]
    fn compare_local_newer() {
        assert_eq!(
            compare_versions("0.4.0", "v0.3.0").unwrap(),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_invalid() {
        assert!(compare_versions("not-a-version", "v0.3.0").is_err());
    }

    #[test]
    fn parse_checksums_valid() {
        // 64 hex chars: 8 groups of 8
        let content = "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8  bugatti-v0.3.0-aarch64-apple-darwin.tar.gz\n";
        let map = parse_checksums(content).unwrap();
        assert_eq!(
            map.get("bugatti-v0.3.0-aarch64-apple-darwin.tar.gz")
                .unwrap(),
            "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
        );
    }

    #[test]
    fn parse_checksums_empty() {
        assert!(parse_checksums("").is_err());
    }

    #[test]
    fn parse_checksums_malformed() {
        assert!(parse_checksums("notahash filename").is_err());
    }

    #[test]
    fn build_release_metadata_constructs_urls() {
        let meta = build_release_metadata("v0.3.1", "https://github.com/codesoda/bugatti-cli");
        assert_eq!(meta.tag, "v0.3.1");
        assert!(meta.artifact_url.contains("bugatti-v0.3.1-"));
        assert!(meta.artifact_url.ends_with(".tar.gz"));
        assert!(meta.checksums_url.contains("checksums-sha256.txt"));
    }

    #[test]
    fn repo_base_strips_suffix() {
        assert_eq!(
            repo_base_from_url("https://github.com/codesoda/bugatti-cli/releases/latest"),
            "https://github.com/codesoda/bugatti-cli"
        );
    }

    #[test]
    fn repo_base_no_suffix() {
        assert_eq!(
            repo_base_from_url("http://localhost:8080"),
            "http://localhost:8080"
        );
    }
}
