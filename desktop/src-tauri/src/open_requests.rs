use serde::Serialize;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_BIFROST_OPEN_FILE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DesktopOpenRequest {
    Route {
        route: String,
        source: DesktopOpenSource,
    },
    BifrostFile {
        path: String,
        filename: String,
        content: String,
        source: DesktopOpenSource,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopOpenSource {
    DeepLink,
    FileAssociation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRequestParseError {
    UnsupportedScheme(String),
    UnsupportedRoute(String),
    UnsupportedFileExtension(PathBuf),
    FileTooLarge { path: PathBuf, size: u64 },
    ReadFile { path: PathBuf, error: String },
    FileUrlToPath(String),
}

impl fmt::Display for OpenRequestParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "unsupported scheme {scheme}")
            }
            Self::UnsupportedRoute(route) => write!(formatter, "unsupported route {route}"),
            Self::UnsupportedFileExtension(path) => {
                write!(
                    formatter,
                    "unsupported file extension for {}",
                    path.display()
                )
            }
            Self::FileTooLarge { path, size } => write!(
                formatter,
                "file {} is too large to open: {size} bytes",
                path.display()
            ),
            Self::ReadFile { path, error } => {
                write!(formatter, "failed to read {}: {error}", path.display())
            }
            Self::FileUrlToPath(url) => write!(formatter, "failed to convert file URL {url}"),
        }
    }
}

pub fn parse_open_url(
    url: &tauri::Url,
) -> Result<Option<DesktopOpenRequest>, OpenRequestParseError> {
    match url.scheme() {
        "bifrost" => parse_bifrost_deep_link(url).map(Some),
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|_| OpenRequestParseError::FileUrlToPath(url.to_string()))?;
            read_bifrost_file_request(&path).map(Some)
        }
        scheme => Err(OpenRequestParseError::UnsupportedScheme(scheme.to_string())),
    }
}

fn parse_bifrost_deep_link(url: &tauri::Url) -> Result<DesktopOpenRequest, OpenRequestParseError> {
    let route = route_from_bifrost_deep_link(url)?;
    Ok(DesktopOpenRequest::Route {
        route,
        source: DesktopOpenSource::DeepLink,
    })
}

pub fn route_from_bifrost_deep_link(url: &tauri::Url) -> Result<String, OpenRequestParseError> {
    let host = url.host_str().unwrap_or_default();
    let path = url.path().trim_matches('/');
    let target = if host.eq_ignore_ascii_case("open") {
        path
    } else if !host.is_empty() {
        host
    } else {
        path
    };

    let route = match target {
        "traffic" => "/traffic",
        "rules" => "/rules",
        "settings" => "/settings",
        other => return Err(OpenRequestParseError::UnsupportedRoute(other.to_string())),
    };

    let mut route = route.to_string();
    if let Some(query) = url.query().filter(|query| !query.trim().is_empty()) {
        route.push('?');
        route.push_str(query);
    }
    Ok(route)
}

fn read_bifrost_file_request(path: &Path) -> Result<DesktopOpenRequest, OpenRequestParseError> {
    let extension_matches = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bifrost"));
    if !extension_matches {
        return Err(OpenRequestParseError::UnsupportedFileExtension(
            path.to_path_buf(),
        ));
    }

    let mut file = File::open(path).map_err(|error| OpenRequestParseError::ReadFile {
        path: path.to_path_buf(),
        error: error.to_string(),
    })?;
    let size = file.metadata().map(|meta| meta.len()).map_err(|error| {
        OpenRequestParseError::ReadFile {
            path: path.to_path_buf(),
            error: error.to_string(),
        }
    })?;
    if size > MAX_BIFROST_OPEN_FILE_BYTES {
        return Err(OpenRequestParseError::FileTooLarge {
            path: path.to_path_buf(),
            size,
        });
    }

    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|error| OpenRequestParseError::ReadFile {
            path: path.to_path_buf(),
            error: error.to_string(),
        })?;
    Ok(DesktopOpenRequest::BifrostFile {
        path: path.to_string_lossy().to_string(),
        filename: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("import.bifrost")
            .to_string(),
        content,
        source: DesktopOpenSource::FileAssociation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bifrost_deep_link_host_maps_to_route() {
        let url = "bifrost://traffic".parse::<tauri::Url>().unwrap();
        assert_eq!(route_from_bifrost_deep_link(&url).unwrap(), "/traffic");
    }

    #[test]
    fn bifrost_deep_link_open_path_maps_to_route_with_query() {
        let url = "bifrost://open/settings?tab=tls"
            .parse::<tauri::Url>()
            .unwrap();
        assert_eq!(
            route_from_bifrost_deep_link(&url).unwrap(),
            "/settings?tab=tls"
        );
    }

    #[test]
    fn bifrost_deep_link_rejects_unknown_route() {
        let url = "bifrost://open/admin".parse::<tauri::Url>().unwrap();
        assert!(matches!(
            route_from_bifrost_deep_link(&url),
            Err(OpenRequestParseError::UnsupportedRoute(route)) if route == "admin"
        ));
    }

    #[test]
    fn file_url_reads_bifrost_file_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.bifrost");
        std::fs::write(&path, "rules payload").unwrap();
        let url = tauri::Url::from_file_path(&path).unwrap();

        let request = parse_open_url(&url).unwrap().unwrap();

        assert_eq!(
            request,
            DesktopOpenRequest::BifrostFile {
                path: path.to_string_lossy().to_string(),
                filename: "rules.bifrost".to_string(),
                content: "rules payload".to_string(),
                source: DesktopOpenSource::FileAssociation,
            }
        );
    }
}
