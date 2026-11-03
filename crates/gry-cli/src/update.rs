//! Self-update from the project's public GitHub Releases.
//!
//! The updater deliberately stays on the free, stable distribution path:
//!
//! * only published releases are considered (`releases/latest`, never a
//!   prerelease or a branch build);
//! * the release archive is verified against the published `SHA256SUMS`;
//! * the binary is replaced with a single atomic rename on Unix;
//! * package-manager owned installs (Homebrew, Nix, Scoop, Chocolatey, ...)
//!   are refused instead of being silently overwritten.
//!
//! Downloads use the system `curl` or `wget` (and PowerShell on Windows) so the
//! binary does not have to embed a TLS stack, certificate bundle, or proxy
//! configuration. There is no update server, account, or telemetry.

use gry_core::{GryError, GryResult};
use semver::Version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_REPO: &str = "PizenLabs/gry";
const DEFAULT_RELEASES_URL: &str = "https://github.com";
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default)]
pub(crate) struct UpdateOptions {
    pub check: bool,
    pub version: Option<String>,
    pub force: bool,
    pub dry_run: bool,
    pub insecure: bool,
    pub quiet: bool,
}

/// Resolve the latest stable release, verify it, and optionally replace the
/// running binary. Returns the machine-readable report printed by the CLI.
pub(crate) fn run(options: &UpdateOptions) -> GryResult<Value> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| GryError::msg(format!("invalid built-in version: {error}")))?;

    let requested = requested_version(options);
    let tag = match requested.as_deref() {
        Some(tag) => normalize_tag(tag),
        None => resolve_latest_tag()?,
    };
    let latest = Version::parse(tag.trim_start_matches('v')).map_err(|error| {
        GryError::invalid(format!("release tag {tag} is not a valid version: {error}"))
    })?;

    let newer = latest > current;
    let different = latest != current;
    let asset = release_target().ok().map(|target| asset_name(&target));
    let base = release_base(&tag);

    if options.check {
        let status = if newer {
            "update_available"
        } else if different {
            "different_version"
        } else {
            "up_to_date"
        };
        let managed = current_binary()
            .ok()
            .and_then(|path| managed_install(&path));
        return Ok(json!({
            "status": status,
            "current": current.to_string(),
            "latest": latest.to_string(),
            "tag": tag,
            "update_available": newer,
            "asset": asset,
            "url": asset.map(|name| format!("{base}/{name}")),
            "managed_by": managed,
        }));
    }

    if !different && !options.force {
        return Ok(json!({
            "status": "up_to_date",
            "current": current.to_string(),
            "latest": latest.to_string(),
            "tag": tag,
            "message": format!("gry {current} is up to date"),
        }));
    }

    if latest < current && requested.is_none() {
        return Ok(json!({
            "status": "up_to_date",
            "current": current.to_string(),
            "latest": latest.to_string(),
            "tag": tag,
            "message": format!(
                "gry {current} is newer than the latest stable release {tag}; nothing to do"
            ),
        }));
    }

    let target = release_target()?;
    let asset = asset_name(&target);
    let binary_name = binary_name(&target);
    let url = format!("{base}/{asset}");
    let path = current_binary()?;
    let managed = managed_install(&path);

    if !options.dry_run {
        if let Some(tool) = managed {
            if !options.force {
                return Err(GryError::msg(format!(
                    "gry was installed by {tool}; update it with that tool, or re-run with --force to replace {} in place",
                    display_path(&path)
                )));
            }
            note(
                options.quiet,
                &format!("warning: replacing a binary managed by {tool}"),
            );
        }
        ensure_writable(path.parent().unwrap_or_else(|| Path::new(".")), &path)?;
    }

    let report = json!({
        "status": if options.dry_run { "would_update" } else { "updated" },
        "from": current.to_string(),
        "to": latest.to_string(),
        "tag": tag,
        "asset": asset,
        "url": url,
        "path": display_path(&path),
        "managed_by": managed,
        "verified": !options.insecure,
        "dry_run": options.dry_run,
    });

    if options.dry_run {
        let mut report = report;
        report["verified"] = json!(false);
        report["message"] = json!(match managed {
            Some(tool) => format!(
                "would update gry {current} -> {latest}, but the binary is managed by {tool}; use that tool or --force"
            ),
            None => format!("would update gry {current} -> {latest}"),
        });
        return Ok(report);
    }

    let work = TempTree::new()?;
    let archive = work.path().join(&asset);
    note(options.quiet, &format!("downloading {url}"));
    fetch_to(&url, &archive)?;
    if fs::metadata(&archive)?.len() > MAX_ARCHIVE_BYTES {
        return Err(GryError::msg(format!(
            "{asset} is larger than {MAX_ARCHIVE_BYTES} bytes; refusing to install"
        )));
    }

    if options.insecure {
        note(
            options.quiet,
            "warning: checksum verification disabled (--insecure)",
        );
    } else {
        let sums_url = format!("{base}/SHA256SUMS");
        let sums_path = work.path().join("SHA256SUMS");
        fetch_to(&sums_url, &sums_path).map_err(|error| {
            GryError::msg(format!(
                "{error}; re-run with --insecure to skip verification"
            ))
        })?;
        let sums = fs::read_to_string(&sums_path)?;
        let expected = expected_checksum(&sums, &asset).ok_or_else(|| {
            GryError::msg(format!(
                "SHA256SUMS does not list {asset}; re-run with --insecure to skip verification"
            ))
        })?;
        let actual = sha256_file(&archive)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(GryError::msg(format!(
                "SHA-256 mismatch for {asset} (expected {expected}, got {actual}); refusing to install"
            )));
        }
        note(options.quiet, "checksum verified");
    }

    let extracted = work.path().join(binary_name);
    extract_binary(&archive, binary_name, &extracted)?;
    replace_binary(&extracted, &path)?;

    let mut report = report;
    report["message"] = json!(format!("updated gry {current} -> {latest}"));
    Ok(report)
}

fn requested_version(options: &UpdateOptions) -> Option<String> {
    options
        .version
        .clone()
        .or_else(|| std::env::var("GRY_VERSION").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("latest"))
}

fn normalize_tag(tag: &str) -> String {
    let tag = tag.trim();
    if let Some(rest) = tag.strip_prefix(['v', 'V']) {
        format!("v{rest}")
    } else {
        format!("v{tag}")
    }
}

fn repo() -> String {
    std::env::var("GRY_REPO")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string())
}

fn releases_url() -> String {
    std::env::var("GRY_RELEASES_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{DEFAULT_RELEASES_URL}/{}/releases", repo()))
}

fn api_url() -> String {
    std::env::var("GRY_UPDATE_API_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("https://api.github.com/repos/{}/releases/latest", repo()))
}

fn release_base(tag: &str) -> String {
    format!("{}/download/{tag}", releases_url())
}

fn release_target() -> GryResult<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin".to_string()),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin".to_string()),
        ("linux", "x86_64") | ("linux", "aarch64") => {
            // The target environment is fixed at compile time, so a musl build
            // asks for the static musl asset and a glibc build asks for gnu.
            let libc = if cfg!(target_env = "musl") {
                "musl"
            } else {
                "gnu"
            };
            Ok(format!("{arch}-unknown-linux-{libc}"))
        }
        ("windows", "x86_64") => Ok("x86_64-pc-windows-gnu".to_string()),
        _ => Err(GryError::unsupported(format!(
            "self-update is not available for {os}/{arch}; use the installer or rebuild from source"
        ))),
    }
}

fn asset_name(target: &str) -> String {
    format!("gry-{target}.tar.gz")
}

fn binary_name(target: &str) -> &'static str {
    if target.contains("windows") {
        "gry.exe"
    } else {
        "gry"
    }
}

fn current_binary() -> GryResult<PathBuf> {
    let exe = std::env::current_exe()?;
    // Resolve symlinks (for example a Homebrew `bin/gry` shim) so the real
    // file is replaced and managed-install detection sees the real location.
    let resolved = fs::canonicalize(&exe).unwrap_or(exe);
    if !fs::metadata(&resolved)?.is_file() {
        return Err(GryError::msg(format!(
            "refusing to update {}: it is not a regular file",
            display_path(&resolved)
        )));
    }
    let name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // A shell wrapper resolves to its interpreter (bash, sh, ...). Replacing
    // that would damage the system, so only self-update a gry binary.
    if !name.starts_with("gry") {
        return Err(GryError::msg(format!(
            "refusing to self-update a process named {name:?}; run the gry binary directly or use the installer"
        )));
    }
    Ok(resolved)
}

fn display_path(path: &Path) -> String {
    let raw = path.to_string_lossy().into_owned();
    raw.strip_prefix(r"\\?\")
        .map(|stripped| stripped.to_string())
        .unwrap_or(raw)
}

fn managed_install(path: &Path) -> Option<&'static str> {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.contains("/Cellar/")
        || path.starts_with("/opt/homebrew/")
        || path.starts_with("/usr/local/Homebrew/")
        || path.starts_with("/home/linuxbrew/")
    {
        Some("Homebrew (`brew upgrade gry`)")
    } else if path.contains("/nix/store/") {
        Some("Nix (`nix profile upgrade`)")
    } else if path.contains("/scoop/apps/") {
        Some("Scoop (`scoop update gry`)")
    } else if path.contains("/chocolatey/") {
        Some("Chocolatey (`choco upgrade gry`)")
    } else {
        None
    }
}

/// Fail before downloading when the running binary cannot be replaced.
fn ensure_writable(dir: &Path, binary: &Path) -> GryResult<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let probe = dir.join(format!(".gry-write-probe-{}-{stamp}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(GryError::msg(format!(
            "cannot write to {dir:?} to update {binary:?}: {error}; re-run with sufficient permissions or reinstall"
        ))),
    }
}

fn sha256_file(path: &Path) -> GryResult<String> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn expected_checksum(sums: &str, asset: &str) -> Option<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let (Some(checksum), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let name = name.trim_start_matches('*').trim_start_matches("./");
        if name == asset {
            return Some(checksum.to_ascii_lowercase());
        }
    }
    None
}

/// Extract only the expected regular file from a gzip-compressed tar archive.
/// Every other entry, including absolute paths and `..` traversal, is ignored.
fn extract_binary(archive_path: &Path, binary: &str, dest: &Path) -> GryResult<()> {
    let file = File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    let mut archive = tar::Archive::new(decoder);
    let mut found = false;

    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?;
        if path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) != Some(binary) {
            continue;
        }
        if found {
            return Err(GryError::msg(format!(
                "{archive_path:?} contains more than one {binary} entry"
            )));
        }
        if entry.size() > MAX_ARCHIVE_BYTES {
            return Err(GryError::msg(format!(
                "{binary} in {archive_path:?} is unexpectedly large"
            )));
        }
        let mut out = File::create(dest)?;
        std::io::copy(&mut entry, &mut out)?;
        out.flush()?;
        out.sync_all()?;
        found = true;
    }

    if !found {
        let _ = fs::remove_file(dest);
        return Err(GryError::msg(format!(
            "release archive does not contain a regular {binary} file"
        )));
    }
    if fs::metadata(dest)?.len() == 0 {
        let _ = fs::remove_file(dest);
        return Err(GryError::msg(format!(
            "release archive contains an empty {binary} file"
        )));
    }
    make_executable(dest)?;
    Ok(())
}

/// Replace `target` atomically. The new file is staged in the same directory so
/// the final rename never crosses a filesystem boundary.
fn replace_binary(source: &Path, target: &Path) -> GryResult<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("gry");
    let pid = std::process::id();

    #[cfg(windows)]
    cleanup_stale_files(parent, file_name);

    // Stage with create_new so a pre-existing file or symlink in the install
    // directory cannot redirect the write. The name is retried on collision.
    let mut staged = None;
    for attempt in 0..16 {
        let candidate = parent.join(format!(".{file_name}.update-{pid}-{attempt}"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut output) => {
                let copied = (|| -> std::io::Result<()> {
                    let mut input = File::open(source)?;
                    std::io::copy(&mut input, &mut output)?;
                    output.flush()?;
                    output.sync_all()
                })();
                drop(output);
                match copied {
                    Ok(()) => {
                        staged = Some(candidate);
                        break;
                    }
                    Err(error) => {
                        let _ = fs::remove_file(&candidate);
                        return Err(error.into());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let staged = staged.ok_or_else(|| {
        GryError::msg(format!(
            "could not stage a new binary in {parent:?}; remove stale .{file_name}.update-* files"
        ))
    })?;

    let result = (|| -> GryResult<()> {
        make_executable(&staged)?;

        #[cfg(windows)]
        {
            // Windows allows renaming a running executable but not replacing
            // it. Move the old file aside, then put the new one in place.
            let old = parent.join(format!(".{file_name}.old-{pid}"));
            let _ = fs::remove_file(&old);
            fs::rename(target, &old).map_err(|error| {
                GryError::msg(format!(
                    "cannot move {} aside for update: {error}",
                    display_path(target)
                ))
            })?;
            if let Err(error) = fs::rename(&staged, target) {
                let _ = fs::rename(&old, target);
                return Err(GryError::msg(format!(
                    "cannot replace {}: {error}",
                    display_path(target)
                )));
            }
            // Still mapped by this process; the next update removes it.
            let _ = fs::remove_file(&old);
        }

        #[cfg(not(windows))]
        {
            fs::rename(&staged, target).map_err(|error| {
                GryError::msg(format!("cannot replace {}: {error}", display_path(target)))
            })?;
        }

        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

#[cfg(windows)]
fn cleanup_stale_files(parent: &Path, file_name: &str) {
    let prefix = format!(".{file_name}.old-");
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn note(quiet: bool, message: &str) {
    if !quiet {
        eprintln!("gry: {message}");
    }
}

// ---------------------------------------------------------------------------
// Release discovery
// ---------------------------------------------------------------------------

fn resolve_latest_tag() -> GryResult<String> {
    // An explicit API override is used as-is: it may be a mirror or a static
    // file used by packaging systems, and it should not fall back to the
    // public release redirect.
    if std::env::var_os("GRY_UPDATE_API_URL").is_some() {
        return latest_from_api();
    }

    let releases = releases_url();
    let mut errors = Vec::new();

    // Prefer the release redirect: it needs no API quota and is exactly the
    // page a user opens for the latest stable build.
    match latest_from_redirect(&releases) {
        Ok(tag) => return Ok(tag),
        Err(error) => errors.push(error.to_string()),
    }

    // Fall back to the public API for environments where HEAD/redirect
    // resolution is unavailable (for example wget/Windows PowerShell).
    match latest_from_api() {
        Ok(tag) => return Ok(tag),
        Err(error) => errors.push(error.to_string()),
    }

    Err(GryError::msg(format!(
        "no stable release could be resolved ({}); pass --version vX.Y.Z (or set GRY_VERSION) to choose a release explicitly",
        errors.join("; ")
    )))
}

fn latest_from_redirect(releases: &str) -> GryResult<String> {
    let url = format!("{releases}/latest");
    ensure_url(&url)?;
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--head",
            "--output",
            null_device(),
            "--write-out",
            "%{url_effective}",
            "--connect-timeout",
            "15",
            "--max-time",
            "60",
        ])
        .arg("--url")
        .arg(&url)
        .output()
        .map_err(|error| GryError::msg(format!("curl is unavailable ({error})")))?;

    if !output.status.success() {
        return Err(GryError::msg(format!(
            "release redirect failed: {}",
            stderr_text(&output)
        )));
    }
    let effective = String::from_utf8_lossy(&output.stdout);
    tag_from_release_url(effective.trim()).ok_or_else(|| {
        GryError::msg(format!(
            "no stable release is published yet ({})",
            effective.trim()
        ))
    })
}

fn latest_from_api() -> GryResult<String> {
    let body = fetch_text(&api_url())?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|error| GryError::msg(format!("invalid release metadata: {error}")))?;
    value
        .get("tag_name")
        .and_then(Value::as_str)
        .map(normalize_tag)
        .ok_or_else(|| GryError::msg("release metadata has no tag_name"))
}

fn tag_from_release_url(url: &str) -> Option<String> {
    let marker = "/releases/tag/";
    let index = url.find(marker)?;
    let tag = url[index + marker.len()..]
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/');
    if tag.is_empty() || !tag.starts_with('v') {
        None
    } else {
        Some(tag.to_string())
    }
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

fn fetch_to(url: &str, dest: &Path) -> GryResult<()> {
    ensure_url(url)?;
    let mut errors = Vec::new();
    let user_agent = format!("gry/{}", env!("CARGO_PKG_VERSION"));
    let max_size = MAX_ARCHIVE_BYTES.to_string();

    let curl = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "--connect-timeout",
            "15",
            "--max-time",
            "600",
            "--max-filesize",
            &max_size,
            "--user-agent",
            &user_agent,
            "--output",
        ])
        .arg(dest)
        .arg("--url")
        .arg(url)
        .output();
    match curl {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) => {
            let _ = fs::remove_file(dest);
            errors.push(stderr_text(&output));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!("curl: {error}")),
    }

    let wget = Command::new("wget")
        .args(["-q", "-t", "3", "-T", "30", "-O"])
        .arg(dest)
        .arg(url)
        .output();
    match wget {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) => {
            let _ = fs::remove_file(dest);
            errors.push(format!("wget: {}", stderr_text(&output)));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!("wget: {error}")),
    }

    #[cfg(windows)]
    match powershell_to(url, dest) {
        Ok(()) => return Ok(()),
        Err(error) => errors.push(error.to_string()),
    }

    Err(GryError::msg(format!(
        "could not download {url}: {}",
        if errors.is_empty() {
            "curl or wget is required".to_string()
        } else {
            errors.join("; ")
        }
    )))
}

fn fetch_text(url: &str) -> GryResult<Vec<u8>> {
    ensure_url(url)?;
    let user_agent = format!("gry/{}", env!("CARGO_PKG_VERSION"));
    let mut errors = Vec::new();

    let curl = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "--connect-timeout",
            "15",
            "--max-time",
            "60",
            "--user-agent",
            &user_agent,
            "--header",
            "Accept: application/vnd.github+json",
        ])
        .arg("--url")
        .arg(url)
        .output();
    match curl {
        Ok(output) if output.status.success() => return Ok(output.stdout),
        Ok(output) => errors.push(stderr_text(&output)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!("curl: {error}")),
    }

    let wget = Command::new("wget")
        .args(["-q", "-t", "3", "-T", "30", "-O", "-"])
        .arg(url)
        .output();
    match wget {
        Ok(output) if output.status.success() => return Ok(output.stdout),
        Ok(output) => errors.push(format!("wget: {}", stderr_text(&output))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!("wget: {error}")),
    }

    #[cfg(windows)]
    match powershell_text(url) {
        Ok(body) => return Ok(body),
        Err(error) => errors.push(error.to_string()),
    }

    Err(GryError::msg(format!(
        "could not fetch {url}: {}",
        if errors.is_empty() {
            "curl or wget is required".to_string()
        } else {
            errors.join("; ")
        }
    )))
}

fn ensure_url(url: &str) -> GryResult<()> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("file://")
    {
        Ok(())
    } else {
        Err(GryError::msg(format!(
            "refusing unsupported update URL: {url}"
        )))
    }
}

fn stderr_text(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    let text = text.trim();
    if text.is_empty() {
        output.status.to_string()
    } else {
        text.to_string()
    }
}

fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

#[cfg(windows)]
fn powershell_to(url: &str, dest: &Path) -> GryResult<()> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -UseBasicParsing -Uri $env:GRY_UPDATE_URL -OutFile $env:GRY_UPDATE_DEST",
        ])
        .env("GRY_UPDATE_URL", url)
        .env("GRY_UPDATE_DEST", dest)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(GryError::msg(format!(
            "PowerShell download failed: {}",
            stderr_text(&output)
        )))
    }
}

#[cfg(windows)]
fn powershell_text(url: &str) -> GryResult<Vec<u8>> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $ProgressPreference='SilentlyContinue'; (Invoke-WebRequest -UseBasicParsing -Uri $env:GRY_UPDATE_URL).Content",
        ])
        .env("GRY_UPDATE_URL", url)
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GryError::msg(format!(
            "PowerShell request failed: {}",
            stderr_text(&output)
        )))
    }
}

// ---------------------------------------------------------------------------
// Temporary workspace
// ---------------------------------------------------------------------------

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new() -> GryResult<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("gry-update-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;

    fn archive_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            if name.starts_with("..") {
                // The builder refuses to write traversal paths through its safe
                // API, so fill the raw header to test the reader's own check.
                let raw = name.as_bytes();
                header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            } else {
                header.set_cksum();
                builder.append_data(&mut header, name, *data).unwrap();
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn parses_checksum_lines() {
        let sums = "abc123  gry-aarch64-apple-darwin.tar.gz\n\
                    def456 *gry-x86_64-unknown-linux-musl.tar.gz\n\
                    aaa111  ./gry-x86_64-apple-darwin.tar.gz\n";
        assert_eq!(
            expected_checksum(sums, "gry-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            expected_checksum(sums, "gry-x86_64-unknown-linux-musl.tar.gz").as_deref(),
            Some("def456")
        );
        assert_eq!(
            expected_checksum(sums, "gry-x86_64-apple-darwin.tar.gz").as_deref(),
            Some("aaa111")
        );
        assert_eq!(expected_checksum(sums, "missing.tar.gz"), None);
    }

    #[test]
    fn normalizes_release_tags() {
        assert_eq!(normalize_tag("1.2.3"), "v1.2.3");
        assert_eq!(normalize_tag("v1.2.3"), "v1.2.3");
        assert_eq!(normalize_tag("V1.2.3"), "v1.2.3");
        assert_eq!(normalize_tag(" v1.2.3 "), "v1.2.3");
    }

    #[test]
    fn extracts_only_the_expected_binary() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("release.tar.gz");
        let bytes = archive_with(&[
            ("LICENSE", b"license"),
            ("gry", b"binary"),
            ("../escape", b"nope"),
        ]);
        fs::write(&archive, bytes).unwrap();
        let dest = dir.path().join("out");
        extract_binary(&archive, "gry", &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"binary");
        assert!(!dir.path().join("escape").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dest).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
    }

    #[test]
    fn rejects_archives_without_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("release.tar.gz");
        fs::write(&archive, archive_with(&[("LICENSE", b"license")])).unwrap();
        let error = extract_binary(&archive, "gry", &dir.path().join("out")).unwrap_err();
        assert!(error.to_string().contains("does not contain"));
    }

    #[test]
    fn rejects_an_empty_binary() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("release.tar.gz");
        fs::write(&archive, archive_with(&[("gry", b"")])).unwrap();
        let error = extract_binary(&archive, "gry", &dir.path().join("out")).unwrap_err();
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn replaces_the_binary_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gry");
        fs::write(&target, b"old").unwrap();
        let source = dir.path().join("new");
        fs::write(&source, b"new").unwrap();
        replace_binary(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
    }

    #[test]
    fn derives_asset_and_binary_names() {
        assert_eq!(
            asset_name("aarch64-apple-darwin"),
            "gry-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(binary_name("x86_64-pc-windows-gnu"), "gry.exe");
        assert_eq!(binary_name("x86_64-unknown-linux-musl"), "gry");
    }

    #[test]
    fn detects_managed_install_paths() {
        assert!(managed_install(Path::new("/opt/homebrew/Cellar/gry/1.0.4/bin/gry")).is_some());
        assert!(managed_install(Path::new("/nix/store/abc-gry/bin/gry")).is_some());
        assert!(managed_install(Path::new("/home/user/.local/bin/gry")).is_none());
    }

    #[test]
    fn parses_the_release_redirect() {
        assert_eq!(
            tag_from_release_url("https://github.com/PizenLabs/gry/releases/tag/v1.2.3").as_deref(),
            Some("v1.2.3")
        );
        assert_eq!(
            tag_from_release_url("https://github.com/PizenLabs/gry/releases"),
            None
        );
        assert_eq!(
            tag_from_release_url("https://github.com/PizenLabs/gry/releases/tag/1.2.3"),
            None
        );
    }
}
