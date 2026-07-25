#[cfg(target_os = "macos")]
use libloading::{Library, Symbol};
use std::path::{Path, PathBuf};

const WIDGET_RELOAD_HOST: &str = "widget-reload";
#[cfg(target_os = "macos")]
const WIDGET_BRIDGE_LIBRARY: &str = "libBifrostWidgetBridge.dylib";

pub fn is_widget_reload_url(url: &tauri::Url) -> bool {
    url.scheme() == "bifrost"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(WIDGET_RELOAD_HOST))
        && url.path().trim_matches('/').is_empty()
}

#[cfg(target_os = "macos")]
pub fn reload_status_widget() -> Result<(), String> {
    let bridge = widget_bridge_path()?;
    unsafe {
        let library = Library::new(&bridge).map_err(|error| {
            format!(
                "failed to load WidgetKit bridge {}: {error}",
                bridge.display()
            )
        })?;
        let reload: Symbol<unsafe extern "C" fn()> = library
            .get(b"bifrost_reload_status_widget\0")
            .map_err(|error| {
                format!(
                    "failed to resolve WidgetKit bridge function in {}: {error}",
                    bridge.display()
                )
            })?;
        reload();
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn reload_status_widget() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn widget_bridge_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to resolve desktop executable path: {error}"))?;
    installed_widget_bridge_path(&executable)
        .filter(|path| path.is_file())
        .or_else(|| {
            let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("bin")
                .join(WIDGET_BRIDGE_LIBRARY);
            development.is_file().then_some(development)
        })
        .ok_or_else(|| {
            format!(
                "WidgetKit bridge is missing beside desktop executable {}",
                executable.display()
            )
        })
}

#[cfg(target_os = "macos")]
fn installed_widget_bridge_path(executable: &Path) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    let contents_dir = macos_dir.parent()?;
    Some(
        contents_dir
            .join("Resources")
            .join("resources")
            .join("bin")
            .join(WIDGET_BRIDGE_LIBRARY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_the_internal_widget_reload_url() {
        let reload = "bifrost://widget-reload".parse::<tauri::Url>().unwrap();
        let settings = "bifrost://settings".parse::<tauri::Url>().unwrap();
        let nested = "bifrost://widget-reload/unexpected"
            .parse::<tauri::Url>()
            .unwrap();

        assert!(is_widget_reload_url(&reload));
        assert!(!is_widget_reload_url(&settings));
        assert!(!is_widget_reload_url(&nested));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resolves_bridge_inside_an_installed_app_bundle() {
        let executable = Path::new("/Applications/Bifrost.app/Contents/MacOS/bifrost-desktop");
        assert_eq!(
            installed_widget_bridge_path(executable).unwrap(),
            Path::new(
                "/Applications/Bifrost.app/Contents/Resources/resources/bin/libBifrostWidgetBridge.dylib"
            )
        );
    }
}
