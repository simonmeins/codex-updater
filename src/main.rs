use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Parser;
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Proxy, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Archive;
use tempfile::{NamedTempFile, TempDir};

const RELEASE_API_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const REPOSITORY_URL: &str = "https://github.com/openai/codex/releases";
const DEFAULT_ASSET_NAME: &str = "codex-x86_64-unknown-linux-gnu.tar.gz";
const INSTALL_NAME: &str = "codex";
const DEFAULT_TARGET_DIR: &str = "/usr/local/bin";
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const USER_AGENT_VALUE: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Parser)]
#[command(
    name = "codex-updater",
    version,
    about = "Checks the installed Codex CLI version and installs a newer GitHub release if available."
)]
struct Args {
    #[arg(long, default_value = DEFAULT_TARGET_DIR)]
    target_dir: PathBuf,

    #[arg(long, default_value = DEFAULT_ASSET_NAME)]
    asset_name: String,

    #[arg(long, default_value = INSTALL_NAME)]
    install_name: String,

    #[arg(long)]
    check_only: bool,

    #[arg(long)]
    force: bool,

    #[arg(long)]
    github_token: Option<String>,

    #[arg(long)]
    socks5_proxy: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    name: String,
    html_url: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let github_token = args
        .github_token
        .clone()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok());
    let socks5_proxy = args
        .socks5_proxy
        .clone()
        .or_else(|| std::env::var("SOCKS5_PROXY").ok());
    let client = build_client(&github_token, socks5_proxy.as_deref())?;
    let release = fetch_latest_release(&client)?;
    let latest_version = parse_release_version(&release)?;
    let target_path = args.target_dir.join(&args.install_name);
    let current_version = detect_installed_version(&target_path)?;

    print_status(
        &target_path,
        current_version.as_ref(),
        &latest_version,
        &release,
    );

    let needs_update = args.force
        || match &current_version {
            Some(current) => current < &latest_version,
            None => true,
        };

    if !needs_update {
        println!("Codex is up to date. No action required.");
        return Ok(());
    }

    if args.check_only {
        println!("An update is available, but --check-only was set.");
        return Ok(());
    }

    ensure_root()?;

    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == args.asset_name)
        .with_context(|| {
            format!(
                "Release does not contain an asset named '{}'. See {}",
                args.asset_name, release.html_url
            )
        })?;

    let workdir = TempDir::new().context("failed to create temporary working directory")?;
    let downloaded_archive = download_asset(&client, asset, workdir.path())?;
    let extracted_binary = extract_binary_from_archive(
        &downloaded_archive,
        &args.asset_name,
        &args.install_name,
        workdir.path(),
    )?;
    verify_extracted_binary(&extracted_binary, &latest_version)?;
    install_binary_atomically(&extracted_binary, &target_path)?;

    let installed_version = detect_installed_version(&target_path)?;
    match installed_version {
        Some(version) if version == latest_version => {
            println!(
                "Codex was successfully updated to version {}: {}",
                version,
                target_path.display()
            );
        }
        Some(version) => {
            bail!(
                "Installation completed, but the installed version ({version}) does not match the expected version ({latest_version})"
            );
        }
        None => {
            bail!("Installation completed, but the installed Codex version could not be verified")
        }
    }

    Ok(())
}

fn build_client(github_token: &Option<String>, socks5_proxy: Option<&str>) -> Result<Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(USER_AGENT, USER_AGENT_VALUE.parse()?);
    headers.insert(ACCEPT, "application/vnd.github+json".parse()?);

    if let Some(token) = github_token {
        let value = format!("Bearer {token}");
        headers.insert(
            AUTHORIZATION,
            value.parse().context("invalid content in GITHUB_TOKEN")?,
        );
    }

    let mut builder = Client::builder().default_headers(headers).https_only(true);

    if let Some(proxy_url) = socks5_proxy {
        builder = builder.proxy(build_socks5_proxy(proxy_url)?);
    }

    builder.build().context("failed to build HTTP client")
}

fn build_socks5_proxy(proxy_url: &str) -> Result<Proxy> {
    let url = Url::parse(proxy_url)
        .with_context(|| format!("invalid SOCKS5 proxy URL: '{proxy_url}'"))?;

    match url.scheme() {
        "socks5" | "socks5h" => {}
        scheme => {
            bail!("invalid proxy scheme '{scheme}'; only socks5:// and socks5h:// are allowed")
        }
    }

    if url.host_str().is_none() {
        bail!("SOCKS5 proxy URL does not contain a host");
    }

    Proxy::all(url).context("failed to configure SOCKS5 proxy")
}

fn fetch_latest_release(client: &Client) -> Result<Release> {
    client
        .get(RELEASE_API_URL)
        .send()
        .context("failed to fetch the latest GitHub release")?
        .error_for_status()
        .context("GitHub API returned a non-success status code")?
        .json()
        .context("failed to read GitHub release response")
}

fn parse_release_version(release: &Release) -> Result<Version> {
    extract_semver(&release.name)
        .or_else(|| extract_semver(&release.tag_name))
        .with_context(|| {
            format!(
                "failed to parse a SemVer version from release '{}' / '{}'",
                release.name, release.tag_name
            )
        })
}

fn detect_installed_version(target_path: &Path) -> Result<Option<Version>> {
    if !target_path.exists() {
        return Ok(None);
    }

    let output = Command::new(target_path)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to execute '{}'", target_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "'{} --version' failed with status {}: {}",
            target_path.display(),
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stdout.trim().is_empty() {
        stderr.as_ref()
    } else {
        stdout.as_ref()
    };

    extract_semver(combined)
        .with_context(|| {
            format!(
                "failed to parse Codex version from '{}': {}",
                target_path.display(),
                combined.trim()
            )
        })
        .map(Some)
}

fn print_status(
    target_path: &Path,
    current: Option<&Version>,
    latest: &Version,
    release: &Release,
) {
    match current {
        Some(current) => println!(
            "Installed: {} ({}) | Available: {} | Source: {}",
            current,
            target_path.display(),
            latest,
            if release.html_url.is_empty() {
                REPOSITORY_URL
            } else {
                &release.html_url
            }
        ),
        None => println!(
            "Installed: not present ({}) | Available: {} | Source: {}",
            target_path.display(),
            latest,
            if release.html_url.is_empty() {
                REPOSITORY_URL
            } else {
                &release.html_url
            }
        ),
    }
}

fn ensure_root() -> Result<()> {
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        bail!("root privileges are required to install to /usr/local/bin; please run with sudo");
    }
    Ok(())
}

fn download_asset(client: &Client, asset: &Asset, workdir: &Path) -> Result<PathBuf> {
    if asset.size == 0 {
        bail!("GitHub reported an empty file for '{}'", asset.name);
    }
    if asset.size > MAX_DOWNLOAD_BYTES {
        bail!(
            "asset '{}' is {} bytes, which exceeds the safety limit of {} bytes",
            asset.name,
            asset.size,
            MAX_DOWNLOAD_BYTES
        );
    }

    let destination = workdir.join(&asset.name);
    let file = File::create(&destination).with_context(|| {
        format!(
            "failed to create temporary file '{}'",
            destination.display()
        )
    })?;
    let mut writer = BufWriter::new(file);
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .with_context(|| format!("failed to download '{}'", asset.browser_download_url))?
        .error_for_status()
        .context("download returned a non-success status code")?;

    if let Some(content_length) = response.content_length() {
        if content_length != asset.size {
            bail!(
                "GitHub reported mismatched sizes: API={} bytes, HTTP={} bytes",
                asset.size,
                content_length
            );
        }
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total_written = 0_u64;

    loop {
        let read = response
            .read(&mut buffer)
            .context("failed to read download")?;
        if read == 0 {
            break;
        }
        total_written += read as u64;
        if total_written > MAX_DOWNLOAD_BYTES {
            bail!(
                "download exceeds the safety limit of {} bytes",
                MAX_DOWNLOAD_BYTES
            );
        }
        writer
            .write_all(&buffer[..read])
            .context("failed to write download to temporary file")?;
        hasher.update(&buffer[..read]);
    }

    writer.flush().context("failed to flush download")?;
    writer
        .into_inner()
        .context("failed to release file handle after download")?
        .sync_all()
        .context("failed to sync downloaded file")?;

    if total_written != asset.size {
        bail!(
            "download size mismatch: expected {} bytes, received {} bytes",
            asset.size,
            total_written
        );
    }

    if let Some(expected_digest) = asset.digest.as_deref() {
        verify_sha256_digest(hasher.finalize(), expected_digest)?;
    }

    Ok(destination)
}

fn verify_sha256_digest(actual_digest: impl AsRef<[u8]>, expected: &str) -> Result<()> {
    let expected = expected
        .strip_prefix("sha256:")
        .unwrap_or(expected)
        .to_ascii_lowercase();
    let actual = to_lower_hex(actual_digest.as_ref());
    if actual != expected {
        bail!("SHA-256 digest mismatch");
    }
    Ok(())
}

fn extract_binary_from_archive(
    archive_path: &Path,
    asset_name: &str,
    install_name: &str,
    workdir: &Path,
) -> Result<PathBuf> {
    let mut archive = Archive::new(GzDecoder::new(
        File::open(archive_path)
            .with_context(|| format!("failed to open archive '{}'", archive_path.display()))?,
    ));
    let extracted_path = workdir.join(install_name);
    let mut found_binary = false;
    let archive_stem = archive_binary_name(asset_name);

    for entry in archive
        .entries()
        .context("failed to read archive contents")?
    {
        let mut entry = entry.context("failed to read archive entry")?;
        let entry_type = entry.header().entry_type();
        let entry_path = entry
            .path()
            .context("failed to read archive path")?
            .into_owned();
        let normalized_name = normalize_archive_entry_name(&entry_path)?;

        if !is_expected_archive_binary(&normalized_name, archive_stem.as_deref(), install_name) {
            bail!(
                "unexpected archive content: '{}' (expected only '{}')",
                entry_path.display(),
                archive_stem.as_deref().unwrap_or(install_name)
            );
        }

        if !entry_type.is_file() {
            bail!(
                "unexpected archive entry type for '{}': only regular files are allowed",
                entry_path.display()
            );
        }

        if found_binary {
            bail!("archive contains multiple matching binaries");
        }

        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&extracted_path)
            .with_context(|| format!("failed to create '{}'", extracted_path.display()))?;
        io::copy(&mut entry, &mut out).context("failed to extract binary from archive")?;
        out.sync_all().context("failed to sync extracted binary")?;
        found_binary = true;
    }

    if !found_binary {
        bail!("archive does not contain an installable binary");
    }

    fs::set_permissions(&extracted_path, fs::Permissions::from_mode(0o755)).with_context(|| {
        format!(
            "failed to set permissions on '{}'",
            extracted_path.display()
        )
    })?;

    Ok(extracted_path)
}

fn archive_binary_name(asset_name: &str) -> Option<&str> {
    asset_name.strip_suffix(".tar.gz")
}

fn normalize_archive_entry_name(entry_path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in entry_path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!(
                "archive contains an unsafe path: '{}'",
                entry_path.display()
            ),
        }
    }

    match parts.as_slice() {
        [name] => Ok(name.clone()),
        _ => bail!("archive path is not flat: '{}'", entry_path.display()),
    }
}

fn is_expected_archive_binary(
    entry_name: &str,
    archive_name: Option<&str>,
    install_name: &str,
) -> bool {
    entry_name == install_name || archive_name.is_some_and(|name| entry_name == name)
}

fn verify_extracted_binary(binary_path: &Path, expected_version: &Version) -> Result<()> {
    let metadata = fs::metadata(binary_path)
        .with_context(|| format!("failed to inspect '{}'", binary_path.display()))?;
    if !metadata.is_file() {
        bail!("extracted path '{}' is not a file", binary_path.display());
    }
    if metadata.len() == 0 {
        bail!("extracted binary '{}' is empty", binary_path.display());
    }

    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "failed to execute '{}' for verification",
                binary_path.display()
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "extracted binary could not be started ({}): {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = extract_semver(&stdout).with_context(|| {
        format!(
            "failed to parse version from extracted binary: {}",
            stdout.trim()
        )
    })?;
    if &version != expected_version {
        bail!(
            "extracted binary version mismatch: expected {}, got {}",
            expected_version,
            version
        );
    }

    Ok(())
}

fn install_binary_atomically(source: &Path, target: &Path) -> Result<()> {
    let target_dir = target
        .parent()
        .context("target path has no parent directory")?;
    fs::create_dir_all(target_dir).with_context(|| {
        format!(
            "failed to create target directory '{}'",
            target_dir.display()
        )
    })?;

    let temp_target = unique_temp_target(
        target_dir,
        target
            .file_name()
            .unwrap_or_else(|| OsStr::new(INSTALL_NAME)),
    )?;
    copy_with_permissions(source, temp_target.path())?;

    fs::rename(temp_target.path(), target).with_context(|| {
        format!(
            "failed to move '{}' to '{}'",
            temp_target.path().display(),
            target.display()
        )
    })?;
    sync_directory(target_dir)?;

    Ok(())
}

fn unique_temp_target(target_dir: &Path, file_name: &OsStr) -> Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix(&format!(
            ".{}.tmp.",
            Path::new(file_name)
                .file_name()
                .unwrap_or_else(|| OsStr::new(INSTALL_NAME))
                .to_string_lossy()
        ))
        .tempfile_in(target_dir)
        .with_context(|| {
            format!(
                "failed to create temporary target file in '{}'",
                target_dir.display()
            )
        })
}

fn copy_with_permissions(source: &Path, target: &Path) -> Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("failed to open '{}'", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(target)
        .with_context(|| format!("failed to write '{}'", target.display()))?;
    io::copy(&mut input, &mut output).context("failed to copy binary into target")?;
    output
        .sync_all()
        .with_context(|| format!("failed to sync '{}'", target.display()))?;
    fs::set_permissions(target, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to set permissions on '{}'", target.display()))?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("failed to open directory '{}'", path.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync directory '{}'", path.display()))
}

fn extract_semver(input: &str) -> Option<Version> {
    let indices: Vec<_> = input.char_indices().collect();
    for (pos, ch) in &indices {
        if !ch.is_ascii_digit() {
            continue;
        }

        let mut end = *pos;
        for (candidate_pos, candidate_ch) in input[*pos..].char_indices() {
            if is_semver_char(candidate_ch) {
                end = *pos + candidate_pos + candidate_ch.len_utf8();
            } else {
                break;
            }
        }

        if end <= *pos {
            continue;
        }

        if let Ok(version) = Version::parse(&input[*pos..end]) {
            return Some(version);
        }
    }

    None
}

fn is_semver_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+')
}

fn to_lower_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_version() {
        let version = extract_semver("codex-cli 0.106.0").unwrap();
        assert_eq!(version, Version::parse("0.106.0").unwrap());
    }

    #[test]
    fn parses_prerelease_version() {
        let version = extract_semver("rust-v0.107.0-alpha.8").unwrap();
        assert_eq!(version, Version::parse("0.107.0-alpha.8").unwrap());
    }

    #[test]
    fn rejects_non_flat_archive_paths() {
        assert!(normalize_archive_entry_name(Path::new("nested/codex")).is_err());
    }

    #[test]
    fn accepts_flat_archive_paths() {
        let name =
            normalize_archive_entry_name(Path::new("./codex-x86_64-unknown-linux-gnu")).unwrap();
        assert_eq!(name, "codex-x86_64-unknown-linux-gnu");
    }

    #[test]
    fn accepts_socks5_proxy_url() {
        build_socks5_proxy("socks5://127.0.0.1:1080").unwrap();
        build_socks5_proxy("socks5h://user:pass@proxy.example:1080").unwrap();
    }

    #[test]
    fn rejects_non_socks_proxy_url() {
        let error = build_socks5_proxy("http://127.0.0.1:8080").unwrap_err();
        assert!(error
            .to_string()
            .contains("only socks5:// and socks5h:// are allowed"));
    }

    #[test]
    fn rejects_socks_proxy_without_host() {
        let error = build_socks5_proxy("socks5://").unwrap_err();
        assert!(error.to_string().contains("does not contain a host"));
    }

    #[test]
    fn rejects_malformed_socks_proxy_url() {
        let error = build_socks5_proxy("socks5://:1080").unwrap_err();
        assert!(error.to_string().contains("invalid SOCKS5 proxy URL"));
    }
}
