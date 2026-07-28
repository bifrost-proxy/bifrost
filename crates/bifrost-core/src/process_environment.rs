use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Internal process-role marker set only on the explicit external CLI worker.
///
/// Long-lived Bifrost processes must remove this variable from child commands so
/// an inherited worker role cannot become ambient application state.
pub const EXTERNAL_CLI_WORKER_ENV: &str = "BIFROST_EXTERNAL_CLI_WORKER";

pub fn inherited_executable_path() -> Option<OsString> {
    let current_path = env::var_os("PATH");
    let home_dir = dirs::home_dir();
    let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    augment_executable_path(
        current_path.as_deref(),
        home_dir.as_deref(),
        local_app_data.as_deref(),
    )
}

pub fn augment_executable_path(
    current_path: Option<&OsStr>,
    home_dir: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Option<OsString> {
    let mut entries = current_path
        .map(env::split_paths)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut seen = entries.iter().cloned().collect::<HashSet<_>>();

    for entry in user_executable_directories(home_dir, local_app_data) {
        if seen.insert(entry.clone()) {
            entries.push(entry);
        }
    }

    if entries.is_empty() {
        return None;
    }
    env::join_paths(entries).ok()
}

fn user_executable_directories(
    home_dir: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut entries = Vec::new();

    #[cfg(unix)]
    {
        if let Some(home_dir) = home_dir {
            entries.extend([
                home_dir.join(".local/bin"),
                home_dir.join(".cargo/bin"),
                home_dir.join(".bifrost/bin"),
            ]);
        }
        entries.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = local_app_data {
            entries.push(local_app_data.join("bifrost/bin"));
        }
        if let Some(home_dir) = home_dir {
            entries.extend([
                home_dir.join(".local/bin"),
                home_dir.join(".cargo/bin"),
                home_dir.join(".bifrost/bin"),
            ]);
        }
    }

    #[cfg(not(windows))]
    let _ = local_app_data;

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn executable_path_preserves_existing_order_and_adds_user_bins() {
        let home_dir = Path::new("/Users/example");
        let current = env::join_paths(["/usr/bin", "/bin"]).expect("current path");
        let augmented =
            augment_executable_path(Some(&current), Some(home_dir), None).expect("augmented path");
        let entries = env::split_paths(&augmented).collect::<Vec<_>>();

        assert_eq!(entries[0], PathBuf::from("/usr/bin"));
        assert_eq!(entries[1], PathBuf::from("/bin"));
        assert!(entries.contains(&home_dir.join(".local/bin")));
        assert!(entries.contains(&home_dir.join(".cargo/bin")));
        assert!(entries.contains(&home_dir.join(".bifrost/bin")));
    }

    #[cfg(unix)]
    #[test]
    fn executable_path_deduplicates_existing_user_bin() {
        let home_dir = Path::new("/Users/example");
        let local_bin = home_dir.join(".local/bin");
        let current =
            env::join_paths([PathBuf::from("/usr/bin"), local_bin.clone()]).expect("current path");
        let augmented =
            augment_executable_path(Some(&current), Some(home_dir), None).expect("augmented path");
        let entries = env::split_paths(&augmented).collect::<Vec<_>>();

        assert_eq!(
            entries.iter().filter(|entry| **entry == local_bin).count(),
            1
        );
    }

    #[cfg(windows)]
    #[test]
    fn executable_path_adds_windows_user_bins() {
        let home_dir = Path::new(r"C:\Users\example");
        let local_app_data = Path::new(r"C:\Users\example\AppData\Local");
        let current = env::join_paths([r"C:\Windows\System32"]).expect("current path");
        let augmented =
            augment_executable_path(Some(&current), Some(home_dir), Some(local_app_data))
                .expect("augmented path");
        let entries = env::split_paths(&augmented).collect::<Vec<_>>();

        assert!(entries.contains(&home_dir.join(".local/bin")));
        assert!(entries.contains(&home_dir.join(".cargo/bin")));
        assert!(entries.contains(&home_dir.join(".bifrost/bin")));
        assert!(entries.contains(&local_app_data.join("bifrost/bin")));
    }
}
