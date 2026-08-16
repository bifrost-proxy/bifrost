use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

const LOCAL_ASSET_ENV_KEYS: [&str; 4] = [
    "BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES",
    "BIFROST_UPGRADE_TEST_LATEST_VERSION",
    "BIFROST_UPGRADE_TEST_ARCHIVE",
    "BIFROST_APP_UPGRADE_TEST_PACKAGE",
];
const SUPPORTED_ARCHIVE_EXTENSIONS: [&str; 3] = ["zip", "tar.xz", "tar.gz"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalUpgradeAssets {
    root: PathBuf,
    version: String,
    cli_archive: PathBuf,
    desktop_package: Option<PathBuf>,
}

impl LocalUpgradeAssets {
    fn discover(root: PathBuf) -> Result<Self, BifrostError> {
        Self::discover_for_detected_target(root, get_target_triple())
    }

    fn discover_for_detected_target(
        root: PathBuf,
        target: Option<&str>,
    ) -> Result<Self, BifrostError> {
        let target = target.ok_or_else(|| {
            BifrostError::Config(
                "--local-assets is not supported on this platform or architecture".to_string(),
            )
        })?;
        Self::discover_for_target(root, target)
    }

    fn require_desktop_package_if_installed(&self) -> Result<(), BifrostError> {
        if installed_desktop_app_path().is_some() && self.desktop_package.is_none() {
            let target = get_target_triple().unwrap_or("unknown-target");
            return Err(BifrostError::NotFound(format!(
                "local assets for v{} contain the CLI archive but not the installed desktop target package: {}",
                self.version,
                local_desktop_asset_name(&self.version, target)
                    .unwrap_or_else(|| "<unsupported desktop target>".to_string())
            )));
        }
        Ok(())
    }

    fn discover_for_target(root: PathBuf, target: &str) -> Result<Self, BifrostError> {
        let root = canonicalize_local_asset_root(&root).map_err(|error| {
            BifrostError::Config(format!(
                "could not open local assets directory {}: {error}",
                root.display()
            ))
        })?;
        if !root.is_dir() {
            return Err(BifrostError::Config(format!(
                "--local-assets must point to a directory: {}",
                root.display()
            )));
        }

        let mut archives_by_version: BTreeMap<String, Vec<(String, PathBuf)>> = BTreeMap::new();
        for entry in fs::read_dir(&root).map_err(BifrostError::Io)? {
            let entry = entry.map_err(BifrostError::Io)?;
            let file_type = entry.file_type().map_err(BifrostError::Io)?;
            if !file_type.is_file() {
                continue;
            }
            // Release asset names are ASCII. Lossy conversion leaves an
            // invalid native filename unable to match the required pattern.
            let file_name = entry.file_name().to_string_lossy().into_owned();
            for extension in SUPPORTED_ARCHIVE_EXTENSIONS {
                let prefix = "bifrost-v";
                let suffix = format!("-{target}.{extension}");
                let Some(version) = file_name
                    .strip_prefix(prefix)
                    .and_then(|name| name.strip_suffix(&suffix))
                else {
                    continue;
                };
                if bifrost_core::version_check::bifrost_version_from_release_tag(&format!(
                    "v{version}"
                ))
                .is_none()
                {
                    return Err(BifrostError::Config(format!(
                        "local CLI archive has an invalid Bifrost version: {file_name}"
                    )));
                }
                archives_by_version
                    .entry(version.to_string())
                    .or_default()
                    .push((extension.to_string(), entry.path()));
            }
        }

        if archives_by_version.is_empty() {
            return Err(BifrostError::NotFound(format!(
                "no local CLI archive found for {target} in {}; expected bifrost-v<VERSION>-{target}.zip (or .tar.xz/.tar.gz)",
                root.display()
            )));
        }
        if archives_by_version.len() != 1 {
            let versions = archives_by_version.keys().cloned().collect::<BTreeSet<_>>();
            return Err(BifrostError::Config(format!(
                "local assets directory must contain exactly one target version for {target}; found: {}",
                versions.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }

        let (version, archives) = archives_by_version.into_iter().next().expect("one version");
        if archives.len() != 1 {
            return Err(BifrostError::Config(format!(
                "local assets directory must contain exactly one CLI archive for v{version} and {target}; found {}",
                archives.len()
            )));
        }
        let (_, cli_archive) = archives.into_iter().next().expect("one archive");
        ensure_nonempty_regular_file(&cli_archive, "local CLI archive")?;
        validate_downloaded_archive(
            &cli_archive,
            archive_ext_from_path(&cli_archive).expect("discovered supported archive"),
        )?;

        let desktop_package = local_desktop_asset_name(&version, target)
            .map(|name| root.join(name))
            .filter(|path| path.exists())
            .map(|path| {
                ensure_nonempty_regular_file(&path, "local desktop package")?;
                Ok::<_, BifrostError>(path)
            })
            .transpose()?;

        Ok(Self {
            root,
            version,
            cli_archive,
            desktop_package,
        })
    }

    fn activate(&self) -> Result<LocalUpgradeEnvironment, BifrostError> {
        LocalUpgradeEnvironment::activate(self)
    }
}

fn canonicalize_local_asset_root(root: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        // std::fs::canonicalize returns a `\\?\` verbatim path on Windows.
        // PowerShell can consume it, but msiexec returns 1619 when that path is
        // forwarded as the MSI package argument. dunce preserves normal drive
        // and UNC spelling whenever the path does not require verbatim syntax.
        dunce::canonicalize(root)
    }
    #[cfg(not(windows))]
    {
        fs::canonicalize(root)
    }
}

pub(super) struct LocalUpgradeContext {
    assets: Option<LocalUpgradeAssets>,
    _environment: Option<LocalUpgradeEnvironment>,
}

impl LocalUpgradeContext {
    pub(super) fn prepare(root: Option<PathBuf>) -> Result<Self, BifrostError> {
        let assets = root.map(LocalUpgradeAssets::discover).transpose()?;
        let environment = assets
            .as_ref()
            .map(LocalUpgradeAssets::activate)
            .transpose()?;
        if let Some(assets) = &assets {
            println!(
                "{} {}",
                "Using local release assets:".bright_cyan(),
                assets.root.display()
            );
            println!(
                "{} v{}",
                "Local target version:".bright_cyan(),
                assets.version
            );
        }
        Ok(Self {
            assets,
            _environment: environment,
        })
    }

    pub(super) fn is_active(&self) -> bool {
        self.assets.is_some()
    }

    pub(super) fn require_desktop_package_if_needed(
        &self,
        skip_app: bool,
    ) -> Result<(), BifrostError> {
        if !skip_app {
            if let Some(assets) = &self.assets {
                assets.require_desktop_package_if_installed()?;
            }
        }
        Ok(())
    }
}

pub(super) fn ensure_local_assets_install_method_is_safe(
    install_method: &InstallMethod,
) -> Result<(), BifrostError> {
    match install_method {
        InstallMethod::Script | InstallMethod::Manual(_) => Ok(()),
        InstallMethod::Homebrew | InstallMethod::Npm | InstallMethod::Pnpm => {
            Err(BifrostError::Config(format!(
            "--local-assets cannot update a {install_method}-owned CLI without contacting its package source. Run the rehearsal from a standalone or install-script binary instead; normal upgrades without --local-assets continue to use {install_method}."
        )))
        }
        InstallMethod::Unknown => Err(BifrostError::Config(
            "--local-assets requires a standalone or install-script Bifrost CLI, but the current executable's installation method could not be determined."
                .to_string(),
        )),
    }
}

fn local_desktop_asset_name(version: &str, target: &str) -> Option<String> {
    let extension = if target.ends_with("-pc-windows-msvc") {
        "msi"
    } else if target.ends_with("-apple-darwin") {
        "dmg"
    } else {
        return None;
    };
    Some(format!("bifrost-desktop-v{version}-{target}.{extension}"))
}

fn ensure_nonempty_regular_file(path: &Path, label: &str) -> Result<(), BifrostError> {
    let metadata = fs::symlink_metadata(path).map_err(BifrostError::Io)?;
    if !metadata.file_type().is_file() {
        return Err(BifrostError::Config(format!(
            "{label} must be a regular file (symlinks are not accepted): {}",
            path.display()
        )));
    }
    if metadata.len() == 0 {
        return Err(BifrostError::Config(format!(
            "{label} is empty: {}",
            path.display()
        )));
    }
    Ok(())
}

struct LocalUpgradeEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl LocalUpgradeEnvironment {
    fn activate(assets: &LocalUpgradeAssets) -> Result<Self, BifrostError> {
        let previous = LOCAL_ASSET_ENV_KEYS
            .iter()
            .map(|key| (*key, env::var_os(key)))
            .collect();
        env::set_var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES", "1");
        env::set_var("BIFROST_UPGRADE_TEST_LATEST_VERSION", &assets.version);
        env::set_var("BIFROST_UPGRADE_TEST_ARCHIVE", &assets.cli_archive);
        match &assets.desktop_package {
            Some(path) => env::set_var("BIFROST_APP_UPGRADE_TEST_PACKAGE", path),
            None => env::remove_var("BIFROST_APP_UPGRADE_TEST_PACKAGE"),
        }
        Ok(Self { previous })
    }
}

impl Drop for LocalUpgradeEnvironment {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_archive(root: &Path, version: &str, target: &str) -> PathBuf {
        let archive = root.join(format!("bifrost-v{version}-{target}.zip"));
        fs::write(&archive, b"local archive fixture").expect("write archive fixture");
        archive
    }

    #[test]
    fn local_assets_reject_package_manager_owned_installations_before_upgrade() {
        for method in [
            InstallMethod::Homebrew,
            InstallMethod::Npm,
            InstallMethod::Pnpm,
        ] {
            let error = ensure_local_assets_install_method_is_safe(&method)
                .expect_err("local assets must not invoke a remote package manager");
            assert!(error
                .to_string()
                .contains("without contacting its package source"));
            assert!(error.to_string().contains("without --local-assets"));
        }

        assert!(ensure_local_assets_install_method_is_safe(&InstallMethod::Script).is_ok());
        assert!(
            ensure_local_assets_install_method_is_safe(&InstallMethod::Manual(PathBuf::from(
                "/tmp/bifrost"
            )))
            .is_ok()
        );
        let unknown = ensure_local_assets_install_method_is_safe(&InstallMethod::Unknown)
            .expect_err("unknown local install targets must fail closed");
        assert!(unknown.to_string().contains("could not be determined"));
    }

    #[test]
    fn discovers_one_release_formatted_windows_asset_set() {
        let root = tempfile::tempdir().expect("assets dir");
        let target = "aarch64-pc-windows-msvc";
        let version = "0.0.181-local.7";
        let archive = create_archive(root.path(), version, target);
        let desktop = root
            .path()
            .join(format!("bifrost-desktop-v{version}-{target}.msi"));
        fs::write(&desktop, b"local msi fixture").expect("write desktop fixture");

        let assets = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect("discover local assets");
        assert_eq!(assets.version, version);
        assert_eq!(
            assets.cli_archive,
            canonicalize_local_asset_root(&archive).expect("canonical archive")
        );
        assert_eq!(
            assets.desktop_package,
            Some(canonicalize_local_asset_root(&desktop).expect("canonical desktop package"))
        );
        #[cfg(windows)]
        assert!(
            !assets.root.to_string_lossy().starts_with(r"\\?\"),
            "local MSI paths must remain compatible with msiexec"
        );
    }

    #[test]
    fn rejects_missing_and_ambiguous_cli_archives() {
        let root = tempfile::tempdir().expect("assets dir");
        let target = "aarch64-pc-windows-msvc";
        assert!(
            LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target).is_err()
        );

        create_archive(root.path(), "0.0.181-local.1", target);
        create_archive(root.path(), "0.0.181-local.2", target);
        let error = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect_err("multiple versions must fail");
        assert!(error.to_string().contains("exactly one target version"));
    }

    #[test]
    fn rejects_multiple_archive_formats_for_the_same_version() {
        let root = tempfile::tempdir().expect("assets dir");
        let target = "aarch64-pc-windows-msvc";
        let version = "0.0.181-local.3";
        create_archive(root.path(), version, target);
        fs::write(
            root.path()
                .join(format!("bifrost-v{version}-{target}.tar.gz")),
            b"stale archive fixture",
        )
        .expect("write second archive format");

        let error = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect_err("multiple formats for one version must fail");
        assert!(error.to_string().contains("exactly one CLI archive"));
    }

    #[test]
    fn local_environment_is_scoped_and_restores_previous_values() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = tempfile::tempdir().expect("assets dir");
        let target = "aarch64-pc-windows-msvc";
        let version = "0.0.181-local.9";
        create_archive(root.path(), version, target);
        let assets = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect("discover local assets");
        let previous = LOCAL_ASSET_ENV_KEYS
            .iter()
            .map(|key| (*key, env::var_os(key)))
            .collect::<Vec<_>>();
        env::set_var("BIFROST_UPGRADE_TEST_LATEST_VERSION", "previous-version");

        {
            let _environment = assets.activate().expect("activate local assets");
            assert_eq!(
                env::var("BIFROST_UPGRADE_TEST_LATEST_VERSION").unwrap(),
                version
            );
            assert_eq!(
                env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE").as_deref(),
                Some(assets.cli_archive.as_os_str())
            );
            assert_eq!(env::var_os("BIFROST_APP_UPGRADE_TEST_PACKAGE"), None);
        }

        for (key, value) in previous {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    #[test]
    fn desktop_asset_names_match_release_conventions() {
        assert_eq!(
            local_desktop_asset_name("0.0.181-local.1", "aarch64-pc-windows-msvc"),
            Some("bifrost-desktop-v0.0.181-local.1-aarch64-pc-windows-msvc.msi".to_string())
        );
        assert_eq!(
            local_desktop_asset_name("0.0.181-local.1", "aarch64-apple-darwin"),
            Some("bifrost-desktop-v0.0.181-local.1-aarch64-apple-darwin.dmg".to_string())
        );
        assert_eq!(
            local_desktop_asset_name("0.0.181-local.1", "x86_64-unknown-linux-gnu"),
            None
        );
    }

    #[test]
    fn prepare_discovers_current_target_and_scopes_all_release_overrides() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = tempfile::tempdir().expect("assets dir");
        let target = get_target_triple().expect("supported test target");
        let version = "0.0.181-local.10";
        create_archive(root.path(), version, target);
        let expected_desktop = local_desktop_asset_name(version, target).map(|desktop_name| {
            let desktop = root.path().join(desktop_name);
            fs::write(&desktop, b"local desktop fixture").expect("write desktop fixture");
            canonicalize_local_asset_root(&desktop).expect("canonical desktop package")
        });
        let previous = LOCAL_ASSET_ENV_KEYS
            .iter()
            .map(|key| (*key, env::var_os(key)))
            .collect::<Vec<_>>();

        {
            let context = LocalUpgradeContext::prepare(Some(root.path().to_path_buf()))
                .expect("prepare current-target local assets");
            assert!(context.is_active());
            context
                .require_desktop_package_if_needed(false)
                .expect("desktop package is present");
            assert_eq!(
                env::var_os("BIFROST_APP_UPGRADE_TEST_PACKAGE").as_deref(),
                expected_desktop.as_deref().map(Path::as_os_str)
            );
        }
        assert!(!LocalUpgradeContext::prepare(None)
            .expect("prepare without local assets")
            .is_active());

        for (key, value) in previous {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    #[test]
    fn discovery_rejects_invalid_roots_versions_and_empty_files() {
        let target = "aarch64-pc-windows-msvc";
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing");
        let error = LocalUpgradeAssets::discover_for_target(missing, target)
            .expect_err("missing root must fail");
        assert!(error
            .to_string()
            .contains("could not open local assets directory"));

        let root_file = temp.path().join("assets-file");
        fs::write(&root_file, b"not a directory").expect("write root file");
        let error = LocalUpgradeAssets::discover_for_target(root_file, target)
            .expect_err("non-directory root must fail");
        assert!(error.to_string().contains("must point to a directory"));

        let invalid_version_root = tempfile::tempdir().expect("invalid version root");
        create_archive(invalid_version_root.path(), "not-a-version", target);
        let error = LocalUpgradeAssets::discover_for_target(
            invalid_version_root.path().to_path_buf(),
            target,
        )
        .expect_err("invalid release version must fail");
        assert!(error.to_string().contains("invalid Bifrost version"));

        let empty_root = tempfile::tempdir().expect("empty archive root");
        let archive = empty_root
            .path()
            .join(format!("bifrost-v0.0.181-local.11-{target}.zip"));
        fs::write(&archive, []).expect("write empty archive");
        let error =
            LocalUpgradeAssets::discover_for_target(empty_root.path().to_path_buf(), target)
                .expect_err("empty archive must fail");
        assert!(error.to_string().contains("local CLI archive is empty"));
    }

    #[test]
    fn discovery_ignores_non_files_and_rejects_invalid_desktop_packages() {
        let target = "aarch64-pc-windows-msvc";
        let version = "0.0.181-local.12";
        let root = tempfile::tempdir().expect("assets dir");
        fs::create_dir(root.path().join(format!("bifrost-v0.0.0-{target}.zip")))
            .expect("create archive-shaped directory");
        create_archive(root.path(), version, target);
        let desktop = root
            .path()
            .join(format!("bifrost-desktop-v{version}-{target}.msi"));
        fs::write(&desktop, []).expect("write empty desktop package");

        let error = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect_err("empty desktop package must fail");
        assert!(error.to_string().contains("local desktop package is empty"));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_desktop_symlinks() {
        use std::os::unix::fs::symlink;

        let target = "aarch64-pc-windows-msvc";
        let version = "0.0.181-local.13";
        let root = tempfile::tempdir().expect("assets dir");
        let archive = create_archive(root.path(), version, target);
        let desktop = root
            .path()
            .join(format!("bifrost-desktop-v{version}-{target}.msi"));
        symlink(&archive, &desktop).expect("create desktop package symlink");

        let error = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect_err("desktop package symlink must fail");
        assert!(error.to_string().contains("must be a regular file"));
    }

    #[test]
    fn installed_desktop_requires_matching_local_package_unless_app_is_skipped() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = tempfile::tempdir().expect("assets dir");
        let target = get_target_triple().expect("supported test target");
        let version = "0.0.181-local.14";
        create_archive(root.path(), version, target);
        let assets = LocalUpgradeAssets::discover_for_target(root.path().to_path_buf(), target)
            .expect("discover CLI-only local assets");
        let install_root = tempfile::tempdir().expect("desktop install root");
        let previous_install_dir = env::var_os("BIFROST_APP_INSTALL_DIR");
        env::set_var("BIFROST_APP_INSTALL_DIR", install_root.path());
        let installed_path = desktop_app_install_candidates()
            .into_iter()
            .next()
            .expect("desktop install candidate");
        fs::create_dir_all(&installed_path).expect("create installed desktop fixture");

        let context = LocalUpgradeContext {
            assets: Some(assets),
            _environment: None,
        };
        context
            .require_desktop_package_if_needed(true)
            .expect("--skip-app does not require desktop package");
        let error = context
            .require_desktop_package_if_needed(false)
            .expect_err("installed desktop requires local package");
        assert!(error
            .to_string()
            .contains("installed desktop target package"));

        match previous_install_dir {
            Some(value) => env::set_var("BIFROST_APP_INSTALL_DIR", value),
            None => env::remove_var("BIFROST_APP_INSTALL_DIR"),
        }
    }

    #[test]
    fn rejects_unsupported_targets() {
        let root = tempfile::tempdir().expect("assets dir");
        let error =
            LocalUpgradeAssets::discover_for_detected_target(root.path().to_path_buf(), None)
                .expect_err("unsupported target must fail");
        assert!(error.to_string().contains("not supported"));
    }

    #[test]
    fn handle_upgrade_rejects_cli_only_assets_before_locking_or_downloading() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let assets_root = tempfile::tempdir().expect("assets dir");
        let target = get_target_triple().expect("supported test target");
        create_archive(assets_root.path(), "0.0.181-local.15", target);
        let install_root = tempfile::tempdir().expect("desktop install root");
        let previous_install_dir = env::var_os("BIFROST_APP_INSTALL_DIR");
        let previous_worker = env::var_os(bifrost_core::EXTERNAL_CLI_WORKER_ENV);
        env::set_var("BIFROST_APP_INSTALL_DIR", install_root.path());
        env::remove_var(bifrost_core::EXTERNAL_CLI_WORKER_ENV);
        let installed_path = desktop_app_install_candidates()
            .into_iter()
            .next()
            .expect("desktop install candidate");
        fs::create_dir_all(installed_path).expect("create installed desktop fixture");

        let error = handle_upgrade(false, Some(assets_root.path().to_path_buf()))
            .expect_err("CLI-only assets cannot upgrade an installed desktop");
        assert!(error
            .to_string()
            .contains("installed desktop target package"));

        match previous_install_dir {
            Some(value) => env::set_var("BIFROST_APP_INSTALL_DIR", value),
            None => env::remove_var("BIFROST_APP_INSTALL_DIR"),
        }
        match previous_worker {
            Some(value) => env::set_var(bifrost_core::EXTERNAL_CLI_WORKER_ENV, value),
            None => env::remove_var(bifrost_core::EXTERNAL_CLI_WORKER_ENV),
        }
    }
}
