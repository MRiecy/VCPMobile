//! `VCPMobileCLI` Android runtime profile 的 P0 校验器。
//!
//! 所有入口只接受相对仓库根；小型 JSON/TSV 有界读取，大型 rootfs/PRoot/loader
//! 仅以固定缓冲区流式计算 size 与 SHA-256，不进入可执行文件或整块内存。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::manifest::vcp_mobile_cli_manifest;
use super::protocol::{
    DEFAULT_BOUNDED_READ_BYTES, DEFAULT_TIMEOUT_MS, MAX_BOUNDED_READ_BYTES, MAX_POLL_WAIT_MS,
    MAX_TIMEOUT_MS,
};

pub const COMMAND_PROFILE_RELATIVE_PATH: &str = "runtime-assets/vcp-cli/command-profile.json";

const EMBEDDED_COMMAND_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/runtime-assets/vcp-cli/command-profile.json"
));
const EMBEDDED_PACKAGE_LOCK: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/runtime-assets/vcp-cli/alpine-packages.lock.tsv"
));

const PROFILE_DIRECTORY: &str = "runtime-assets/vcp-cli";
const MAX_PROFILE_BYTES: u64 = 256 * 1024;
const MAX_PACKAGE_LOCK_BYTES: u64 = 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 64 * 1024;
const EXPECTED_COMMAND_COUNT: usize = 41;
const EXPECTED_LOCKED_PACKAGE_COUNT: usize = 72;

const EXPECTED_COMMANDS: [&str; EXPECTED_COMMAND_COUNT] = [
    "bash", "sh", "pwd", "ls", "mkdir", "cp", "mv", "rm", "ln", "touch", "cat", "head", "tail",
    "wc", "stat", "grep", "sed", "awk", "sort", "uniq", "cut", "tr", "xargs", "find", "diff",
    "patch", "tar", "gzip", "xz", "zip", "unzip", "file", "jq", "curl", "wget", "git", "ssh",
    "scp", "python3", "pip", "apk",
];

const REQUIRED_UNSUPPORTED: [&str; 9] = [
    "sudo",
    "apt",
    "dnf",
    "systemctl",
    "docker",
    "adb",
    "gui",
    "android_root",
    "shizuku",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VcpCliCommandProfile {
    pub schema_version: u32,
    pub profile_id: String,
    pub platform: ProfilePlatform,
    pub guest: ProfileGuest,
    pub invocation: ProfileInvocation,
    pub proot: ProfileProot,
    pub talloc: ProfileTalloc,
    pub rootfs: ProfileRootfs,
    pub commands: ProfileCommands,
    pub command_paths: BTreeMap<String, String>,
    pub unsupported_by_default: Vec<String>,
    pub budgets: ProfileBudgets,
    pub apk_budget: ProfileApkBudget,
    pub probe: ProfileProbe,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfilePlatform {
    pub host: String,
    pub abi: String,
    pub min_sdk: u32,
    pub privilege: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileGuest {
    pub distribution: String,
    pub version: String,
    pub libc: String,
    pub libc_version: String,
    pub simulated_uid: u32,
    pub android_root: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileInvocation {
    pub shell: String,
    pub shell_version: String,
    pub argv: Vec<String>,
    pub default_cwd: String,
    pub workspace: String,
    pub skills_access: String,
    pub interactive: bool,
    pub persistent_shell_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileProot {
    pub source: String,
    pub tag: String,
    pub commit: String,
    pub source_archive_sha256: String,
    pub ndk: String,
    pub android_api: u32,
    pub patch: String,
    pub loader_mode: String,
    pub binary: String,
    pub binary_sha256: String,
    pub binary_bytes: u64,
    pub loader_binary: String,
    pub loader_sha256: String,
    pub loader_bytes: u64,
    pub license: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileTalloc {
    pub version: String,
    pub source: String,
    pub source_archive_sha256: String,
    pub linkage: String,
    pub license: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileRootfs {
    pub base_source: String,
    pub base_sha256: String,
    pub archive: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub tar_content_sha256: String,
    pub logical_bytes: u64,
    pub installed_package_count: usize,
    pub locked_incremental_package_count: usize,
    pub package_lock: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileCommands {
    pub shell_and_files: Vec<String>,
    pub text_and_search: Vec<String>,
    pub archives_and_data: Vec<String>,
    pub network_and_vcs: Vec<String>,
    pub scripts_and_packages: Vec<String>,
}

impl ProfileCommands {
    fn iter(&self) -> impl Iterator<Item = &str> {
        self.shell_and_files
            .iter()
            .chain(&self.text_and_search)
            .chain(&self.archives_and_data)
            .chain(&self.network_and_vcs)
            .chain(&self.scripts_and_packages)
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileBudgets {
    pub foreground_yield_ms: u64,
    pub default_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub default_poll_bytes: usize,
    pub max_poll_bytes: usize,
    pub artifact_bytes_per_job: u64,
    pub workspace_default_bytes: u64,
    pub default_concurrent_jobs: usize,
    pub max_concurrent_jobs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileApkBudget {
    pub raw_asset_bytes: u64,
    pub estimated_compressed_increment_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileProbe {
    pub script: String,
    pub verified_at: String,
    pub device_class: String,
    pub android_version: u32,
    pub android_api: u32,
    pub abi: String,
    pub all_commands_present: bool,
    pub bash_was_actual_interpreter: bool,
    pub negative_process_group_signal_removed_whole_tree: bool,
    pub background_reliability_claim: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedAlpinePackage {
    pub name: String,
    pub version: String,
    pub license: String,
    pub repository: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRuntimeAsset {
    /// 始终是调用者提供的相对仓库根下的相对路径。
    pub repository_path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCommandProfileContract {
    pub profile: VcpCliCommandProfile,
    pub locked_packages: Vec<LockedAlpinePackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCommandProfile {
    pub contract: ValidatedCommandProfileContract,
    pub rootfs: VerifiedRuntimeAsset,
    pub proot: VerifiedRuntimeAsset,
    pub proot_loader: VerifiedRuntimeAsset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileValidationErrorKind {
    InvalidPath,
    Io,
    InvalidJson,
    InvalidPackageLock,
    Invariant,
    AssetMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileValidationError {
    pub kind: ProfileValidationErrorKind,
    pub field: String,
    pub message: String,
}

impl ProfileValidationError {
    fn new(
        kind: ProfileValidationErrorKind,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            field: field.into(),
            message: message.into(),
        }
    }

    fn invariant(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ProfileValidationErrorKind::Invariant, field, message)
    }

    fn invalid_path(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ProfileValidationErrorKind::InvalidPath, field, message)
    }
}

impl fmt::Display for ProfileValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ProfileValidationError {}

/// 从固定仓库相对路径读取 profile 和 package lock，并校验全部 P0 逻辑合同。
/// `repository_root` 必须是相对于当前进程目录的仓库 `src-tauri` 根。
pub async fn load_command_profile_contract(
    repository_root: &Path,
) -> Result<ValidatedCommandProfileContract, ProfileValidationError> {
    validate_relative_repository_root(repository_root)?;
    let profile_path = repository_root.join(COMMAND_PROFILE_RELATIVE_PATH);
    let profile_bytes =
        read_bounded_regular_file(&profile_path, MAX_PROFILE_BYTES, "commandProfile").await?;
    let profile =
        serde_json::from_slice::<VcpCliCommandProfile>(&profile_bytes).map_err(|error| {
            ProfileValidationError::new(
                ProfileValidationErrorKind::InvalidJson,
                "commandProfile",
                format!("invalid typed JSON: {error}"),
            )
        })?;

    let package_lock_path = resolve_profile_asset_path(
        repository_root,
        &profile.rootfs.package_lock,
        "rootfs.packageLock",
    )?;
    let package_lock_bytes = read_bounded_regular_file(
        &package_lock_path,
        MAX_PACKAGE_LOCK_BYTES,
        "rootfs.packageLock",
    )
    .await?;
    let package_lock_text = std::str::from_utf8(&package_lock_bytes).map_err(|error| {
        ProfileValidationError::new(
            ProfileValidationErrorKind::InvalidPackageLock,
            "rootfs.packageLock",
            format!("lock must be UTF-8: {error}"),
        )
    })?;
    let locked_packages = parse_package_lock(package_lock_text)?;
    validate_profile_contract(&profile, &locked_packages)?;

    Ok(ValidatedCommandProfileContract {
        profile,
        locked_packages,
    })
}

/// APK 编译时冻结且完成全部逻辑/lock 不变量校验的 profile。
/// 运行时 staged 资产只接受这个身份；
/// 大型 rootfs/PRoot/loader 本身仍通过 plugin stage 后流式验证，不被 `include_bytes!`。
pub fn embedded_command_profile() -> Result<VcpCliCommandProfile, ProfileValidationError> {
    let profile = serde_json::from_str(EMBEDDED_COMMAND_PROFILE_JSON).map_err(|error| {
        ProfileValidationError::new(
            ProfileValidationErrorKind::InvalidJson,
            "embeddedCommandProfile",
            format!("invalid typed JSON: {error}"),
        )
    })?;
    let locked_packages = parse_package_lock(EMBEDDED_PACKAGE_LOCK)?;
    validate_profile_contract(&profile, &locked_packages)?;
    Ok(profile)
}

pub async fn verify_staged_runtime_assets(
    profile: &VcpCliCommandProfile,
    rootfs_archive: PathBuf,
    proot_binary: PathBuf,
    proot_loader: PathBuf,
) -> Result<
    (
        VerifiedRuntimeAsset,
        VerifiedRuntimeAsset,
        VerifiedRuntimeAsset,
    ),
    ProfileValidationError,
> {
    let rootfs = verify_regular_asset(
        rootfs_archive,
        profile.rootfs.archive_bytes,
        &profile.rootfs.archive_sha256,
        "rootfs.archive",
    )
    .await?;
    let proot = verify_regular_asset(
        proot_binary,
        profile.proot.binary_bytes,
        &profile.proot.binary_sha256,
        "proot.binary",
    )
    .await?;
    let loader = verify_regular_asset(
        proot_loader,
        profile.proot.loader_bytes,
        &profile.proot.loader_sha256,
        "proot.loaderBinary",
    )
    .await?;
    Ok((rootfs, proot, loader))
}

/// 在逻辑合同通过后，流式校验 profile 指向的 rootfs、PRoot 与 unbundled loader 实物。
pub async fn verify_command_profile_assets(
    repository_root: &Path,
    contract: &ValidatedCommandProfileContract,
) -> Result<
    (
        VerifiedRuntimeAsset,
        VerifiedRuntimeAsset,
        VerifiedRuntimeAsset,
    ),
    ProfileValidationError,
> {
    validate_relative_repository_root(repository_root)?;
    let rootfs_path = resolve_profile_asset_path(
        repository_root,
        &contract.profile.rootfs.archive,
        "rootfs.archive",
    )?;
    let proot_path = resolve_profile_asset_path(
        repository_root,
        &contract.profile.proot.binary,
        "proot.binary",
    )?;
    let loader_path = resolve_profile_asset_path(
        repository_root,
        &contract.profile.proot.loader_binary,
        "proot.loaderBinary",
    )?;

    let rootfs = verify_regular_asset(
        rootfs_path,
        contract.profile.rootfs.archive_bytes,
        &contract.profile.rootfs.archive_sha256,
        "rootfs.archive",
    )
    .await?;
    let proot = verify_regular_asset(
        proot_path,
        contract.profile.proot.binary_bytes,
        &contract.profile.proot.binary_sha256,
        "proot.binary",
    )
    .await?;
    let loader = verify_regular_asset(
        loader_path,
        contract.profile.proot.loader_bytes,
        &contract.profile.proot.loader_sha256,
        "proot.loaderBinary",
    )
    .await?;
    Ok((rootfs, proot, loader))
}

/// P0/CI 主入口：typed JSON、lock 与三个发行实物必须同时通过。
pub async fn load_and_validate_command_profile(
    repository_root: &Path,
) -> Result<ValidatedCommandProfile, ProfileValidationError> {
    let contract = load_command_profile_contract(repository_root).await?;
    let (rootfs, proot, proot_loader) =
        verify_command_profile_assets(repository_root, &contract).await?;
    Ok(ValidatedCommandProfile {
        contract,
        rootfs,
        proot,
        proot_loader,
    })
}

fn validate_profile_contract(
    profile: &VcpCliCommandProfile,
    packages: &[LockedAlpinePackage],
) -> Result<(), ProfileValidationError> {
    require_equal("schemaVersion", profile.schema_version, 1_u32)?;
    require_nonempty("profileId", &profile.profile_id)?;

    require_equal("platform.host", profile.platform.host.as_str(), "android")?;
    require_equal("platform.abi", profile.platform.abi.as_str(), "arm64-v8a")?;
    require_equal("platform.minSdk", profile.platform.min_sdk, 26_u32)?;
    require_equal(
        "platform.privilege",
        profile.platform.privilege.as_str(),
        "app_uid_non_root",
    )?;

    require_equal(
        "guest.distribution",
        profile.guest.distribution.as_str(),
        "alpine",
    )?;
    require_equal("guest.libc", profile.guest.libc.as_str(), "musl")?;
    require_equal("guest.simulatedUid", profile.guest.simulated_uid, 0_u32)?;
    require_false("guest.androidRoot", profile.guest.android_root)?;

    require_equal(
        "invocation.shell",
        profile.invocation.shell.as_str(),
        "/bin/bash",
    )?;
    let expected_argv = ["/bin/bash", "-lc", "<command>"];
    if profile
        .invocation
        .argv
        .iter()
        .map(String::as_str)
        .ne(expected_argv)
    {
        return Err(ProfileValidationError::invariant(
            "invocation.argv",
            "must be exactly [/bin/bash, -lc, <command>]",
        ));
    }
    require_equal(
        "invocation.defaultCwd",
        profile.invocation.default_cwd.as_str(),
        "/workspace",
    )?;
    require_equal(
        "invocation.workspace",
        profile.invocation.workspace.as_str(),
        "/workspace",
    )?;
    require_equal(
        "invocation.skillsAccess",
        profile.invocation.skills_access.as_str(),
        "action_only",
    )?;
    require_false("invocation.interactive", profile.invocation.interactive)?;
    require_false(
        "invocation.persistentShellState",
        profile.invocation.persistent_shell_state,
    )?;

    validate_sha256(
        "proot.sourceArchiveSha256",
        &profile.proot.source_archive_sha256,
    )?;
    validate_git_commit("proot.commit", &profile.proot.commit)?;
    require_equal(
        "proot.loaderMode",
        profile.proot.loader_mode.as_str(),
        "unbundled_required",
    )?;
    validate_sha256("proot.binarySha256", &profile.proot.binary_sha256)?;
    require_positive("proot.binaryBytes", profile.proot.binary_bytes)?;
    validate_sha256("proot.loaderSha256", &profile.proot.loader_sha256)?;
    require_positive("proot.loaderBytes", profile.proot.loader_bytes)?;
    require_equal(
        "proot.androidApi",
        profile.proot.android_api,
        profile.platform.min_sdk,
    )?;
    resolve_profile_relative_components(&profile.proot.patch, "proot.patch")?;
    resolve_profile_relative_components(&profile.proot.binary, "proot.binary")?;
    resolve_profile_relative_components(&profile.proot.loader_binary, "proot.loaderBinary")?;
    if profile.proot.binary == profile.proot.loader_binary {
        return Err(ProfileValidationError::invariant(
            "proot.loaderBinary",
            "must be a separate APK-native executable from the PRoot binary",
        ));
    }

    validate_sha256(
        "talloc.sourceArchiveSha256",
        &profile.talloc.source_archive_sha256,
    )?;
    validate_sha256("rootfs.baseSha256", &profile.rootfs.base_sha256)?;
    validate_sha256("rootfs.archiveSha256", &profile.rootfs.archive_sha256)?;
    validate_sha256(
        "rootfs.tarContentSha256",
        &profile.rootfs.tar_content_sha256,
    )?;
    require_positive("rootfs.archiveBytes", profile.rootfs.archive_bytes)?;
    if profile.rootfs.logical_bytes < profile.rootfs.archive_bytes {
        return Err(ProfileValidationError::invariant(
            "rootfs.logicalBytes",
            "must not be smaller than the compressed archive",
        ));
    }
    require_equal(
        "rootfs.installedPackageCount",
        profile.rootfs.installed_package_count,
        80_usize,
    )?;
    require_equal(
        "rootfs.lockedIncrementalPackageCount",
        profile.rootfs.locked_incremental_package_count,
        EXPECTED_LOCKED_PACKAGE_COUNT,
    )?;
    resolve_profile_relative_components(&profile.rootfs.archive, "rootfs.archive")?;
    resolve_profile_relative_components(&profile.rootfs.package_lock, "rootfs.packageLock")?;

    validate_commands(profile)?;
    validate_unsupported(profile)?;
    validate_budgets(profile)?;
    validate_package_lock(profile, packages)?;
    validate_probe(profile)?;

    let physical_asset_bytes = profile
        .rootfs
        .archive_bytes
        .checked_add(profile.proot.binary_bytes)
        .and_then(|bytes| bytes.checked_add(profile.proot.loader_bytes))
        .ok_or_else(|| {
            ProfileValidationError::invariant("apkBudget.rawAssetBytes", "asset size overflow")
        })?;
    require_equal(
        "apkBudget.rawAssetBytes",
        profile.apk_budget.raw_asset_bytes,
        physical_asset_bytes,
    )?;
    require_positive(
        "apkBudget.estimatedCompressedIncrementBytes",
        profile.apk_budget.estimated_compressed_increment_bytes,
    )?;
    if profile.apk_budget.estimated_compressed_increment_bytes > physical_asset_bytes {
        return Err(ProfileValidationError::invariant(
            "apkBudget.estimatedCompressedIncrementBytes",
            "must not exceed the raw runtime asset bytes",
        ));
    }

    Ok(())
}

fn validate_commands(profile: &VcpCliCommandProfile) -> Result<(), ProfileValidationError> {
    let declared = profile.commands.iter().collect::<Vec<_>>();
    if declared.len() != EXPECTED_COMMAND_COUNT {
        return Err(ProfileValidationError::invariant(
            "commands",
            format!(
                "expected {EXPECTED_COMMAND_COUNT} commands, found {}",
                declared.len()
            ),
        ));
    }
    let declared_set = declared.iter().copied().collect::<BTreeSet<_>>();
    if declared_set.len() != declared.len() {
        return Err(ProfileValidationError::invariant(
            "commands",
            "command names must be unique",
        ));
    }
    let expected_set = EXPECTED_COMMANDS.into_iter().collect::<BTreeSet<_>>();
    if declared_set != expected_set {
        return Err(ProfileValidationError::invariant(
            "commands",
            "command set differs from the frozen 41-command baseline",
        ));
    }

    let path_keys = profile
        .command_paths
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if path_keys != declared_set {
        return Err(ProfileValidationError::invariant(
            "commandPaths",
            "keys must exactly match the declared command set",
        ));
    }
    for (command, path) in &profile.command_paths {
        if path.is_empty()
            || path.contains('\0')
            || !(path.starts_with('/') || path.starts_with("builtin:"))
        {
            return Err(ProfileValidationError::invariant(
                format!("commandPaths.{command}"),
                "must be an absolute guest path or explicit builtin path",
            ));
        }
    }
    require_equal(
        "commandPaths.bash",
        profile.command_paths.get("bash").map(String::as_str),
        Some("/bin/bash"),
    )?;
    Ok(())
}

fn validate_unsupported(profile: &VcpCliCommandProfile) -> Result<(), ProfileValidationError> {
    let values = profile
        .unsupported_by_default
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let set = values.iter().copied().collect::<BTreeSet<_>>();
    if set.len() != values.len() {
        return Err(ProfileValidationError::invariant(
            "unsupportedByDefault",
            "entries must be unique",
        ));
    }
    for required in REQUIRED_UNSUPPORTED {
        if !set.contains(required) {
            return Err(ProfileValidationError::invariant(
                "unsupportedByDefault",
                format!("missing required unsupported capability: {required}"),
            ));
        }
    }
    if profile.commands.iter().any(|command| set.contains(command)) {
        return Err(ProfileValidationError::invariant(
            "unsupportedByDefault",
            "unsupported entries must not overlap advertised commands",
        ));
    }
    Ok(())
}

fn validate_budgets(profile: &VcpCliCommandProfile) -> Result<(), ProfileValidationError> {
    let budgets = &profile.budgets;
    require_equal(
        "budgets.foregroundYieldMs",
        budgets.foreground_yield_ms,
        8_000_u64,
    )?;
    if budgets.foreground_yield_ms >= vcp_mobile_cli_manifest().communication.timeout {
        return Err(ProfileValidationError::invariant(
            "budgets.foregroundYieldMs",
            "must remain below the manifest communication timeout",
        ));
    }
    require_equal(
        "budgets.defaultTimeoutMs",
        budgets.default_timeout_ms,
        DEFAULT_TIMEOUT_MS,
    )?;
    require_equal(
        "budgets.maxTimeoutMs",
        budgets.max_timeout_ms,
        MAX_TIMEOUT_MS,
    )?;
    require_equal(
        "budgets.defaultPollBytes",
        budgets.default_poll_bytes,
        DEFAULT_BOUNDED_READ_BYTES,
    )?;
    require_equal(
        "budgets.maxPollBytes",
        budgets.max_poll_bytes,
        MAX_BOUNDED_READ_BYTES,
    )?;
    require_equal(
        "budgets.artifactBytesPerJob",
        budgets.artifact_bytes_per_job,
        268_435_456_u64,
    )?;
    require_equal(
        "budgets.workspaceDefaultBytes",
        budgets.workspace_default_bytes,
        2_147_483_648_u64,
    )?;
    require_equal(
        "budgets.defaultConcurrentJobs",
        budgets.default_concurrent_jobs,
        2_usize,
    )?;
    require_equal(
        "budgets.maxConcurrentJobs",
        budgets.max_concurrent_jobs,
        4_usize,
    )?;
    require_equal(
        "budgets.foregroundYieldMs",
        budgets.foreground_yield_ms,
        MAX_POLL_WAIT_MS,
    )?;
    Ok(())
}

fn validate_package_lock(
    profile: &VcpCliCommandProfile,
    packages: &[LockedAlpinePackage],
) -> Result<(), ProfileValidationError> {
    require_equal(
        "rootfs.packageLock",
        packages.len(),
        EXPECTED_LOCKED_PACKAGE_COUNT,
    )?;
    let names = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect::<BTreeSet<_>>();
    if names.len() != packages.len() {
        return Err(ProfileValidationError::invariant(
            "rootfs.packageLock",
            "package names must be unique",
        ));
    }
    for package in packages {
        require_nonempty("rootfs.packageLock.version", &package.version)?;
        require_nonempty("rootfs.packageLock.license", &package.license)?;
        require_nonempty("rootfs.packageLock.repository", &package.repository)?;
        validate_sha256("rootfs.packageLock.sha256", &package.sha256)?;
        require_positive("rootfs.packageLock.bytes", package.bytes)?;
    }

    let bash = packages
        .iter()
        .find(|package| package.name == "bash")
        .ok_or_else(|| {
            ProfileValidationError::invariant(
                "rootfs.packageLock",
                "locked packages must include bash",
            )
        })?;
    require_equal(
        "invocation.shellVersion",
        profile.invocation.shell_version.as_str(),
        bash.version.as_str(),
    )?;
    let musl = packages
        .iter()
        .find(|package| package.name == "musl")
        .ok_or_else(|| {
            ProfileValidationError::invariant(
                "rootfs.packageLock",
                "locked packages must include musl",
            )
        })?;
    require_equal(
        "guest.libcVersion",
        profile.guest.libc_version.as_str(),
        musl.version.as_str(),
    )?;
    Ok(())
}

fn validate_probe(profile: &VcpCliCommandProfile) -> Result<(), ProfileValidationError> {
    require_equal(
        "probe.abi",
        profile.probe.abi.as_str(),
        profile.platform.abi.as_str(),
    )?;
    if profile.probe.android_api < profile.platform.min_sdk {
        return Err(ProfileValidationError::invariant(
            "probe.androidApi",
            "probe device API must meet minSdk",
        ));
    }
    require_true(
        "probe.allCommandsPresent",
        profile.probe.all_commands_present,
    )?;
    require_true(
        "probe.bashWasActualInterpreter",
        profile.probe.bash_was_actual_interpreter,
    )?;
    require_true(
        "probe.negativeProcessGroupSignalRemovedWholeTree",
        profile
            .probe
            .negative_process_group_signal_removed_whole_tree,
    )?;
    require_equal(
        "probe.backgroundReliabilityClaim",
        profile.probe.background_reliability_claim.as_str(),
        "foreground_only",
    )?;
    Ok(())
}

fn parse_package_lock(input: &str) -> Result<Vec<LockedAlpinePackage>, ProfileValidationError> {
    const HEADER: &str = "# name\tversion\tlicense\trepository\tsha256\tbytes";
    let mut lines = input.lines();
    if lines.next() != Some(HEADER) {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::InvalidPackageLock,
            "rootfs.packageLock",
            "unexpected TSV header",
        ));
    }

    let mut packages = Vec::new();
    for (offset, line) in lines.enumerate() {
        let line_number = offset + 2;
        if line.is_empty() {
            return Err(ProfileValidationError::new(
                ProfileValidationErrorKind::InvalidPackageLock,
                "rootfs.packageLock",
                format!("empty row at line {line_number}"),
            ));
        }
        let columns = line.split('\t').collect::<Vec<_>>();
        if columns.len() != 6 {
            return Err(ProfileValidationError::new(
                ProfileValidationErrorKind::InvalidPackageLock,
                "rootfs.packageLock",
                format!("line {line_number} must contain exactly 6 columns"),
            ));
        }
        let bytes = columns[5].parse::<u64>().map_err(|_| {
            ProfileValidationError::new(
                ProfileValidationErrorKind::InvalidPackageLock,
                "rootfs.packageLock",
                format!("line {line_number} has invalid bytes"),
            )
        })?;
        packages.push(LockedAlpinePackage {
            name: columns[0].to_string(),
            version: columns[1].to_string(),
            license: columns[2].to_string(),
            repository: columns[3].to_string(),
            sha256: columns[4].to_string(),
            bytes,
        });
    }
    Ok(packages)
}

async fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
    field: &str,
) -> Result<Vec<u8>, ProfileValidationError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("cannot inspect {}: {error}", path.display()),
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("{} must be a regular non-symlink file", path.display()),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("{} exceeds the bounded read limit", path.display()),
        ));
    }
    let bytes = tokio::fs::read(path).await.map_err(|error| {
        ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("{} changed while being read", path.display()),
        ));
    }
    Ok(bytes)
}

async fn verify_regular_asset(
    path: PathBuf,
    expected_bytes: u64,
    expected_sha256: &str,
    field: &str,
) -> Result<VerifiedRuntimeAsset, ProfileValidationError> {
    let metadata = tokio::fs::symlink_metadata(&path).await.map_err(|error| {
        ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("cannot inspect {}: {error}", path.display()),
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("{} must be a regular non-symlink file", path.display()),
        ));
    }
    if metadata.len() != expected_bytes {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::AssetMismatch,
            field,
            format!(
                "size mismatch for {}: expected {expected_bytes}, found {}",
                path.display(),
                metadata.len()
            ),
        ));
    }

    let mut file = tokio::fs::File::open(&path).await.map_err(|error| {
        ProfileValidationError::new(
            ProfileValidationErrorKind::Io,
            field,
            format!("cannot open {}: {error}", path.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut total_bytes = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| {
            ProfileValidationError::new(
                ProfileValidationErrorKind::Io,
                field,
                format!("cannot stream {}: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes.checked_add(read as u64).ok_or_else(|| {
            ProfileValidationError::new(
                ProfileValidationErrorKind::AssetMismatch,
                field,
                "streamed size overflow",
            )
        })?;
        if total_bytes > expected_bytes {
            return Err(ProfileValidationError::new(
                ProfileValidationErrorKind::AssetMismatch,
                field,
                format!("{} grew while being verified", path.display()),
            ));
        }
        hasher.update(&buffer[..read]);
    }

    if total_bytes != expected_bytes {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::AssetMismatch,
            field,
            format!(
                "streamed size mismatch for {}: expected {expected_bytes}, found {total_bytes}",
                path.display()
            ),
        ));
    }
    let actual_sha256 = hex::encode(hasher.finalize());
    if actual_sha256 != expected_sha256 {
        return Err(ProfileValidationError::new(
            ProfileValidationErrorKind::AssetMismatch,
            field,
            format!(
                "SHA-256 mismatch for {}: expected {expected_sha256}, found {actual_sha256}",
                path.display()
            ),
        ));
    }

    Ok(VerifiedRuntimeAsset {
        repository_path: path,
        sha256: actual_sha256,
        bytes: total_bytes,
    })
}

fn validate_relative_repository_root(path: &Path) -> Result<(), ProfileValidationError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ProfileValidationError::invalid_path(
            "repositoryRoot",
            "must be a non-empty relative path",
        ));
    }
    for component in path.components() {
        if !matches!(component, Component::CurDir | Component::Normal(_)) {
            return Err(ProfileValidationError::invalid_path(
                "repositoryRoot",
                "must not escape through parent/root/prefix components",
            ));
        }
    }
    Ok(())
}

fn resolve_profile_asset_path(
    repository_root: &Path,
    declared_path: &str,
    field: &str,
) -> Result<PathBuf, ProfileValidationError> {
    let components = resolve_profile_relative_components(declared_path, field)?;
    let mut path = repository_root.to_path_buf();
    for component in components {
        path.push(component);
    }
    Ok(path)
}

fn resolve_profile_relative_components(
    declared_path: &str,
    field: &str,
) -> Result<Vec<OsString>, ProfileValidationError> {
    if declared_path.is_empty() || declared_path.contains('\0') {
        return Err(ProfileValidationError::invalid_path(
            field,
            "declared asset path must be non-empty",
        ));
    }
    let declared = Path::new(declared_path);
    if declared.is_absolute() {
        return Err(ProfileValidationError::invalid_path(
            field,
            "absolute asset paths are forbidden",
        ));
    }

    let mut resolved = Path::new(PROFILE_DIRECTORY)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in declared.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => resolved.push(value.to_os_string()),
            Component::ParentDir => {
                if resolved.pop().is_none() {
                    return Err(ProfileValidationError::invalid_path(
                        field,
                        "asset path escapes the repository root",
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ProfileValidationError::invalid_path(
                    field,
                    "absolute/prefixed asset paths are forbidden",
                ));
            }
        }
    }
    if resolved.is_empty() {
        return Err(ProfileValidationError::invalid_path(
            field,
            "asset path must resolve to a file inside the repository",
        ));
    }
    Ok(resolved)
}

fn validate_sha256(field: &str, value: &str) -> Result<(), ProfileValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ProfileValidationError::invariant(
            field,
            "must be a lowercase 64-character SHA-256",
        ));
    }
    Ok(())
}

fn validate_git_commit(field: &str, value: &str) -> Result<(), ProfileValidationError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProfileValidationError::invariant(
            field,
            "must be a 40-character Git commit",
        ));
    }
    Ok(())
}

fn require_nonempty(field: &str, value: &str) -> Result<(), ProfileValidationError> {
    if value.trim().is_empty() {
        return Err(ProfileValidationError::invariant(
            field,
            "must not be empty",
        ));
    }
    Ok(())
}

fn require_positive(field: &str, value: u64) -> Result<(), ProfileValidationError> {
    if value == 0 {
        return Err(ProfileValidationError::invariant(field, "must be positive"));
    }
    Ok(())
}

fn require_true(field: &str, value: bool) -> Result<(), ProfileValidationError> {
    if !value {
        return Err(ProfileValidationError::invariant(field, "must be true"));
    }
    Ok(())
}

fn require_false(field: &str, value: bool) -> Result<(), ProfileValidationError> {
    if value {
        return Err(ProfileValidationError::invariant(field, "must be false"));
    }
    Ok(())
}

fn require_equal<T>(field: &str, actual: T, expected: T) -> Result<(), ProfileValidationError>
where
    T: PartialEq + fmt::Debug,
{
    if actual != expected {
        return Err(ProfileValidationError::invariant(
            field,
            format!("expected {expected:?}, found {actual:?}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELATIVE_REPOSITORY_ROOT: &str = ".";

    #[tokio::test]
    async fn repository_profile_contract_is_typed_and_valid() {
        let contract = load_command_profile_contract(Path::new(RELATIVE_REPOSITORY_ROOT))
            .await
            .expect("repository command profile contract");
        assert_eq!(contract.profile.invocation.shell, "/bin/bash");
        assert_eq!(
            contract.profile.commands.iter().count(),
            EXPECTED_COMMAND_COUNT
        );
        assert_eq!(contract.profile.command_paths.len(), EXPECTED_COMMAND_COUNT);
        assert_eq!(
            contract.locked_packages.len(),
            EXPECTED_LOCKED_PACKAGE_COUNT
        );
    }

    #[tokio::test]
    async fn critical_profile_invariants_fail_closed() {
        let contract = load_command_profile_contract(Path::new(RELATIVE_REPOSITORY_ROOT))
            .await
            .expect("repository command profile contract");

        let mut cases = Vec::<(VcpCliCommandProfile, Vec<LockedAlpinePackage>, &str)>::new();

        let mut shell = contract.profile.clone();
        shell.invocation.shell = "/bin/sh".to_string();
        cases.push((shell, contract.locked_packages.clone(), "invocation.shell"));

        let mut argv = contract.profile.clone();
        argv.invocation.argv = vec!["/bin/bash".to_string(), "-c".to_string()];
        cases.push((argv, contract.locked_packages.clone(), "invocation.argv"));

        let mut commands = contract.profile.clone();
        commands.commands.shell_and_files.pop();
        cases.push((commands, contract.locked_packages.clone(), "commands"));

        let mut paths = contract.profile.clone();
        paths.command_paths.remove("bash");
        cases.push((paths, contract.locked_packages.clone(), "commandPaths"));

        let mut unsupported = contract.profile.clone();
        unsupported
            .unsupported_by_default
            .retain(|value| value != "android_root");
        cases.push((
            unsupported,
            contract.locked_packages.clone(),
            "unsupportedByDefault",
        ));

        let mut budgets = contract.profile.clone();
        budgets.budgets.max_timeout_ms -= 1;
        cases.push((
            budgets,
            contract.locked_packages.clone(),
            "budgets.maxTimeoutMs",
        ));

        let mut loader_mode = contract.profile.clone();
        loader_mode.proot.loader_mode = "bundled".to_string();
        cases.push((
            loader_mode,
            contract.locked_packages.clone(),
            "proot.loaderMode",
        ));

        let mut packages = contract.locked_packages.clone();
        packages.pop();
        cases.push((contract.profile.clone(), packages, "rootfs.packageLock"));

        for (profile, packages, expected_field) in cases {
            let error = validate_profile_contract(&profile, &packages)
                .expect_err("mutated profile must fail closed");
            assert_eq!(error.kind, ProfileValidationErrorKind::Invariant);
            assert_eq!(error.field, expected_field);
        }
    }

    #[test]
    fn asset_paths_are_relative_and_cannot_escape_repository() {
        assert!(validate_relative_repository_root(Path::new(".")).is_ok());
        assert!(validate_relative_repository_root(Path::new("src-tauri")).is_ok());
        assert!(validate_relative_repository_root(Path::new("/tmp/repository")).is_err());
        assert!(validate_relative_repository_root(Path::new("../repository")).is_err());

        let proot = resolve_profile_relative_components(
            "../../plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot.so",
            "proot.binary",
        )
        .expect("profile PRoot path stays inside repository");
        assert_eq!(
            PathBuf::from_iter(proot),
            PathBuf::from("plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot.so")
        );
        let loader = resolve_profile_relative_components(
            "../../plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot_loader.so",
            "proot.loaderBinary",
        )
        .expect("profile unbundled loader path stays inside repository");
        assert_eq!(
            PathBuf::from_iter(loader),
            PathBuf::from(
                "plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot_loader.so"
            )
        );
        let rootfs = resolve_profile_relative_components(
            "android-assets/vcp-cli-rootfs-3.24.1-aarch64.tar.zst",
            "rootfs.archive",
        )
        .expect("profile rootfs path stays inside repository");
        assert_eq!(
            PathBuf::from_iter(rootfs),
            PathBuf::from(
                "runtime-assets/vcp-cli/android-assets/vcp-cli-rootfs-3.24.1-aarch64.tar.zst"
            )
        );
        assert!(resolve_profile_relative_components("../../../escape", "asset").is_err());
        assert!(resolve_profile_relative_components("/absolute", "asset").is_err());
    }

    #[tokio::test]
    async fn streaming_asset_verifier_checks_size_and_sha256() {
        let directory = tempfile::tempdir().expect("temporary asset directory");
        let asset = directory.path().join("asset.bin");
        tokio::fs::write(&asset, b"streamed-runtime-asset")
            .await
            .expect("write temporary asset");

        let verified = verify_regular_asset(
            asset.clone(),
            22,
            "2608c8c39f28488fc0e3bfc68a9b8f233dbe63b7b93418e6e2bf42f7133ae3fe",
            "asset",
        )
        .await
        .expect("streamed asset must verify");
        assert_eq!(verified.bytes, 22);

        let size_error = verify_regular_asset(
            asset.clone(),
            21,
            "2608c8c39f28488fc0e3bfc68a9b8f233dbe63b7b93418e6e2bf42f7133ae3fe",
            "asset",
        )
        .await
        .expect_err("wrong size must fail");
        assert_eq!(size_error.kind, ProfileValidationErrorKind::AssetMismatch);

        let hash_error = verify_regular_asset(asset, 22, &"0".repeat(64), "asset")
            .await
            .expect_err("wrong hash must fail");
        assert_eq!(hash_error.kind, ProfileValidationErrorKind::AssetMismatch);
    }

    #[tokio::test]
    async fn repository_rootfs_proot_and_loader_match_profile_bytes_and_sha256() {
        let validated = load_and_validate_command_profile(Path::new(RELATIVE_REPOSITORY_ROOT))
            .await
            .expect("repository runtime assets must match profile");
        assert_eq!(
            validated.rootfs.bytes,
            validated.contract.profile.rootfs.archive_bytes
        );
        assert_eq!(
            validated.proot.bytes,
            validated.contract.profile.proot.binary_bytes
        );
        assert_eq!(
            validated.proot_loader.bytes,
            validated.contract.profile.proot.loader_bytes
        );
        assert!(validated.rootfs.repository_path.is_relative());
        assert!(validated.proot.repository_path.is_relative());
        assert!(validated.proot_loader.repository_path.is_relative());
        assert!(validated.rootfs.repository_path.ends_with(
            "runtime-assets/vcp-cli/android-assets/vcp-cli-rootfs-3.24.1-aarch64.tar.zst"
        ));
        assert!(validated
            .proot
            .repository_path
            .ends_with("plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot.so"));
        assert!(validated.proot_loader.repository_path.ends_with(
            "plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot_loader.so"
        ));
    }
}
