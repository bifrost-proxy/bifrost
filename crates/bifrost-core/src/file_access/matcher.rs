//! Glob + deny pattern matching helpers.
//!
//! We deliberately keep the pattern surface small (POSIX-style globs with `**`
//! segment wildcard) to avoid drift between the server-side policy engine and
//! the CLI `--dry-run` checker.

use std::path::Path;

use crate::file_access::error::FileAccessError;

/// A compiled set of positive glob patterns. Used for `roots`-relative
/// allowlists such as the `file.glob` command's `pattern` argument.
#[derive(Debug, Clone)]
pub struct GlobMatcher {
    patterns: Vec<CompiledGlob>,
}

/// A compiled set of deny patterns. Matches are checked against the
/// root-relative POSIX-style path.
#[derive(Debug, Clone)]
pub struct DenyMatcher {
    patterns: Vec<CompiledGlob>,
}

#[derive(Debug, Clone)]
struct CompiledGlob {
    raw: String,
    regex: regex::Regex,
}

impl GlobMatcher {
    pub fn new(patterns: &[String]) -> Result<Self, FileAccessError> {
        let compiled = patterns
            .iter()
            .map(|p| CompiledGlob::compile(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { patterns: compiled })
    }

    /// Returns `true` if any pattern matches the given root-relative path.
    pub fn is_match<P: AsRef<Path>>(&self, rel: P) -> bool {
        let s = to_posix(rel.as_ref());
        self.patterns.iter().any(|p| p.regex.is_match(&s))
    }

    /// Returns all raw patterns, useful for diagnostics / audit logs.
    pub fn patterns(&self) -> impl Iterator<Item = &str> {
        self.patterns.iter().map(|p| p.raw.as_str())
    }
}

impl DenyMatcher {
    pub fn new(patterns: &[String]) -> Result<Self, FileAccessError> {
        let compiled = patterns
            .iter()
            .map(|p| CompiledGlob::compile(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { patterns: compiled })
    }

    /// Returns the first matching raw pattern, if any. Returning the raw
    /// pattern is intentional: we want the audit log to record *which* deny
    /// rule fired.
    pub fn match_raw<P: AsRef<Path>>(&self, rel: P) -> Option<&str> {
        let s = to_posix(rel.as_ref());
        self.patterns
            .iter()
            .find(|p| p.regex.is_match(&s))
            .map(|p| p.raw.as_str())
    }
}

impl CompiledGlob {
    fn compile(raw: &str) -> Result<Self, FileAccessError> {
        let regex_src = glob_to_regex(raw);
        let regex = regex::Regex::new(&regex_src).map_err(|e| FileAccessError::InvalidGlob {
            pattern: raw.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self {
            raw: raw.to_string(),
            regex,
        })
    }
}

/// Convert POSIX-style glob to regex.
///
/// Supported tokens:
/// - `?`    — any single non-slash char
/// - `*`    — any run of non-slash chars
/// - `**`   — any run including slashes
/// - `[...]`— character class (passed through; caller must ensure validity)
///
/// The resulting regex is anchored (`^...$`) so callers only need
/// `is_match`.
fn glob_to_regex(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len() * 2 + 4);
    out.push('^');
    let bytes = pat.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(".*");
                    i += 2;
                } else {
                    out.push_str("[^/]*");
                    i += 1;
                }
            }
            b'?' => {
                out.push_str("[^/]");
                i += 1;
            }
            b'.' | b'+' | b'(' | b')' | b'|' | b'^' | b'$' | b'{' | b'}' | b'\\' => {
                out.push('\\');
                out.push(bytes[i] as char);
                i += 1;
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out.push('$');
    out
}

fn to_posix(p: &Path) -> String {
    // Normalize Windows `\` to `/` for cross-platform pattern parity.
    p.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_does_not_cross_slash() {
        let m = DenyMatcher::new(&["*.key".into()]).unwrap();
        assert!(m.match_raw("foo.key").is_some());
        assert!(m.match_raw("a/foo.key").is_none());
    }

    #[test]
    fn double_star_crosses_slash() {
        let m = DenyMatcher::new(&["**/*.key".into()]).unwrap();
        assert!(m.match_raw("foo.key").is_some());
        assert!(m.match_raw("a/b/foo.key").is_some());
    }

    #[test]
    fn git_dir_denied() {
        let m = DenyMatcher::new(&["**/.git/**".into()]).unwrap();
        assert!(m.match_raw(".git/config").is_some());
        assert!(m.match_raw("a/.git/hooks/pre-commit").is_some());
        assert!(m.match_raw("src/main.rs").is_none());
    }
}
