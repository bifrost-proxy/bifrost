use std::fmt;

#[derive(Debug)]
pub enum BifrostError {
    Config(String),
    Parse(String),
    Rule(String),
    Proxy(String),
    Tls(String),
    Io(std::io::Error),
    Network(String),
    NotFound(String),
    AlreadyExists(String),
    Storage(String),
}

impl fmt::Display for BifrostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BifrostError::Config(msg) => write!(f, "Config error: {}", msg),
            BifrostError::Parse(msg) => write!(f, "Parse error: {}", msg),
            BifrostError::Rule(msg) => write!(f, "Rule error: {}", msg),
            BifrostError::Proxy(msg) => write!(f, "Proxy error: {}", msg),
            BifrostError::Tls(msg) => write!(f, "TLS error: {}", msg),
            BifrostError::Io(err) => write!(f, "IO error: {}", err),
            BifrostError::Network(msg) => write!(f, "Network error: {}", msg),
            BifrostError::NotFound(msg) => write!(f, "Not found: {}", msg),
            BifrostError::AlreadyExists(msg) => write!(f, "Already exists: {}", msg),
            BifrostError::Storage(msg) => write!(f, "Storage error: {}", msg),
        }
    }
}

impl std::error::Error for BifrostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BifrostError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BifrostError {
    fn from(err: std::io::Error) -> Self {
        BifrostError::Io(err)
    }
}

impl From<serde_json::Error> for BifrostError {
    fn from(err: serde_json::Error) -> Self {
        BifrostError::Parse(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, BifrostError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn display_for_all_variants() {
        assert_eq!(
            BifrostError::Config("c".into()).to_string(),
            "Config error: c"
        );
        assert_eq!(
            BifrostError::Parse("p".into()).to_string(),
            "Parse error: p"
        );
        assert_eq!(BifrostError::Rule("r".into()).to_string(), "Rule error: r");
        assert_eq!(
            BifrostError::Proxy("x".into()).to_string(),
            "Proxy error: x"
        );
        assert_eq!(BifrostError::Tls("t".into()).to_string(), "TLS error: t");
        assert_eq!(
            BifrostError::Network("n".into()).to_string(),
            "Network error: n"
        );
        assert_eq!(
            BifrostError::NotFound("nf".into()).to_string(),
            "Not found: nf"
        );
        assert_eq!(
            BifrostError::AlreadyExists("ae".into()).to_string(),
            "Already exists: ae"
        );
        assert_eq!(
            BifrostError::Storage("s".into()).to_string(),
            "Storage error: s"
        );
        let io = BifrostError::Io(std::io::Error::other("disk"));
        assert!(io.to_string().starts_with("IO error: "));
    }

    #[test]
    fn source_is_some_only_for_io() {
        let io = BifrostError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "x"));
        assert!(io.source().is_some());
        assert!(BifrostError::Config("c".into()).source().is_none());
    }

    #[test]
    fn from_io_and_serde_json() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let be: BifrostError = io_err.into();
        assert!(matches!(be, BifrostError::Io(_)));

        let json_err = serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err();
        let be: BifrostError = json_err.into();
        assert!(matches!(be, BifrostError::Parse(_)));
    }
}
