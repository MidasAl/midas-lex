use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

type MidasLexResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const DEFAULT_RELEASE_REPO: &str = "MidasAl/midas-lex";
const XDG_INSTALL_DIR: &str = "midas-lex/verus";
const LEGACY_INSTALL_DIR: &str = ".midas-lex/verus";
const INSTALL_HOME_ENV: &str = "MIDAS_LEX_VERUS_HOME";
const RELEASE_REPO_ENV: &str = "MIDAS_LEX_VERUS_RELEASE_REPOSITORY";
const VERBOSE_ENV: &str = "MIDAS_LEX_VERUS_VERBOSE";
const LOG_ENV: &str = "MIDAS_LEX_VERUS_LOG";
const BACKGROUND_UPDATE_EXE_ENV: &str = "MIDAS_LEX_VERUS_WRAPPER_BACKGROUND_UPDATE_EXE";
const BACKGROUND_UPDATE_MARKER_ENV: &str = "MIDAS_LEX_VERUS_WRAPPER_BACKGROUND_UPDATE_MARKER";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_BINARY_BYTES: u64 = 200 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

fn main() -> ExitCode {
    init_logger();
    match run(env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(err) => {
            log::error!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<OsString>) -> MidasLexResult<ExitCode> {
    if args.is_empty() && is_background_update_child()? {
        run_background_update();
        return Ok(ExitCode::SUCCESS);
    }
    let config = Config::from_env()?;
    let target = Target::current()?;
    let install = InstallStore::new(config.install_home.clone());
    let (selector, real_args) = parse_version_selector(args)?;
    match selector {
        Some(selector) => {
            let selected = install.ensure_version(&config, &target, &selector)?;
            log_dispatch(&config, &selected.tag, &selected.bin_path);
            run_real_binary(&selected.bin_path, real_args)
        }
        None => match install.latest_installed(&target)? {
            Some(installed) => {
                log_dispatch(&config, &installed.tag, &installed.bin_path);
                run_real_binary_with_background_update(
                    &installed.bin_path,
                    real_args,
                    &target,
                    &config,
                )
            }
            None => {
                log::info!("no installed Midas Lex binary; downloading latest release");
                let bin = install.install_latest(&config, &target)?;
                let tag = bin_tag(&bin).unwrap_or_else(|| "unknown".to_string());
                log_dispatch(&config, &tag, &bin);
                run_real_binary(&bin, real_args)
            }
        },
    }
}

fn run_background_update() {
    let result = (|| -> MidasLexResult<()> {
        let config = Config::from_env()?;
        let target = Target::current()?;
        let install = InstallStore::new(config.install_home.clone());
        install.update_latest(&config, &target)
    })();
    if let Err(err) = result {
        log::warn!("automatic update check failed: {err}");
    }
}

fn maybe_spawn_background_update(target: &Target, config: &Config) -> MidasLexResult<()> {
    let exe = env::current_exe()?;
    let stamp = update_stamp_path(target, &exe)?;
    if !claim_update_timer(&stamp)? {
        return Ok(());
    }
    let marker = background_marker_path(target)?;
    let mut marker_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)?;
    marker_file.write_all(sha256_file(&exe)?.as_bytes())?;
    log::info!("checking for Midas Lex updates in the background");
    Command::new(&exe)
        .env(INSTALL_HOME_ENV, config.install_home.as_os_str())
        .env(RELEASE_REPO_ENV, &config.release_repo)
        .env(BACKGROUND_UPDATE_EXE_ENV, exe.as_os_str())
        .env(BACKGROUND_UPDATE_MARKER_ENV, marker.as_os_str())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok(())
}

fn is_background_update_child() -> MidasLexResult<bool> {
    is_background_update_marker(
        env::var_os(BACKGROUND_UPDATE_EXE_ENV),
        env::var_os(BACKGROUND_UPDATE_MARKER_ENV),
        &env::current_exe()?,
    )
}

fn is_background_update_marker(
    expected: Option<OsString>,
    marker: Option<OsString>,
    exe: &Path,
) -> MidasLexResult<bool> {
    let Some(expected) = expected else {
        return Ok(false);
    };
    let Some(marker) = marker else {
        return Ok(false);
    };
    let marker = PathBuf::from(marker);
    if Path::new(&expected) != exe {
        return Ok(false);
    }
    let marker_body = match fs::read(&marker) {
        Ok(body) => body,
        Err(_) => return Ok(false),
    };
    let _ = fs::remove_file(marker);
    Ok(marker_body == sha256_file(exe)?.as_bytes())
}

fn run_real_binary(bin: &Path, args: Vec<OsString>) -> MidasLexResult<ExitCode> {
    let status = Command::new(bin).args(args).status()?;
    Ok(exit_code_from_status(status))
}

fn run_real_binary_with_background_update(
    bin: &Path,
    args: Vec<OsString>,
    target: &Target,
    config: &Config,
) -> MidasLexResult<ExitCode> {
    let mut child = Command::new(bin).args(args).spawn()?;
    if let Err(err) = maybe_spawn_background_update(target, config) {
        log::warn!("automatic update check could not start: {err}");
    }
    let status = child.wait()?;
    Ok(exit_code_from_status(status))
}

fn init_logger() {
    let default_filter = if env_bool(VERBOSE_ENV) {
        "info"
    } else {
        "warn"
    };
    let env = env_logger::Env::new().filter_or(LOG_ENV, default_filter);
    let _ = env_logger::Builder::from_env(env)
        .format_timestamp(None)
        .format_target(false)
        .try_init();
}

fn log_dispatch(config: &Config, tag: &str, bin: &Path) {
    if config.verbose {
        log::info!("running Midas Lex {tag} from {}", bin.display());
    }
}

fn bin_tag(bin: &Path) -> Option<String> {
    bin.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(code) => ExitCode::from(code.clamp(0, 255) as u8),
        None => ExitCode::FAILURE,
    }
}

#[derive(Debug)]
struct Config {
    install_home: PathBuf,
    release_repo: String,
    verbose: bool,
}

impl Config {
    fn from_env() -> MidasLexResult<Self> {
        let install_home = match env::var_os(INSTALL_HOME_ENV) {
            Some(path) => PathBuf::from(path),
            None => default_install_home()?,
        };
        let release_repo =
            env::var(RELEASE_REPO_ENV).unwrap_or_else(|_| DEFAULT_RELEASE_REPO.to_string());
        let verbose = env_bool(VERBOSE_ENV);
        Ok(Self {
            install_home,
            release_repo,
            verbose,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    triple: &'static str,
    exe_name: &'static str,
}

impl Target {
    fn current() -> MidasLexResult<Self> {
        current_target().ok_or_else(|| error("unsupported Midas Lex platform"))
    }

    fn asset_name(&self, tag: &str) -> String {
        format!(
            "midas-lex-private-{tag}-{}{}",
            self.triple,
            self.exe_suffix()
        )
    }

    fn checksum_asset_name(&self, tag: &str) -> String {
        format!("{}.sha256", self.asset_name(tag))
    }

    fn exe_suffix(&self) -> &'static str {
        if self.exe_name.ends_with(".exe") {
            ".exe"
        } else {
            ""
        }
    }
}

fn current_target() -> Option<Target> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return Some(Target {
            triple: "x86_64-unknown-linux-musl",
            exe_name: "midas-lex",
        });
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        return Some(Target {
            triple: "aarch64-unknown-linux-musl",
            exe_name: "midas-lex",
        });
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Some(Target {
            triple: "x86_64-apple-darwin",
            exe_name: "midas-lex",
        });
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Some(Target {
            triple: "aarch64-apple-darwin",
            exe_name: "midas-lex",
        });
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return Some(Target {
            triple: "x86_64-pc-windows-msvc",
            exe_name: "midas-lex.exe",
        });
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        return Some(Target {
            triple: "aarch64-pc-windows-msvc",
            exe_name: "midas-lex.exe",
        });
    }
    #[allow(unreachable_code)]
    None
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum VersionSelector {
    Exact { tag: String, version: Version },
    Prerelease,
}

fn parse_version_selector(
    args: Vec<OsString>,
) -> MidasLexResult<(Option<VersionSelector>, Vec<OsString>)> {
    let Some(first) = args.first().and_then(|arg| arg.to_str()) else {
        return Ok((None, args));
    };
    if first == "+prerelease" {
        return Ok((
            Some(VersionSelector::Prerelease),
            args.into_iter().skip(1).collect(),
        ));
    }
    let Some(raw_version) = first.strip_prefix("+v") else {
        return Ok((None, args));
    };
    if raw_version.is_empty() {
        return Ok((None, args));
    }
    let Ok(version) = Version::parse(raw_version) else {
        return Ok((None, args));
    };
    Ok((
        Some(VersionSelector::Exact {
            tag: first[1..].to_string(),
            version,
        }),
        args.into_iter().skip(1).collect(),
    ))
}

fn parse_tag_version(tag: &str) -> MidasLexResult<Version> {
    let raw = tag.strip_prefix('v').unwrap_or(tag);
    Ok(Version::parse(raw)?)
}

#[derive(Debug)]
struct InstalledVersion {
    tag: String,
    version: Version,
    bin_path: PathBuf,
}

#[derive(Debug)]
struct InstallStore {
    root: PathBuf,
}

impl InstallStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn ensure_version(
        &self,
        config: &Config,
        target: &Target,
        selector: &VersionSelector,
    ) -> MidasLexResult<InstalledVersion> {
        match selector {
            VersionSelector::Exact { tag, version } => {
                if let Some(bin_path) = self.verified_bin(tag, target)? {
                    return Ok(InstalledVersion {
                        tag: tag.clone(),
                        version: version.clone(),
                        bin_path,
                    });
                }
                log::info!("downloading Midas Lex {tag}");
                let release =
                    ReleaseClient::new(config.release_repo.clone()).release_for_tag(tag)?;
                if parse_tag_version(&release.tag_name)? != *version {
                    return Err(error(format!(
                        "release tag {} did not match requested {}",
                        release.tag_name, tag
                    )));
                }
                let bin_path = self.install_release(&release, target)?;
                Ok(InstalledVersion {
                    tag: release.tag_name,
                    version: version.clone(),
                    bin_path,
                })
            }
            VersionSelector::Prerelease => {
                let release =
                    ReleaseClient::new(config.release_repo.clone()).latest_prerelease()?;
                let version = parse_tag_version(&release.tag_name)?;
                if let Some(bin_path) = self.verified_bin(&release.tag_name, target)? {
                    return Ok(InstalledVersion {
                        tag: release.tag_name,
                        version,
                        bin_path,
                    });
                }
                log::info!("downloading Midas Lex {}", release.tag_name);
                let bin_path = self.install_release(&release, target)?;
                Ok(InstalledVersion {
                    tag: release.tag_name,
                    version,
                    bin_path,
                })
            }
        }
    }

    fn install_latest(&self, config: &Config, target: &Target) -> MidasLexResult<PathBuf> {
        let release = ReleaseClient::new(config.release_repo.clone()).latest_release()?;
        self.install_release(&release, target)
    }

    fn update_latest(&self, config: &Config, target: &Target) -> MidasLexResult<()> {
        let Some(current) = self.latest_installed(target)? else {
            return Ok(());
        };
        let release = ReleaseClient::new(config.release_repo.clone()).latest_release()?;
        let remote_version = parse_tag_version(&release.tag_name)?;
        if remote_version <= current.version {
            log::info!("Midas Lex {} is already installed", current.tag);
            return Ok(());
        }
        log::info!(
            "downloading Midas Lex {} for the next invocation",
            release.tag_name
        );
        let installed = self.install_release(&release, target)?;
        log::info!("installed Midas Lex update at {}", installed.display());
        Ok(())
    }

    fn latest_installed(&self, target: &Target) -> MidasLexResult<Option<InstalledVersion>> {
        let dir = self.root.join("toolchains");
        if !dir.exists() {
            return Ok(None);
        }
        let mut stable_versions = Vec::new();
        let mut pre_versions = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let tag = entry.file_name().to_string_lossy().into_owned();
            let Ok(version) = parse_tag_version(&tag) else {
                continue;
            };
            let checksum_record = match fs::read_to_string(self.checksum_path(&tag, target)) {
                Ok(record) => record,
                Err(_) => continue,
            };
            if let Some(bin_path) = self.verified_bin(&tag, target)? {
                let installed = InstalledVersion {
                    tag,
                    version,
                    bin_path,
                };
                if is_pre_release(&installed.version)
                    || checksum_record_is_pre_release(&checksum_record)
                {
                    pre_versions.push(installed);
                } else {
                    stable_versions.push(installed);
                }
            }
        }
        stable_versions.sort_by(|left, right| left.version.cmp(&right.version));
        if stable_versions.is_empty() {
            pre_versions.sort_by(|left, right| left.version.cmp(&right.version));
            Ok(pre_versions.pop())
        } else {
            Ok(stable_versions.pop())
        }
    }

    fn install_release(&self, release: &Release, target: &Target) -> MidasLexResult<PathBuf> {
        let _lock = InstallLock::acquire(&self.root)?;
        if let Some(bin) = self.verified_bin(&release.tag_name, target)? {
            return Ok(bin);
        }
        let bin = self.bin_path(&release.tag_name, target);
        let asset_name = target.asset_name(&release.tag_name);
        let checksum_name = target.checksum_asset_name(&release.tag_name);
        let asset = release
            .asset(&asset_name)
            .ok_or_else(|| error(format!("release asset missing: {asset_name}")))?;
        let checksum_asset = release
            .asset(&checksum_name)
            .ok_or_else(|| error(format!("release checksum missing: {checksum_name}")))?;
        fs::create_dir_all(self.root.join("downloads"))?;
        let bin_parent = bin
            .parent()
            .ok_or_else(|| error("invalid Midas Lex install path"))?;
        fs::create_dir_all(bin_parent)?;
        let checksum_text = download_text(&checksum_asset.browser_download_url, MAX_TEXT_BYTES)?;
        let expected = parse_checksum(&checksum_text)?;
        let download = self.download_path(&asset_name);
        if download.exists() {
            fs::remove_file(&download)?;
        }
        download_file(&asset.browser_download_url, &download, MAX_BINARY_BYTES)?;
        let actual = sha256_file(&download)?;
        if actual != expected {
            let _ = fs::remove_file(&download);
            return Err(error(format!(
                "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
            )));
        }
        let tmp = bin.with_extension("download");
        if tmp.exists() {
            fs::remove_file(&tmp)?;
        }
        fs::rename(&download, &tmp)?;
        set_executable(&tmp)?;
        if bin.exists() {
            fs::remove_file(&bin)?;
        }
        fs::rename(&tmp, &bin)?;
        self.record_checksum(
            release,
            target,
            &asset_name,
            &expected,
            &asset.browser_download_url,
            &checksum_asset.browser_download_url,
        )?;
        log::info!(
            "installed Midas Lex {} for {}",
            release.tag_name,
            target.triple
        );
        Ok(bin)
    }

    fn bin_path(&self, tag: &str, target: &Target) -> PathBuf {
        self.root
            .join("toolchains")
            .join(tag)
            .join(target.triple)
            .join(target.exe_name)
    }

    fn checksum_path(&self, tag: &str, target: &Target) -> PathBuf {
        self.root
            .join("checksums")
            .join(tag)
            .join(format!("{}.sha256", target.triple))
    }

    fn download_path(&self, asset_name: &str) -> PathBuf {
        self.root
            .join("downloads")
            .join(format!("{asset_name}.{}.download", std::process::id()))
    }

    fn verified_bin(&self, tag: &str, target: &Target) -> MidasLexResult<Option<PathBuf>> {
        let bin = self.bin_path(tag, target);
        if !bin.is_file() {
            return Ok(None);
        }
        let checksum_path = self.checksum_path(tag, target);
        if !checksum_path.is_file() {
            return Ok(None);
        }
        let Ok(record) = fs::read_to_string(checksum_path) else {
            return Ok(None);
        };
        let Ok(expected) = parse_checksum(&record) else {
            return Ok(None);
        };
        let actual = sha256_file(&bin)?;
        if actual == expected {
            Ok(Some(bin))
        } else {
            Ok(None)
        }
    }

    fn record_checksum(
        &self,
        release: &Release,
        target: &Target,
        asset_name: &str,
        checksum: &str,
        asset_url: &str,
        checksum_url: &str,
    ) -> MidasLexResult<()> {
        let dir = self.root.join("checksums").join(&release.tag_name);
        fs::create_dir_all(&dir)?;
        let body = format!(
            "{checksum}  {asset_name}\nversion: {}\ntarget: {}\npre_release: {}\nasset_url: {}\nchecksum_url: {}\n",
            release.tag_name, target.triple, release.prerelease, asset_url, checksum_url
        );
        fs::write(dir.join(format!("{}.sha256", target.triple)), body)?;
        Ok(())
    }
}

struct InstallLock {
    path: PathBuf,
}

impl InstallLock {
    fn acquire(root: &Path) -> MidasLexResult<Self> {
        let lock_dir = root.join("locks");
        fs::create_dir_all(&lock_dir)?;
        let path = lock_dir.join("install.lock");
        for _ in 0..120 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_secs(1));
                }
                Err(err) => return Err(Box::new(err)),
            }
        }
        Err(error("timed out waiting for Midas Lex install lock"))
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

struct ReleaseClient {
    repo: String,
}

impl ReleaseClient {
    fn new(repo: String) -> Self {
        Self { repo }
    }

    fn latest_release(&self) -> MidasLexResult<Release> {
        let body = self.fetch_text("releases?per_page=100")?;
        latest_semver_release(
            serde_json::from_str(&body)?,
            ReleaseMode::StableThenPrerelease,
        )
    }

    fn latest_prerelease(&self) -> MidasLexResult<Release> {
        let body = self.fetch_text("releases?per_page=100")?;
        latest_semver_release(serde_json::from_str(&body)?, ReleaseMode::AllowPrerelease)
    }

    fn release_for_tag(&self, tag: &str) -> MidasLexResult<Release> {
        let body = self.fetch_text(&format!("releases/tags/{tag}"))?;
        Ok(serde_json::from_str(&body)?)
    }

    fn fetch_text(&self, path: &str) -> MidasLexResult<String> {
        let url = format!("https://api.github.com/repos/{}/{path}", self.repo);
        http_get_text(&url, MAX_TEXT_BYTES)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReleaseMode {
    StableThenPrerelease,
    AllowPrerelease,
}

fn latest_semver_release(releases: Vec<Release>, mode: ReleaseMode) -> MidasLexResult<Release> {
    let mut best_stable: Option<(Version, Release)> = None;
    let mut best_any: Option<(Version, Release)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Ok(version) = parse_tag_version(&release.tag_name) else {
            continue;
        };
        update_best(&mut best_any, version.clone(), release.clone());
        if !release.prerelease && !is_pre_release(&version) {
            update_best(&mut best_stable, version, release);
        }
    }
    let best = match mode {
        ReleaseMode::StableThenPrerelease => best_stable.or(best_any),
        ReleaseMode::AllowPrerelease => best_any,
    };
    best.map(|(_, release)| release)
        .ok_or_else(|| error("no semver Midas Lex releases found"))
}

fn update_best(best: &mut Option<(Version, Release)>, version: Version, release: Release) {
    match best {
        Some((best_version, _)) if version <= *best_version => {}
        _ => *best = Some((version, release)),
    }
}

fn is_pre_release(version: &Version) -> bool {
    !version.pre.is_empty()
}

fn http_get_text(url: &str, max_bytes: u64) -> MidasLexResult<String> {
    Ok(ureq::get(url)
        .header("User-Agent", "midas-lex-wrapper")
        .call()?
        .body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_string()?)
}

fn download_text(url: &str, max_bytes: u64) -> MidasLexResult<String> {
    if let Some(path) = file_url_path(url) {
        let bytes = fs::read(path)?;
        if bytes.len() as u64 > max_bytes {
            return Err(error("downloaded file exceeds size limit"));
        }
        return Ok(String::from_utf8(bytes)?);
    }
    http_get_text(url, max_bytes)
}

fn download_file(url: &str, dest: &Path, max_bytes: u64) -> MidasLexResult<()> {
    if let Some(path) = file_url_path(url) {
        return write_limited(File::open(path)?, dest, max_bytes);
    }
    let response = ureq::get(url)
        .header("User-Agent", "midas-lex-wrapper")
        .call()?;
    write_limited(response.into_body().into_reader(), dest, max_bytes)
}

fn write_limited(reader: impl Read, dest: &Path, max_bytes: u64) -> MidasLexResult<()> {
    let mut reader = reader.take(max_bytes.saturating_add(1));
    let mut file = File::create(dest)?;
    let copied = io::copy(&mut reader, &mut file)?;
    file.flush()?;
    if copied > max_bytes {
        let _ = fs::remove_file(dest);
        return Err(error("downloaded file exceeds size limit"));
    }
    Ok(())
}

fn file_url_path(url: &str) -> Option<PathBuf> {
    url.strip_prefix("file://").map(PathBuf::from)
}

fn parse_checksum(text: &str) -> MidasLexResult<String> {
    let token = text
        .split_whitespace()
        .next()
        .ok_or_else(|| error("empty checksum file"))?
        .to_ascii_lowercase();
    if token.len() != 64 || !token.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(error(
            "checksum file does not start with a SHA-256 hex digest",
        ));
    }
    Ok(token)
}

fn checksum_record_is_pre_release(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "pre_release: true")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_digest(digest)
}

fn sha256_file(path: &Path) -> MidasLexResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let n_bytes = file.read(&mut buffer)?;
        if n_bytes == 0 {
            break;
        }
        hasher.update(&buffer[..n_bytes]);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn hex_digest(digest: impl IntoIterator<Item = u8>) -> String {
    let mut out = String::with_capacity(64);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn update_stamp_path(target: &Target, exe: &Path) -> MidasLexResult<PathBuf> {
    let exe_hash = short_hash(sha256_file(exe)?.as_bytes());
    Ok(temp_state_dir()?.join(format!("update-{}-{}.stamp", target.triple, exe_hash)))
}

fn background_marker_path(target: &Target) -> MidasLexResult<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    Ok(temp_state_dir()?.join(format!(
        "update-{}-{}-{nanos}.marker",
        target.triple,
        std::process::id()
    )))
}

fn claim_update_timer(stamp: &Path) -> MidasLexResult<bool> {
    if let Ok(metadata) = fs::symlink_metadata(stamp)
        && let Ok(modified) = metadata.modified()
        && metadata.file_type().is_file()
        && SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO)
            < UPDATE_CHECK_INTERVAL
    {
        return Ok(false);
    }
    match OpenOptions::new().write(true).create_new(true).open(stamp) {
        Ok(mut file) => file.write_all(b"")?,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(stamp)?;
            if !metadata.file_type().is_file() {
                return Err(error("update timer path is not a regular file"));
            }
            let mut file = OpenOptions::new().write(true).truncate(true).open(stamp)?;
            file.write_all(b"")?;
        }
        Err(err) => return Err(Box::new(err)),
    }
    Ok(true)
}

#[cfg(unix)]
fn temp_state_dir() -> MidasLexResult<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let path = PathBuf::from("/tmp").join(format!("midas-lex-wrapper-uid-{uid}"));
    ensure_private_temp_dir(&path)?;
    Ok(path)
}

#[cfg(not(unix))]
fn temp_state_dir() -> MidasLexResult<PathBuf> {
    let user = env::var_os("USER")
        .or_else(|| env::var_os("USERNAME"))
        .map(|value| short_hash(value.as_encoded_bytes()))
        .unwrap_or_else(|| "unknown".to_string());
    let path = env::temp_dir().join(format!("midas-lex-wrapper-{user}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn ensure_private_temp_dir(path: &Path) -> MidasLexResult<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err(error("temp state path is not a directory"));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
        }
        Err(err) => return Err(Box::new(err)),
    }
    let metadata = fs::symlink_metadata(path)?;
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid {
        return Err(error(
            "temp state directory is not owned by the current user",
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode != 0o700 {
        fs::set_permissions(path, PermissionsExt::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(error("temp state directory is not private"));
    }
    Ok(())
}

fn short_hash(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..16].to_string()
}

fn default_install_home() -> MidasLexResult<PathBuf> {
    default_install_home_from_env(env::var_os("XDG_DATA_HOME"), env::var_os("HOME"))
}

#[cfg(not(windows))]
fn default_install_home_from_env(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
) -> MidasLexResult<PathBuf> {
    if let Some(path) = xdg_data_home
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path).join(XDG_INSTALL_DIR));
    }
    if let Some(home) = home
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home).join(LEGACY_INSTALL_DIR));
    }
    Err(error(format!(
        "cannot determine data directory; set {INSTALL_HOME_ENV}, XDG_DATA_HOME, or HOME"
    )))
}

#[cfg(windows)]
fn default_install_home_from_env(
    _xdg_data_home: Option<OsString>,
    _home: Option<OsString>,
) -> MidasLexResult<PathBuf> {
    Ok(home_dir()?.join(LEGACY_INSTALL_DIR))
}

#[cfg(windows)]
fn home_dir() -> MidasLexResult<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Some(profile) = env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) => {
            let mut home = drive;
            home.push(path);
            Ok(PathBuf::from(home))
        }
        _ => Err(error(format!(
            "cannot determine home directory; set {INSTALL_HOME_ENV}"
        ))),
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> MidasLexResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> MidasLexResult<()> {
    Ok(())
}

fn error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(io::Error::other(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn test_target() -> Target {
        Target {
            triple: "x86_64-unknown-linux-musl",
            exe_name: "midas-lex",
        }
    }

    #[test]
    fn parses_plus_version_selector() {
        let args = vec![
            OsString::from("+v0.0.1-alpha.1"),
            OsString::from("docs"),
            OsString::from("read"),
            OsString::from("helper_step_protocol"),
        ];
        let (selector, remaining) = parse_version_selector(args).unwrap();
        let VersionSelector::Exact { tag, version } = selector.unwrap() else {
            panic!("expected exact selector");
        };
        assert_eq!(tag, "v0.0.1-alpha.1");
        assert_eq!(version, Version::parse("0.0.1-alpha.1").unwrap());
        assert_eq!(
            remaining,
            vec![
                OsString::from("docs"),
                OsString::from("read"),
                OsString::from("helper_step_protocol")
            ]
        );
    }

    #[test]
    fn keeps_plus_semver_selector_without_v_unchanged() {
        let args = vec![OsString::from("+0.0.2"), OsString::from("docs")];
        let (selector, remaining) = parse_version_selector(args.clone()).unwrap();
        assert!(selector.is_none());
        assert_eq!(remaining, args);
    }

    #[test]
    fn parses_plus_prerelease_selector() {
        let args = vec![OsString::from("+prerelease"), OsString::from("docs")];
        let (selector, remaining) = parse_version_selector(args).unwrap();
        assert_eq!(selector, Some(VersionSelector::Prerelease));
        assert_eq!(remaining, vec![OsString::from("docs")]);
    }

    #[test]
    fn keeps_normal_args_unchanged() {
        let args = vec![
            OsString::from("docs"),
            OsString::from("read"),
            OsString::from("helper_step_protocol"),
        ];
        let (selector, remaining) = parse_version_selector(args.clone()).unwrap();
        assert!(selector.is_none());
        assert_eq!(remaining, args);
    }

    #[test]
    fn keeps_background_update_like_arg_unchanged() {
        let args = vec![OsString::from("--midas-lex-wrapper-background-update")];
        let (selector, remaining) = parse_version_selector(args.clone()).unwrap();
        assert!(selector.is_none());
        assert_eq!(remaining, args);
    }

    #[test]
    fn keeps_bare_plus_arg_unchanged() {
        let args = vec![OsString::from("+"), OsString::from("docs")];
        let (selector, remaining) = parse_version_selector(args.clone()).unwrap();
        assert!(selector.is_none());
        assert_eq!(remaining, args);
    }

    #[test]
    fn background_marker_requires_marker_file() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("midas-lex");
        assert!(
            !is_background_update_marker(Some(exe.clone().into_os_string()), None, &exe).unwrap()
        );
    }

    #[test]
    fn background_marker_is_one_use() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("midas-lex");
        let marker = dir.path().join("marker");
        fs::write(&exe, b"wrapper exe").unwrap();
        fs::write(&marker, sha256_file(&exe).unwrap()).unwrap();
        assert!(
            is_background_update_marker(
                Some(exe.clone().into_os_string()),
                Some(marker.clone().into_os_string()),
                &exe
            )
            .unwrap()
        );
        assert!(!marker.exists());
    }

    #[test]
    fn background_marker_rejects_path_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("midas-lex");
        let marker = dir.path().join("marker");
        fs::write(&exe, b"wrapper exe").unwrap();
        fs::write(&marker, exe.as_os_str().as_encoded_bytes()).unwrap();
        assert!(
            !is_background_update_marker(
                Some(exe.clone().into_os_string()),
                Some(marker.clone().into_os_string()),
                &exe
            )
            .unwrap()
        );
    }

    #[test]
    fn formats_asset_names() {
        let target = test_target();
        assert_eq!(
            target.asset_name("v0.0.1-alpha.1"),
            "midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl"
        );
        assert_eq!(
            target.checksum_asset_name("v0.0.1-alpha.1"),
            "midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl.sha256"
        );
    }

    #[test]
    fn parses_checksum_prefix() {
        let checksum = "A".repeat(64);
        let text =
            format!("{checksum}  midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl\n");
        assert_eq!(parse_checksum(&text).unwrap(), "a".repeat(64));
    }

    #[test]
    fn latest_installed_uses_ordinary_semver_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        for tag in ["v0.9.0", "v0.10.0-alpha.1", "v0.10.0"] {
            write_cached_install(&store, &target, tag, tag.as_bytes(), tag.as_bytes());
        }
        let latest = store.latest_installed(&target).unwrap().unwrap();
        assert_eq!(latest.tag, "v0.10.0");
    }

    #[test]
    fn latest_installed_falls_back_to_pre_releases() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install(
            &store,
            &target,
            "v0.10.0-alpha.1",
            b"valid-pre",
            b"valid-pre",
        );
        let latest = store.latest_installed(&target).unwrap().unwrap();
        assert_eq!(latest.tag, "v0.10.0-alpha.1");
    }

    #[test]
    fn latest_installed_prefers_stable_over_cached_github_pre_releases() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install(&store, &target, "v0.9.0", b"valid-old", b"valid-old");
        write_cached_install_with_pre_release(
            &store,
            &target,
            "v1.0.0",
            b"valid-pre",
            b"valid-pre",
            true,
        );
        let latest = store.latest_installed(&target).unwrap().unwrap();
        assert_eq!(latest.tag, "v0.9.0");
    }

    #[test]
    fn latest_installed_falls_back_to_cached_github_pre_releases() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install_with_pre_release(
            &store,
            &target,
            "v1.0.0",
            b"valid-pre",
            b"valid-pre",
            true,
        );
        let latest = store.latest_installed(&target).unwrap().unwrap();
        assert_eq!(latest.tag, "v1.0.0");
    }

    #[test]
    fn latest_installed_requires_checksum_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        let bin = store.bin_path("v0.0.1-alpha.1", &target);
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(bin, b"unchecked").unwrap();
        assert!(store.latest_installed(&target).unwrap().is_none());
    }

    #[test]
    fn latest_installed_skips_tampered_newer_binary() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install(&store, &target, "v0.9.0", b"valid-old", b"valid-old");
        write_cached_install(&store, &target, "v0.10.0", b"tampered-new", b"original-new");
        let latest = store.latest_installed(&target).unwrap().unwrap();
        assert_eq!(latest.tag, "v0.9.0");
    }

    #[test]
    fn latest_installed_skips_corrupt_checksum_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install(&store, &target, "v0.9.0", b"valid-old", b"valid-old");
        let bin = store.bin_path("v0.10.0", &target);
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"new").unwrap();
        let checksum_path = store.checksum_path("v0.10.0", &target);
        fs::create_dir_all(checksum_path.parent().unwrap()).unwrap();
        fs::write(checksum_path, b"not a checksum").unwrap();
        let latest = store.latest_installed(&target).unwrap().unwrap();
        assert_eq!(latest.tag, "v0.9.0");
    }

    #[test]
    fn install_release_verifies_checksum_and_records_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let binary_path = source_dir
            .path()
            .join("midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl");
        let checksum_path = source_dir
            .path()
            .join("midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl.sha256");
        fs::write(&binary_path, b"fake binary").unwrap();
        fs::write(
            &checksum_path,
            format!(
                "{}  {}\n",
                sha256_hex(b"fake binary"),
                binary_path.display()
            ),
        )
        .unwrap();
        let release = Release {
            tag_name: "v0.0.1-alpha.1".to_string(),
            draft: false,
            prerelease: true,
            assets: vec![
                ReleaseAsset {
                    name: "midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl".to_string(),
                    browser_download_url: format!("file://{}", binary_path.display()),
                },
                ReleaseAsset {
                    name: "midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl.sha256"
                        .to_string(),
                    browser_download_url: format!("file://{}", checksum_path.display()),
                },
            ],
        };
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        let installed = store.install_release(&release, &target).unwrap();
        assert_eq!(
            installed,
            dir.path()
                .join("toolchains/v0.0.1-alpha.1/x86_64-unknown-linux-musl/midas-lex")
        );
        assert_eq!(fs::read(&installed).unwrap(), b"fake binary");
        assert!(
            dir.path()
                .join("checksums/v0.0.1-alpha.1/x86_64-unknown-linux-musl.sha256")
                .is_file()
        );
        let record = fs::read_to_string(
            dir.path()
                .join("checksums/v0.0.1-alpha.1/x86_64-unknown-linux-musl.sha256"),
        )
        .unwrap();
        assert!(record.contains("pre_release: true"));
        assert!(record.contains("asset_url: file://"));
        assert!(record.contains("checksum_url: file://"));
    }

    #[test]
    fn latest_semver_release_skips_pre_releases_and_drafts() {
        let latest = latest_semver_release(
            vec![
                test_release("not-a-version", false),
                test_release("v9.0.0", true),
                test_release("v0.0.1-alpha.1", false),
                test_release("v0.0.0", false),
            ],
            ReleaseMode::StableThenPrerelease,
        )
        .unwrap();
        assert_eq!(latest.tag_name, "v0.0.0");
    }

    #[test]
    fn latest_semver_release_skips_github_pre_releases_with_ordinary_tags() {
        let latest = latest_semver_release(
            vec![test_release("v0.9.0", false), test_pre_release("v1.0.0")],
            ReleaseMode::StableThenPrerelease,
        )
        .unwrap();
        assert_eq!(latest.tag_name, "v0.9.0");
    }

    #[test]
    fn latest_semver_release_can_include_pre_releases() {
        let latest = latest_semver_release(
            vec![
                test_release("v0.9.0", false),
                test_release("v1.0.0-alpha.1", false),
                test_pre_release("v1.0.0-beta.1"),
            ],
            ReleaseMode::AllowPrerelease,
        )
        .unwrap();
        assert_eq!(latest.tag_name, "v1.0.0-beta.1");
    }

    #[test]
    fn latest_semver_release_falls_back_to_pre_release() {
        let latest = latest_semver_release(
            vec![
                test_release("not-a-version", false),
                test_release("v9.0.0", true),
                test_release("v0.0.1-alpha.1", false),
                test_pre_release("v0.0.2-beta.1"),
            ],
            ReleaseMode::StableThenPrerelease,
        )
        .unwrap();
        assert_eq!(latest.tag_name, "v0.0.2-beta.1");
    }

    #[test]
    fn update_timer_uses_stamp_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = dir.path().join("update.stamp");
        assert!(claim_update_timer(&stamp).unwrap());
        assert!(!claim_update_timer(&stamp).unwrap());
    }

    #[test]
    fn update_stamp_uses_private_temp_subdir() {
        let target = test_target();
        let exe = env::current_exe().unwrap();
        let stamp = update_stamp_path(&target, &exe).unwrap();
        #[cfg(unix)]
        assert_eq!(
            stamp.parent().unwrap(),
            Path::new("/tmp").join(format!("midas-lex-wrapper-uid-{}", unsafe {
                libc::geteuid()
            }))
        );
        assert!(
            stamp
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("midas-lex-wrapper-")
        );
        assert!(
            stamp
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(target.triple)
        );
        assert!(
            stamp
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&short_hash(sha256_file(&exe).unwrap().as_bytes()))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn default_install_home_uses_xdg_data_home() {
        let path = default_install_home_from_env(
            Some(OsString::from("/tmp/midas-xdg")),
            Some(OsString::from("/tmp/home")),
        )
        .unwrap();
        assert_eq!(path, Path::new("/tmp/midas-xdg").join("midas-lex/verus"));
    }

    #[cfg(not(windows))]
    #[test]
    fn default_install_home_falls_back_to_legacy_home_dir() {
        let path = default_install_home_from_env(None, Some(OsString::from("/tmp/home"))).unwrap();
        assert_eq!(path, Path::new("/tmp/home").join(".midas-lex/verus"));
    }

    #[cfg(unix)]
    #[test]
    fn private_temp_dir_uses_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        ensure_private_temp_dir(&path).unwrap();
        let mode = fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn private_temp_dir_rejects_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        fs::create_dir(&real).unwrap();
        symlink(&real, &link).unwrap();
        let err = ensure_private_temp_dir(&link).unwrap_err();
        assert!(err.to_string().contains("not a directory"));
    }

    #[cfg(unix)]
    #[test]
    fn update_timer_rejects_symlink_stamp() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let stamp = dir.path().join("stamp");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, &stamp).unwrap();
        let err = claim_update_timer(&stamp).unwrap_err();
        assert!(err.to_string().contains("regular file"));
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn install_lock_removes_lock_file_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _lock = InstallLock::acquire(dir.path()).unwrap();
            assert!(dir.path().join("locks/install.lock").is_file());
        }
        assert!(!dir.path().join("locks/install.lock").exists());
    }

    #[test]
    fn download_file_url_enforces_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload");
        let dest = dir.path().join("dest");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"abcd").unwrap();
        let err = download_file(&format!("file://{}", path.display()), &dest, 3).unwrap_err();
        assert!(err.to_string().contains("size limit"));
        assert!(!dest.exists());
    }

    fn write_cached_install(
        store: &InstallStore,
        target: &Target,
        tag: &str,
        binary: &[u8],
        checksum_source: &[u8],
    ) {
        write_cached_install_with_pre_release(store, target, tag, binary, checksum_source, false);
    }

    fn write_cached_install_with_pre_release(
        store: &InstallStore,
        target: &Target,
        tag: &str,
        binary: &[u8],
        checksum_source: &[u8],
        pre_release: bool,
    ) {
        let bin = store.bin_path(tag, target);
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, binary).unwrap();
        let checksum_path = store.checksum_path(tag, target);
        fs::create_dir_all(checksum_path.parent().unwrap()).unwrap();
        fs::write(
            checksum_path,
            format!(
                "{}  {}\npre_release: {}\n",
                sha256_hex(checksum_source),
                target.asset_name(tag),
                pre_release
            ),
        )
        .unwrap();
    }

    fn test_release(tag: &str, draft: bool) -> Release {
        Release {
            tag_name: tag.to_string(),
            draft,
            prerelease: false,
            assets: Vec::new(),
        }
    }

    fn test_pre_release(tag: &str) -> Release {
        Release {
            tag_name: tag.to_string(),
            draft: false,
            prerelease: true,
            assets: Vec::new(),
        }
    }
}
