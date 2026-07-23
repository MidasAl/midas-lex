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
#[cfg(not(windows))]
const XDG_INSTALL_DIR: &str = "midas-lex/verus";
const LEGACY_INSTALL_DIR: &str = ".midas-lex/verus";
const INSTALL_HOME_ENV: &str = "MIDAS_LEX_VERUS_HOME";
const RELEASE_REPO_ENV: &str = "MIDAS_LEX_VERUS_RELEASE_REPOSITORY";
const VERBOSE_ENV: &str = "MIDAS_LEX_VERUS_VERBOSE";
const LOG_ENV: &str = "MIDAS_LEX_VERUS_LOG";
const CONFIG_FILE: &str = "config.toml";
const CARGO_METADATA_KEY: &str = "midas_lex";
const BACKGROUND_UPDATE_EXE_ENV: &str = "MIDAS_LEX_VERUS_WRAPPER_BACKGROUND_UPDATE_EXE";
const BACKGROUND_UPDATE_MARKER_ENV: &str = "MIDAS_LEX_VERUS_WRAPPER_BACKGROUND_UPDATE_MARKER";
const WRAPPER_UPDATE_LOCK_FILE: &str = ".midas-lex-self-update.lock";
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
    let (cli_selector, real_args) = parse_version_selector(args)?;
    let project_version_preference = if cli_selector.is_some() {
        None
    } else {
        load_cargo_version_preference()?
    };
    let selector = resolve_version_selector(
        cli_selector,
        project_version_preference,
        config.global_version_preference.clone(),
    );
    let installed_default = if selector.is_none() {
        install.latest_installed(&target)?
    } else {
        None
    };
    let update_policy = automatic_update_policy(selector.is_some(), installed_default.is_some());
    let (tag, bin_path) = match (selector, installed_default) {
        (Some(selector), None) => {
            let selected = install.ensure_version(&config, &target, &selector)?;
            (selected.tag, selected.bin_path)
        }
        (None, Some(installed)) => (installed.tag, installed.bin_path),
        (None, None) => {
            log::info!("no installed Midas Lex binary; downloading latest release");
            let bin = install.install_latest(&config, &target)?;
            let tag = bin_tag(&bin).unwrap_or_else(|| "unknown".to_string());
            (tag, bin)
        }
        (Some(_), Some(_)) => unreachable!("explicit selectors do not resolve default runtimes"),
    };
    log_dispatch(&config, &tag, &bin_path);
    match update_policy {
        AutomaticUpdatePolicy::AfterRuntimeStart => {
            run_real_binary_with_background_update(&bin_path, real_args, &target, &config)
        }
        AutomaticUpdatePolicy::SkipFirstRun | AutomaticUpdatePolicy::SkipExplicitSelector => {
            run_real_binary(&bin_path, real_args)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutomaticUpdatePolicy {
    AfterRuntimeStart,
    SkipFirstRun,
    SkipExplicitSelector,
}

fn automatic_update_policy(
    has_selector: bool,
    has_installed_default: bool,
) -> AutomaticUpdatePolicy {
    if has_selector {
        AutomaticUpdatePolicy::SkipExplicitSelector
    } else if has_installed_default {
        AutomaticUpdatePolicy::AfterRuntimeStart
    } else {
        AutomaticUpdatePolicy::SkipFirstRun
    }
}

fn run_background_update() {
    let result = (|| -> MidasLexResult<()> {
        let config = Config::from_env()?;
        let target = Target::current()?;
        let install = InstallStore::new(config.install_home.clone());
        let release = ReleaseClient::new(config.release_repo.clone()).latest_release()?;
        let results = apply_background_release(&install, &release, &target, |release, target| {
            update_current_wrapper_from_release(release, target, &config.release_repo)
        });
        log_background_update_results(results);
        Ok(())
    })();
    if let Err(err) = result {
        log::warn!("automatic update check failed: {err}");
    }
}

struct BackgroundUpdateResults {
    wrapper: MidasLexResult<WrapperUpdateStatus>,
    runtime: MidasLexResult<()>,
}

fn apply_background_release(
    install: &InstallStore,
    release: &Release,
    target: &Target,
    update_wrapper: impl FnOnce(&Release, &Target) -> MidasLexResult<WrapperUpdateStatus>,
) -> BackgroundUpdateResults {
    let wrapper = update_wrapper(release, target);
    let runtime = install.update_release(release, target);
    BackgroundUpdateResults { wrapper, runtime }
}

fn log_background_update_results(results: BackgroundUpdateResults) {
    match results.wrapper {
        Ok(WrapperUpdateStatus::Current { version }) => {
            log::info!("Midas Lex wrapper v{version} is current");
        }
        Ok(WrapperUpdateStatus::Updated { from, to }) => {
            log::info!("updated Midas Lex wrapper from v{from} to {to}");
        }
        Ok(WrapperUpdateStatus::WindowsReinstallRequired {
            current_exe,
            release_tag,
        }) => {
            log::warn!("{}", windows_reinstall_notice(&current_exe, &release_tag));
        }
        Ok(WrapperUpdateStatus::SkippedUnofficialRepository) => {
            log::info!(
                "automatic wrapper update skipped because {RELEASE_REPO_ENV} does not name {DEFAULT_RELEASE_REPO}"
            );
        }
        Err(err) => log::warn!("automatic wrapper update failed: {err}"),
    }
    if let Err(err) = results.runtime {
        log::warn!("automatic runtime update failed: {err}");
    }
}

fn windows_reinstall_notice(current_exe: &Path, release_tag: &str) -> String {
    format!(
        "a newer Midas Lex wrapper {release_tag} is available, but the running Windows executable `{}` cannot be replaced safely; after Midas Lex exits, run `cargo install midas-lex --force`",
        current_exe.display()
    )
}

fn maybe_spawn_background_update(target: &Target, config: &Config) -> MidasLexResult<()> {
    let exe = env::current_exe()?;
    let stamp = update_stamp_path(target)?;
    if !claim_update_timer(&stamp)? {
        return Ok(());
    }
    let marker = background_marker_path(target)?;
    let mut marker_cleanup = RemoveFileOnDrop::new(marker.clone());
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
    marker_cleanup.disarm();
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
    run_real_binary_with_update_start(bin, args, || maybe_spawn_background_update(target, config))
}

fn run_real_binary_with_update_start(
    bin: &Path,
    args: Vec<OsString>,
    start_update: impl FnOnce() -> MidasLexResult<()>,
) -> MidasLexResult<ExitCode> {
    let mut child = Command::new(bin).args(args).spawn()?;
    if let Err(err) = start_update() {
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
    global_version_preference: Option<VersionPreference>,
}

impl Config {
    fn from_env() -> MidasLexResult<Self> {
        let install_home = match env::var_os(INSTALL_HOME_ENV) {
            Some(path) => PathBuf::from(path),
            None => default_install_home()?,
        };
        let release_repo = configured_release_repo();
        let verbose = env_bool(VERBOSE_ENV);
        let global_version_preference = load_global_version_preference(&install_home)?;
        Ok(Self {
            install_home,
            release_repo,
            verbose,
            global_version_preference,
        })
    }
}

fn configured_release_repo() -> String {
    env::var(RELEASE_REPO_ENV).unwrap_or_else(|_| DEFAULT_RELEASE_REPO.to_string())
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionSettings {
    version: Option<String>,
    prerelease: Option<bool>,
}

impl VersionSettings {
    fn preference(&self, source: &str) -> MidasLexResult<Option<VersionPreference>> {
        if self.version.is_some() && self.prerelease == Some(true) {
            return Err(error(format!(
                "invalid Midas Lex version selection in {source}: `version` and `prerelease = true` cannot be combined"
            )));
        }
        if let Some(raw) = &self.version {
            let raw = raw.strip_prefix('v').unwrap_or(raw);
            let version = Version::parse(raw)
                .map_err(|err| error(format!("invalid Midas Lex `version` in {source}: {err}")))?;
            return Ok(Some(VersionPreference::Exact {
                tag: format!("v{version}"),
                version,
            }));
        }
        Ok(self.prerelease.map(|enabled| {
            if enabled {
                VersionPreference::Prerelease
            } else {
                VersionPreference::Default
            }
        }))
    }
}

fn load_global_version_preference(
    install_home: &Path,
) -> MidasLexResult<Option<VersionPreference>> {
    let path = install_home.join(CONFIG_FILE);
    let source = match fs::read(&path) {
        Ok(source) => source,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(error(format!(
                "failed to read Midas Lex config {}: {err}",
                path.display()
            )));
        }
    };
    if source.len() as u64 > MAX_TEXT_BYTES {
        return Err(error(format!(
            "Midas Lex config {} exceeds {MAX_TEXT_BYTES} bytes",
            path.display()
        )));
    }
    let source = String::from_utf8(source).map_err(|err| {
        error(format!(
            "Midas Lex config {} is not UTF-8: {err}",
            path.display()
        ))
    })?;
    let settings: VersionSettings = toml::from_str(&source).map_err(|err| {
        error(format!(
            "failed to parse Midas Lex config {}: {err}",
            path.display()
        ))
    })?;
    settings.preference(&format!("`{}`", path.display()))
}

#[derive(Deserialize)]
struct CargoProjectMetadata {
    #[serde(default)]
    workspace_metadata: serde_json::Value,
    #[serde(default)]
    packages: Vec<CargoPackageMetadata>,
}

#[derive(Deserialize)]
struct CargoPackageMetadata {
    manifest_path: PathBuf,
    #[serde(default)]
    metadata: serde_json::Value,
}

fn load_cargo_version_preference() -> MidasLexResult<Option<VersionPreference>> {
    let output = match Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            log::info!(
                "ignoring Cargo.toml version selection: failed to run `cargo metadata`: {err}"
            );
            return Ok(None);
        }
    };
    if !output.status.success() {
        log::info!(
            "ignoring Cargo.toml version selection because `cargo metadata` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(None);
    }
    let metadata: CargoProjectMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|err| error(format!("invalid `cargo metadata` output: {err}")))?;
    let output = match Command::new("cargo")
        .args(["locate-project", "--message-format", "plain"])
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            log::info!(
                "ignoring Cargo.toml version selection: failed to run `cargo locate-project`: {err}"
            );
            return Ok(None);
        }
    };
    if !output.status.success() {
        log::info!(
            "ignoring Cargo.toml version selection because `cargo locate-project` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(None);
    }
    let manifest_path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    version_preference_from_cargo_metadata(&metadata, &manifest_path)
}

fn version_preference_from_cargo_metadata(
    metadata: &CargoProjectMetadata,
    manifest_path: &Path,
) -> MidasLexResult<Option<VersionPreference>> {
    if let Some(settings) = version_settings_from_metadata(&metadata.workspace_metadata)? {
        return settings.preference("`workspace.metadata.midas_lex`");
    }
    let Some(package) = metadata
        .packages
        .iter()
        .find(|package| package.manifest_path == manifest_path)
    else {
        return Ok(None);
    };
    let Some(settings) = version_settings_from_metadata(&package.metadata)? else {
        return Ok(None);
    };
    settings.preference("`package.metadata.midas_lex`")
}

fn version_settings_from_metadata(
    metadata: &serde_json::Value,
) -> MidasLexResult<Option<VersionSettings>> {
    let Some(settings) = metadata.get(CARGO_METADATA_KEY) else {
        return Ok(None);
    };
    serde_json::from_value(settings.clone())
        .map(Some)
        .map_err(|err| error(format!("invalid Cargo.toml `midas_lex` metadata: {err}")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    triple: &'static str,
    exe_name: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WrapperUpdateSupport {
    AtomicReplace,
    RunningWindowsExe,
}

impl Target {
    fn current() -> MidasLexResult<Self> {
        current_target().ok_or_else(|| error("unsupported Midas Lex platform"))
    }

    fn runtime_asset_name(&self, tag: &str) -> String {
        format!(
            "midas-lex-private-{tag}-{}{}",
            self.triple,
            self.exe_suffix()
        )
    }

    fn runtime_checksum_asset_name(&self, tag: &str) -> String {
        format!("{}.sha256", self.runtime_asset_name(tag))
    }

    fn wrapper_asset_name(&self, tag: &str) -> String {
        format!("midas-lex-{tag}-{}{}", self.triple, self.exe_suffix())
    }

    fn wrapper_checksum_asset_name(&self, tag: &str) -> String {
        format!("{}.sha256", self.wrapper_asset_name(tag))
    }

    fn wrapper_update_support(&self) -> MidasLexResult<WrapperUpdateSupport> {
        match self.triple {
            "x86_64-unknown-linux-musl"
            | "aarch64-unknown-linux-musl"
            | "x86_64-apple-darwin"
            | "aarch64-apple-darwin" => Ok(WrapperUpdateSupport::AtomicReplace),
            "x86_64-pc-windows-msvc" | "aarch64-pc-windows-msvc" => {
                Ok(WrapperUpdateSupport::RunningWindowsExe)
            }
            _ => Err(error(format!(
                "automatic wrapper update is unsupported for target {}",
                self.triple
            ))),
        }
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum VersionPreference {
    Default,
    Exact { tag: String, version: Version },
    Prerelease,
}

impl VersionPreference {
    fn into_selector(self) -> Option<VersionSelector> {
        match self {
            Self::Default => None,
            Self::Exact { tag, version } => Some(VersionSelector::Exact { tag, version }),
            Self::Prerelease => Some(VersionSelector::Prerelease),
        }
    }
}

fn resolve_version_selector(
    cli: Option<VersionSelector>,
    project: Option<VersionPreference>,
    global: Option<VersionPreference>,
) -> Option<VersionSelector> {
    match cli {
        Some(selector) => Some(selector),
        None => project
            .or(global)
            .and_then(VersionPreference::into_selector),
    }
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

    fn update_release(&self, release: &Release, target: &Target) -> MidasLexResult<()> {
        let Some(current) = self.latest_installed(target)? else {
            return Ok(());
        };
        let remote_version = parse_tag_version(&release.tag_name)?;
        if remote_version <= current.version {
            log::info!("Midas Lex {} is already installed", current.tag);
            return Ok(());
        }
        log::info!(
            "downloading Midas Lex {} for the next invocation",
            release.tag_name
        );
        let installed = self.install_release(release, target)?;
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
        let _lock = FileLock::acquire(
            &self.root.join("locks/install.lock"),
            "Midas Lex runtime install",
        )?;
        if let Some(bin) = self.verified_bin(&release.tag_name, target)? {
            return Ok(bin);
        }
        let bin = self.bin_path(&release.tag_name, target);
        let asset_name = target.runtime_asset_name(&release.tag_name);
        let checksum_name = target.runtime_checksum_asset_name(&release.tag_name);
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
        let expected = parse_asset_checksum(&checksum_text, &asset_name)?;
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

struct FileLock {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl FileLock {
    fn acquire(path: &Path, operation: &str) -> MidasLexResult<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| error(format!("invalid {operation} lock path")))?;
        fs::create_dir_all(parent).map_err(|err| {
            error(format!(
                "cannot create {operation} lock directory {}: {err}",
                parent.display()
            ))
        })?;
        Self::acquire_file(path, operation)
    }

    #[cfg(unix)]
    fn acquire_file(path: &Path, operation: &str) -> MidasLexResult<Self> {
        use std::os::fd::AsRawFd;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|err| {
                error(format!(
                    "cannot open {operation} lock {}: {err}",
                    path.display()
                ))
            })?;
        for _ in 0..120 {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Self { file });
            }
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_secs(1));
                }
                _ => {
                    return Err(error(format!(
                        "cannot lock {operation} file {}: {err}",
                        path.display()
                    )));
                }
            }
        }
        Err(error(format!(
            "timed out waiting for {operation} lock {}",
            path.display()
        )))
    }

    #[cfg(not(unix))]
    fn acquire_file(path: &Path, operation: &str) -> MidasLexResult<Self> {
        for _ in 0..120 {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_secs(1));
                }
                Err(err) => {
                    return Err(error(format!(
                        "cannot create {operation} lock {}: {err}",
                        path.display()
                    )));
                }
            }
        }
        Err(error(format!(
            "timed out waiting for {operation} lock {}",
            path.display()
        )))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
        #[cfg(not(unix))]
        {
            let _ = fs::remove_file(&self.path);
        }
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum WrapperUpdateStatus {
    Current {
        version: Version,
    },
    Updated {
        from: Version,
        to: String,
    },
    WindowsReinstallRequired {
        current_exe: PathBuf,
        release_tag: String,
    },
    SkippedUnofficialRepository,
}

fn update_current_wrapper_from_release(
    release: &Release,
    target: &Target,
    release_repo: &str,
) -> MidasLexResult<WrapperUpdateStatus> {
    if release_repo != DEFAULT_RELEASE_REPO {
        return Ok(WrapperUpdateStatus::SkippedUnofficialRepository);
    }
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let Some(release_version) = newer_wrapper_release_version(release, &current_version)? else {
        return Ok(WrapperUpdateStatus::Current {
            version: current_version,
        });
    };
    let current_exe = resolve_running_wrapper_path()?;
    update_newer_wrapper_from_release(
        release,
        target,
        &current_exe,
        &current_version,
        &release_version,
    )
}

fn resolve_running_wrapper_path() -> MidasLexResult<PathBuf> {
    let current_exe = env::current_exe().map_err(|err| {
        error(format!(
            "cannot resolve the running wrapper executable: {err}"
        ))
    })?;
    fs::canonicalize(&current_exe).map_err(|err| {
        error(format!(
            "cannot resolve the running wrapper executable {}: {err}",
            current_exe.display()
        ))
    })
}

#[cfg(test)]
fn update_wrapper_from_release(
    release: &Release,
    target: &Target,
    current_exe: &Path,
    current_version: &Version,
) -> MidasLexResult<WrapperUpdateStatus> {
    let Some(release_version) = newer_wrapper_release_version(release, current_version)? else {
        return Ok(WrapperUpdateStatus::Current {
            version: current_version.clone(),
        });
    };
    update_newer_wrapper_from_release(
        release,
        target,
        current_exe,
        current_version,
        &release_version,
    )
}

fn newer_wrapper_release_version(
    release: &Release,
    current_version: &Version,
) -> MidasLexResult<Option<Version>> {
    if release.draft {
        return Err(error("automatic wrapper update refuses draft releases"));
    }
    let release_version = parse_tag_version(&release.tag_name)?;
    if release_version <= *current_version {
        return Ok(None);
    }
    Ok(Some(release_version))
}

fn update_newer_wrapper_from_release(
    release: &Release,
    target: &Target,
    current_exe: &Path,
    current_version: &Version,
    release_version: &Version,
) -> MidasLexResult<WrapperUpdateStatus> {
    debug_assert!(release_version > current_version);
    if target.wrapper_update_support()? == WrapperUpdateSupport::RunningWindowsExe {
        return Ok(WrapperUpdateStatus::WindowsReinstallRequired {
            current_exe: current_exe.to_path_buf(),
            release_tag: release.tag_name.clone(),
        });
    }
    let asset_name = target.wrapper_asset_name(&release.tag_name);
    let checksum_name = target.wrapper_checksum_asset_name(&release.tag_name);
    let asset = release
        .asset(&asset_name)
        .ok_or_else(|| error(format!("release wrapper asset missing: {asset_name}")))?;
    let checksum_asset = release
        .asset(&checksum_name)
        .ok_or_else(|| error(format!("release wrapper checksum missing: {checksum_name}")))?;
    let initial_digest = current_wrapper_digest(current_exe)?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| error("running wrapper executable has no parent directory"))?;
    let _lock = FileLock::acquire(
        &parent.join(WRAPPER_UPDATE_LOCK_FILE),
        "Midas Lex wrapper update",
    )?;
    require_unchanged_wrapper(current_exe, &initial_digest)?;

    let checksum_text = download_text(&checksum_asset.browser_download_url, MAX_TEXT_BYTES)?;
    let expected = parse_asset_checksum(&checksum_text, &asset_name)?;
    let stage = wrapper_update_stage_path(current_exe)?;
    download_file(&asset.browser_download_url, &stage, MAX_BINARY_BYTES).map_err(|err| {
        error(format!(
            "cannot stage wrapper update beside {}: {err}; ensure its directory is writable",
            current_exe.display()
        ))
    })?;
    let mut cleanup = RemoveFileOnDrop::new(stage.clone());
    let actual = sha256_file(&stage)?;
    if actual != expected {
        return Err(error(format!(
            "checksum mismatch for {asset_name}: expected {expected}, got {actual}; the running wrapper was not changed"
        )));
    }
    let permissions = require_unchanged_wrapper(current_exe, &initial_digest)?;
    fs::set_permissions(&stage, permissions).map_err(|err| {
        error(format!(
            "cannot preserve executable permissions on staged wrapper {}: {err}",
            stage.display()
        ))
    })?;
    File::open(&stage)?.sync_all()?;
    fs::rename(&stage, current_exe).map_err(|err| {
        error(format!(
            "cannot atomically replace the running wrapper {}: {err}; ensure its directory is writable or run `cargo install midas-lex --force` after this command exits",
            current_exe.display()
        ))
    })?;
    cleanup.disarm();
    sync_parent_dir(parent).map_err(|err| {
        error(format!(
            "the wrapper was replaced, but its directory {} could not be synced: {err}",
            parent.display()
        ))
    })?;
    log::info!(
        "installed Midas Lex wrapper {} at {}",
        release.tag_name,
        current_exe.display()
    );
    Ok(WrapperUpdateStatus::Updated {
        from: current_version.clone(),
        to: release.tag_name.clone(),
    })
}

fn current_wrapper_digest(path: &Path) -> MidasLexResult<String> {
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        error(format!(
            "cannot inspect the running wrapper executable {}: {err}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(error(format!(
            "running wrapper executable is not a regular file: {}",
            path.display()
        )));
    }
    sha256_file(path).map_err(|err| {
        error(format!(
            "cannot read the running wrapper executable {}: {err}",
            path.display()
        ))
    })
}

fn require_unchanged_wrapper(path: &Path, initial_digest: &str) -> MidasLexResult<fs::Permissions> {
    let digest = current_wrapper_digest(path)?;
    if digest != initial_digest {
        return Err(error(format!(
            "running wrapper path {} changed during automatic update; no file was replaced",
            path.display()
        )));
    }
    Ok(fs::metadata(path)?.permissions())
}

fn wrapper_update_stage_path(current_exe: &Path) -> MidasLexResult<PathBuf> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| error("running wrapper executable has no parent directory"))?;
    let name = current_exe
        .file_name()
        .ok_or_else(|| error("running wrapper executable has no file name"))?
        .to_string_lossy();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    Ok(parent.join(format!(
        ".{name}.wrapper-update-{}-{nanos}.tmp",
        std::process::id()
    )))
}

struct RemoveFileOnDrop {
    path: PathBuf,
    armed: bool,
}

impl RemoveFileOnDrop {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> io::Result<()> {
    Ok(())
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
    let file = OpenOptions::new().write(true).create_new(true).open(dest)?;
    let result = (|| {
        if let Some(path) = file_url_path(url) {
            return write_limited(File::open(path)?, file, max_bytes);
        }
        let response = ureq::get(url)
            .header("User-Agent", "midas-lex-wrapper")
            .call()?;
        write_limited(response.into_body().into_reader(), file, max_bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

fn write_limited(reader: impl Read, mut file: File, max_bytes: u64) -> MidasLexResult<()> {
    let mut reader = reader.take(max_bytes.saturating_add(1));
    let copied = io::copy(&mut reader, &mut file)?;
    file.flush()?;
    if copied > max_bytes {
        return Err(error("downloaded file exceeds size limit"));
    }
    file.sync_all()?;
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

fn parse_asset_checksum(text: &str, asset_name: &str) -> MidasLexResult<String> {
    let mut lines = text.lines();
    let line = lines.next().ok_or_else(|| error("empty checksum file"))?;
    if lines.next().is_some() {
        return Err(error("checksum file must contain exactly one line"));
    }
    let mut fields = line.split_ascii_whitespace();
    let digest = fields
        .next()
        .ok_or_else(|| error("empty checksum file"))?
        .to_ascii_lowercase();
    let recorded_name = fields
        .next()
        .ok_or_else(|| error("checksum line does not name its release asset"))?;
    if fields.next().is_some()
        || digest.len() != 64
        || !digest.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(error("malformed SHA-256 checksum line"));
    }
    if recorded_name != asset_name {
        return Err(error(format!(
            "checksum names {recorded_name}, expected {asset_name}"
        )));
    }
    Ok(digest)
}

fn checksum_record_is_pre_release(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "pre_release: true")
}

#[cfg(any(test, not(unix)))]
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

fn update_stamp_path(target: &Target) -> MidasLexResult<PathBuf> {
    Ok(temp_state_dir()?.join(format!("update-{}.stamp", target.triple)))
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
    let _lock = FileLock::acquire(
        &stamp.with_extension("lock"),
        "Midas Lex automatic update timer",
    )?;
    match fs::symlink_metadata(stamp) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(error("update timer path is not a regular file"));
            }
            if let Ok(modified) = metadata.modified()
                && SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or(Duration::ZERO)
                    < UPDATE_CHECK_INTERVAL
            {
                return Ok(false);
            }
            let mut file = OpenOptions::new().write(true).truncate(true).open(stamp)?;
            file.write_all(b"")?;
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stamp)?;
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

#[cfg(not(unix))]
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
    fn automatic_update_policy_preserves_first_run_and_selector_behavior() {
        assert_eq!(
            automatic_update_policy(false, true),
            AutomaticUpdatePolicy::AfterRuntimeStart
        );
        assert_eq!(
            automatic_update_policy(false, false),
            AutomaticUpdatePolicy::SkipFirstRun
        );
        assert_eq!(
            automatic_update_policy(true, false),
            AutomaticUpdatePolicy::SkipExplicitSelector
        );
        assert_eq!(
            automatic_update_policy(true, true),
            AutomaticUpdatePolicy::SkipExplicitSelector
        );
    }

    #[test]
    fn keeps_self_update_like_args_unchanged() {
        let args = vec![OsString::from("+self-update"), OsString::from("--help")];
        let (selector, remaining) = parse_version_selector(args.clone()).unwrap();
        assert!(selector.is_none());
        assert_eq!(remaining, args);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_non_utf8_runtime_args_unchanged() {
        use std::os::unix::ffi::OsStringExt;
        for args in [
            vec![OsString::from_vec(vec![0xff]), OsString::from("docs")],
            vec![
                OsString::from("+self-update"),
                OsString::from_vec(vec![0xff]),
            ],
        ] {
            let (selector, remaining) = parse_version_selector(args.clone()).unwrap();
            assert!(selector.is_none());
            assert_eq!(remaining, args);
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
    fn configured_version_accepts_semver_with_optional_tag_prefix() {
        for raw in ["0.0.2-beta.1", "v0.0.2-beta.1"] {
            let preference = VersionSettings {
                version: Some(raw.to_string()),
                prerelease: None,
            }
            .preference("test config")
            .unwrap();
            assert_eq!(
                preference,
                Some(VersionPreference::Exact {
                    tag: "v0.0.2-beta.1".to_string(),
                    version: Version::parse("0.0.2-beta.1").unwrap(),
                })
            );
        }
    }

    #[test]
    fn configured_version_rejects_invalid_and_conflicting_values() {
        let invalid = VersionSettings {
            version: Some("latest".to_string()),
            prerelease: None,
        }
        .preference("test config")
        .unwrap_err();
        assert!(invalid.to_string().contains("invalid Midas Lex `version`"));

        let conflicting = VersionSettings {
            version: Some("0.0.2".to_string()),
            prerelease: Some(true),
        }
        .preference("test config")
        .unwrap_err();
        assert!(conflicting.to_string().contains("cannot be combined"));

        let unknown = toml::from_str::<VersionSettings>("channel = \"beta\"").unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));
    }

    #[test]
    fn version_selection_precedence_is_cli_project_global_default() {
        let cli = VersionSelector::Exact {
            tag: "v0.0.2-beta.2".to_string(),
            version: Version::parse("0.0.2-beta.2").unwrap(),
        };
        let global = VersionPreference::Exact {
            tag: "v0.0.1".to_string(),
            version: Version::parse("0.0.1").unwrap(),
        };
        assert_eq!(
            resolve_version_selector(
                Some(cli.clone()),
                Some(VersionPreference::Prerelease),
                Some(global.clone()),
            ),
            Some(cli)
        );
        assert_eq!(
            resolve_version_selector(
                None,
                Some(VersionPreference::Prerelease),
                Some(global.clone()),
            ),
            Some(VersionSelector::Prerelease)
        );
        assert_eq!(
            resolve_version_selector(None, Some(VersionPreference::Default), Some(global.clone())),
            None
        );
        assert_eq!(
            resolve_version_selector(None, None, Some(global.clone())),
            global.into_selector()
        );
        assert_eq!(resolve_version_selector(None, None, None), None);
    }

    #[test]
    fn cargo_workspace_selection_precedes_current_package_selection() {
        let metadata: CargoProjectMetadata = serde_json::from_value(serde_json::json!({
            "workspace_metadata": {
                "midas_lex": { "prerelease": true }
            },
            "packages": [{
                "manifest_path": "/workspace/member/Cargo.toml",
                "metadata": {
                    "midas_lex": { "version": "0.0.1" }
                }
            }]
        }))
        .unwrap();
        let preference = version_preference_from_cargo_metadata(
            &metadata,
            Path::new("/workspace/member/Cargo.toml"),
        )
        .unwrap();
        assert_eq!(preference, Some(VersionPreference::Prerelease));
    }

    #[test]
    fn cargo_package_selection_and_explicit_default_are_supported() {
        let metadata: CargoProjectMetadata = serde_json::from_value(serde_json::json!({
            "workspace_metadata": {},
            "packages": [{
                "manifest_path": "/workspace/member/Cargo.toml",
                "metadata": {
                    "midas_lex": { "prerelease": false }
                }
            }]
        }))
        .unwrap();
        let preference = version_preference_from_cargo_metadata(
            &metadata,
            Path::new("/workspace/member/Cargo.toml"),
        )
        .unwrap();
        assert_eq!(preference, Some(VersionPreference::Default));
    }

    #[test]
    fn cargo_metadata_exposes_normal_manifest_version_selection() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "midas-lex-config-test"
version = "0.1.0"
edition = "2024"

[package.metadata.midas_lex]
version = "0.0.2-beta.1"
"#,
        )
        .unwrap();
        let output = Command::new("cargo")
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let metadata: CargoProjectMetadata = serde_json::from_slice(&output.stdout).unwrap();
        let preference =
            version_preference_from_cargo_metadata(&metadata, &dir.path().join("Cargo.toml"))
                .unwrap();
        assert_eq!(
            preference,
            Some(VersionPreference::Exact {
                tag: "v0.0.2-beta.1".to_string(),
                version: Version::parse("0.0.2-beta.1").unwrap(),
            })
        );
    }

    #[test]
    fn root_config_is_optional_and_remains_outside_managed_toolchains() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_global_version_preference(dir.path()).unwrap(), None);

        let config_path = dir.path().join(CONFIG_FILE);
        let config_source = b"version = \"0.0.2-beta.1\"\n";
        fs::write(&config_path, config_source).unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install(&store, &target, "v0.0.1", b"runtime", b"runtime");

        assert_eq!(
            load_global_version_preference(dir.path()).unwrap(),
            Some(VersionPreference::Exact {
                tag: "v0.0.2-beta.1".to_string(),
                version: Version::parse("0.0.2-beta.1").unwrap(),
            })
        );
        assert_eq!(fs::read(config_path).unwrap(), config_source);
    }

    #[test]
    fn root_config_rejects_unknown_fields_and_conflicting_selectors() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(CONFIG_FILE);
        fs::write(&config_path, "channel = \"beta\"\n").unwrap();
        let unknown = load_global_version_preference(dir.path()).unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));

        fs::write(config_path, "version = \"0.0.2\"\nprerelease = true\n").unwrap();
        let conflicting = load_global_version_preference(dir.path()).unwrap_err();
        assert!(conflicting.to_string().contains("cannot be combined"));
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
            target.runtime_asset_name("v0.0.1-alpha.1"),
            "midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl"
        );
        assert_eq!(
            target.runtime_checksum_asset_name("v0.0.1-alpha.1"),
            "midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl.sha256"
        );
        assert_eq!(
            target.wrapper_asset_name("v0.0.2-beta.1"),
            "midas-lex-v0.0.2-beta.1-x86_64-unknown-linux-musl"
        );
        let windows = Target {
            triple: "aarch64-pc-windows-msvc",
            exe_name: "midas-lex.exe",
        };
        assert_eq!(
            windows.wrapper_checksum_asset_name("v0.0.2-beta.1"),
            "midas-lex-v0.0.2-beta.1-aarch64-pc-windows-msvc.exe.sha256"
        );
    }

    #[test]
    fn wrapper_update_lock_name_stays_rollout_compatible() {
        assert_eq!(WRAPPER_UPDATE_LOCK_FILE, ".midas-lex-self-update.lock");
    }

    #[test]
    fn parses_checksum_prefix() {
        let checksum = "A".repeat(64);
        let text =
            format!("{checksum}  midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl\n");
        assert_eq!(parse_checksum(&text).unwrap(), "a".repeat(64));
    }

    #[test]
    fn published_checksum_requires_one_matching_asset_line() {
        let digest = "A".repeat(64);
        let asset = "midas-lex-v0.0.2-beta.1-x86_64-unknown-linux-musl";
        assert_eq!(
            parse_asset_checksum(&format!("{digest}  {asset}\n"), asset).unwrap(),
            "a".repeat(64)
        );
        for malformed in [
            format!("{digest}\n"),
            format!("{digest}  other-asset\n"),
            format!("{digest}  {asset} extra\n"),
            format!("{digest}  {asset}\n{digest}  {asset}\n"),
            format!("short  {asset}\n"),
        ] {
            assert!(parse_asset_checksum(&malformed, asset).is_err());
        }
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
                binary_path.file_name().unwrap().to_string_lossy()
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
    fn background_wrapper_failure_does_not_block_runtime_update() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = InstallStore::new(dir.path().to_path_buf());
        let target = test_target();
        write_cached_install(
            &store,
            &target,
            "v0.0.1-beta.1",
            b"old runtime",
            b"old runtime",
        );
        let mut release = write_wrapper_release(
            source_dir.path(),
            &target,
            "v0.0.2-beta.1",
            b"wrapper",
            b"wrapper",
        );
        add_runtime_assets(
            source_dir.path(),
            &target,
            &mut release,
            b"new runtime",
            b"new runtime",
        );

        let results = apply_background_release(&store, &release, &target, |_, _| {
            Err(error("injected wrapper update failure"))
        });

        assert!(
            results
                .wrapper
                .unwrap_err()
                .to_string()
                .contains("injected")
        );
        results.runtime.unwrap();
        let installed = store
            .verified_bin("v0.0.2-beta.1", &target)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(installed).unwrap(), b"new runtime");
    }

    #[test]
    fn update_start_failure_does_not_change_runtime_exit() {
        use std::cell::Cell;
        let called = Cell::new(false);
        let status = run_real_binary_with_update_start(
            &env::current_exe().unwrap(),
            vec![
                OsString::from("__midas_lex_no_such_test__"),
                OsString::from("--exact"),
                OsString::from("--quiet"),
            ],
            || {
                called.set(true);
                Err(error("injected update start failure"))
            },
        )
        .unwrap();
        assert!(called.get());
        assert_eq!(status, ExitCode::SUCCESS);
    }

    #[test]
    fn automatic_wrapper_update_skips_unofficial_repository() {
        let status = update_current_wrapper_from_release(
            &test_pre_release("v0.0.2-beta.1"),
            &test_target(),
            "example/not-official",
        )
        .unwrap();
        assert_eq!(status, WrapperUpdateStatus::SkippedUnofficialRepository);
    }

    #[cfg(unix)]
    #[test]
    fn automatic_wrapper_update_atomically_replaces_and_preserves_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let current_exe = dir.path().join("midas-lex");
        fs::write(&current_exe, b"old wrapper").unwrap();
        fs::set_permissions(&current_exe, fs::Permissions::from_mode(0o751)).unwrap();
        let old_inode = fs::metadata(&current_exe).unwrap().ino();
        let target = test_target();
        let release = write_wrapper_release(
            source_dir.path(),
            &target,
            "v0.0.2-beta.1",
            b"new verified wrapper",
            b"new verified wrapper",
        );

        let status = update_wrapper_from_release(
            &release,
            &target,
            &current_exe,
            &Version::parse("0.0.1-beta.1").unwrap(),
        )
        .unwrap();

        assert_eq!(
            status,
            WrapperUpdateStatus::Updated {
                from: Version::parse("0.0.1-beta.1").unwrap(),
                to: "v0.0.2-beta.1".to_string(),
            }
        );
        assert_eq!(fs::read(&current_exe).unwrap(), b"new verified wrapper");
        let metadata = fs::metadata(&current_exe).unwrap();
        assert_ne!(metadata.ino(), old_inode);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o751);
        assert_no_wrapper_update_stage(dir.path());
    }

    #[cfg(unix)]
    #[test]
    fn automatic_wrapper_checksum_mismatch_preserves_wrapper_and_cleans_stage() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let current_exe = dir.path().join("midas-lex");
        fs::write(&current_exe, b"old wrapper").unwrap();
        let target = test_target();
        let release = write_wrapper_release(
            source_dir.path(),
            &target,
            "v0.0.2-beta.1",
            b"unverified replacement",
            b"different replacement",
        );

        let err = update_wrapper_from_release(
            &release,
            &target,
            &current_exe,
            &Version::parse("0.0.1-beta.1").unwrap(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("checksum mismatch"));
        assert_eq!(fs::read(&current_exe).unwrap(), b"old wrapper");
        assert_no_wrapper_update_stage(dir.path());
    }

    #[cfg(unix)]
    #[test]
    fn automatic_wrapper_update_rejects_malformed_or_missing_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let current_exe = dir.path().join("midas-lex");
        fs::write(&current_exe, b"old wrapper").unwrap();
        let target = test_target();
        let mut release = write_wrapper_release(
            source_dir.path(),
            &target,
            "v0.0.2-beta.1",
            b"replacement",
            b"replacement",
        );
        let checksum_path = source_dir
            .path()
            .join(target.wrapper_checksum_asset_name("v0.0.2-beta.1"));
        fs::write(&checksum_path, b"malformed checksum\n").unwrap();
        let err = update_wrapper_from_release(
            &release,
            &target,
            &current_exe,
            &Version::parse("0.0.1-beta.1").unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("malformed"));
        assert_eq!(fs::read(&current_exe).unwrap(), b"old wrapper");

        release.assets.pop();
        let err = update_wrapper_from_release(
            &release,
            &target,
            &current_exe,
            &Version::parse("0.0.1-beta.1").unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("wrapper checksum missing"));
        assert_eq!(fs::read(&current_exe).unwrap(), b"old wrapper");
        assert_no_wrapper_update_stage(dir.path());
    }

    #[cfg(unix)]
    #[test]
    fn automatic_wrapper_permission_failure_is_non_destructive() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let current_exe = dir.path().join("midas-lex");
        fs::write(&current_exe, b"old wrapper").unwrap();
        let target = test_target();
        let release = write_wrapper_release(
            source_dir.path(),
            &target,
            "v0.0.2-beta.1",
            b"replacement",
            b"replacement",
        );
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let result = update_wrapper_from_release(
            &release,
            &target,
            &current_exe,
            &Version::parse("0.0.1-beta.1").unwrap(),
        );
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrapper update lock"));
        assert_eq!(fs::read(&current_exe).unwrap(), b"old wrapper");
        assert_no_wrapper_update_stage(dir.path());
    }

    #[test]
    fn windows_newer_release_notice_contains_running_path_and_recovery() {
        let current_exe = PathBuf::from(r"D:\custom tools\midas-lex.exe");
        for target in [
            Target {
                triple: "x86_64-pc-windows-msvc",
                exe_name: "midas-lex.exe",
            },
            Target {
                triple: "aarch64-pc-windows-msvc",
                exe_name: "midas-lex.exe",
            },
        ] {
            let status = update_wrapper_from_release(
                &test_pre_release("v0.0.1-beta.3"),
                &target,
                &current_exe,
                &Version::parse("0.0.1-beta.2").unwrap(),
            )
            .unwrap();
            assert_eq!(
                status,
                WrapperUpdateStatus::WindowsReinstallRequired {
                    current_exe: current_exe.clone(),
                    release_tag: "v0.0.1-beta.3".to_string(),
                }
            );
            let notice = windows_reinstall_notice(&current_exe, "v0.0.1-beta.3");
            assert!(notice.contains(&current_exe.display().to_string()));
            assert!(notice.contains("cannot be replaced safely"));
            assert!(notice.contains("cargo install midas-lex --force"));
        }
    }

    #[test]
    fn windows_equal_or_newer_local_version_does_not_prompt_or_download() {
        let target = Target {
            triple: "x86_64-pc-windows-msvc",
            exe_name: "midas-lex.exe",
        };
        for (release_tag, current) in [
            ("v0.0.1-beta.2", "0.0.1-beta.2"),
            ("v0.0.1-beta.1", "0.0.1-beta.2"),
            ("v0.0.1-beta.2", "0.0.2-dev.1"),
        ] {
            let current = Version::parse(current).unwrap();
            assert_eq!(
                update_wrapper_from_release(
                    &test_pre_release(release_tag),
                    &target,
                    Path::new("path-is-not-read"),
                    &current,
                )
                .unwrap(),
                WrapperUpdateStatus::Current { version: current }
            );
        }
    }

    #[test]
    fn windows_validates_release_before_reinstall_notice() {
        let target = Target {
            triple: "x86_64-pc-windows-msvc",
            exe_name: "midas-lex.exe",
        };
        let current = Version::parse("0.0.1-beta.2").unwrap();
        let err = update_wrapper_from_release(
            &test_release("v0.0.1-beta.3", true),
            &target,
            Path::new("midas-lex.exe"),
            &current,
        )
        .unwrap_err();
        assert!(err.to_string().contains("draft"));
        assert!(
            update_wrapper_from_release(
                &test_release("not-a-version", false),
                &target,
                Path::new("midas-lex.exe"),
                &current,
            )
            .is_err()
        );
    }

    #[test]
    fn current_wrapper_path_comes_from_the_running_process() {
        let expected = fs::canonicalize(env::current_exe().unwrap()).unwrap();
        assert_eq!(resolve_running_wrapper_path().unwrap(), expected);
    }

    #[test]
    fn automatic_windows_notice_uses_resolved_running_path() {
        let current = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let newer = Version::new(current.major, current.minor, current.patch + 1);
        let release = test_release(&format!("v{newer}"), false);
        let target = Target {
            triple: "x86_64-pc-windows-msvc",
            exe_name: "midas-lex.exe",
        };
        let expected = resolve_running_wrapper_path().unwrap();
        let status =
            update_current_wrapper_from_release(&release, &target, DEFAULT_RELEASE_REPO).unwrap();
        let WrapperUpdateStatus::WindowsReinstallRequired {
            current_exe,
            release_tag,
        } = status
        else {
            panic!("expected Windows reinstall notice");
        };
        assert_eq!(current_exe, expected);
        assert_eq!(release_tag, format!("v{newer}"));
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
        let stamp = update_stamp_path(&target).unwrap();
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
        assert_eq!(
            stamp.file_name().unwrap().to_string_lossy(),
            format!("update-{}.stamp", target.triple)
        );
    }

    #[test]
    fn concurrent_update_timer_claims_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = dir.path().join("update.stamp");
        let left_stamp = stamp.clone();
        let left = thread::spawn(move || claim_update_timer(&left_stamp).unwrap());
        let right = thread::spawn(move || claim_update_timer(&stamp).unwrap());
        let mut results = [left.join().unwrap(), right.join().unwrap()];
        results.sort_unstable();
        assert_eq!(results, [false, true]);
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
    fn file_lock_can_be_reacquired_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locks/install.lock");
        {
            let _lock = FileLock::acquire(&path, "test install").unwrap();
            assert!(path.is_file());
        }
        let _lock = FileLock::acquire(&path, "test install").unwrap();
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

    fn write_wrapper_release(
        source_dir: &Path,
        target: &Target,
        tag: &str,
        binary: &[u8],
        checksum_source: &[u8],
    ) -> Release {
        let asset_name = target.wrapper_asset_name(tag);
        let checksum_name = target.wrapper_checksum_asset_name(tag);
        let binary_path = source_dir.join(&asset_name);
        let checksum_path = source_dir.join(&checksum_name);
        fs::write(&binary_path, binary).unwrap();
        fs::write(
            &checksum_path,
            format!("{}  {asset_name}\n", sha256_hex(checksum_source)),
        )
        .unwrap();
        Release {
            tag_name: tag.to_string(),
            draft: false,
            prerelease: true,
            assets: vec![
                ReleaseAsset {
                    name: asset_name,
                    browser_download_url: format!("file://{}", binary_path.display()),
                },
                ReleaseAsset {
                    name: checksum_name,
                    browser_download_url: format!("file://{}", checksum_path.display()),
                },
            ],
        }
    }

    fn add_runtime_assets(
        source_dir: &Path,
        target: &Target,
        release: &mut Release,
        binary: &[u8],
        checksum_source: &[u8],
    ) {
        let asset_name = target.runtime_asset_name(&release.tag_name);
        let checksum_name = target.runtime_checksum_asset_name(&release.tag_name);
        let binary_path = source_dir.join(&asset_name);
        let checksum_path = source_dir.join(&checksum_name);
        fs::write(&binary_path, binary).unwrap();
        fs::write(
            &checksum_path,
            format!("{}  {asset_name}\n", sha256_hex(checksum_source)),
        )
        .unwrap();
        release.assets.extend([
            ReleaseAsset {
                name: asset_name,
                browser_download_url: format!("file://{}", binary_path.display()),
            },
            ReleaseAsset {
                name: checksum_name,
                browser_download_url: format!("file://{}", checksum_path.display()),
            },
        ]);
    }

    #[cfg(unix)]
    fn assert_no_wrapper_update_stage(dir: &Path) {
        assert!(!fs::read_dir(dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
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
                target.runtime_asset_name(tag),
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
