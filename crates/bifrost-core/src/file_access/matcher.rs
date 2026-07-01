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
    tokens: Vec<GlobToken>,
    case_insensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobToken {
    Literal(char),
    Star,
    DoubleStar,
    DoubleStarSlash,
    Question,
    Class(CharClass),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CharClass {
    negated: bool,
    items: Vec<CharClassItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CharClassItem {
    Char(char),
    Range(char, char),
}

impl GlobMatcher {
    pub fn new(patterns: &[String]) -> Result<Self, FileAccessError> {
        // Positive allowlist matching stays case-sensitive: an allowlist must
        // never be widened by case folding.
        let compiled = patterns
            .iter()
            .map(|p| CompiledGlob::compile(p, false))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { patterns: compiled })
    }

    /// Returns `true` if any pattern matches the given root-relative path.
    pub fn is_match<P: AsRef<Path>>(&self, rel: P) -> bool {
        let s = to_posix(rel.as_ref());
        self.patterns.iter().any(|p| p.is_match(&s))
    }

    /// Returns all raw patterns, useful for diagnostics / audit logs.
    pub fn patterns(&self) -> impl Iterator<Item = &str> {
        self.patterns.iter().map(|p| p.raw.as_str())
    }
}

impl DenyMatcher {
    pub fn new(patterns: &[String]) -> Result<Self, FileAccessError> {
        // Deny patterns are compiled case-insensitively so that secret/VCS
        // files (e.g. `.GIT/config`, `ID_RSA`, `config.KEY`, `.ENV`) are still
        // blocked on case-insensitive filesystems such as macOS (APFS/HFS+)
        // and Windows (NTFS), where the on-disk name's case can differ from the
        // pattern. A deny-list must never be *narrowed* by case sensitivity.
        let compiled = patterns
            .iter()
            .map(|p| CompiledGlob::compile(p, true))
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
            .find(|p| p.is_match(&s))
            .map(|p| p.raw.as_str())
    }
}

impl CompiledGlob {
    fn compile(raw: &str, case_insensitive: bool) -> Result<Self, FileAccessError> {
        let pattern = if case_insensitive {
            raw.to_lowercase()
        } else {
            raw.to_string()
        };
        let tokens = parse_glob(&pattern).map_err(|reason| FileAccessError::InvalidGlob {
            pattern: raw.to_string(),
            reason,
        })?;
        Ok(Self {
            raw: raw.to_string(),
            tokens,
            case_insensitive,
        })
    }

    fn is_match(&self, rel_posix: &str) -> bool {
        let input = if self.case_insensitive {
            rel_posix.to_lowercase()
        } else {
            rel_posix.to_string()
        };
        glob_match(&self.tokens, &input)
    }
}

fn parse_glob(pat: &str) -> Result<Vec<GlobToken>, String> {
    let chars: Vec<char> = pat.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    if i + 2 < chars.len() && chars[i + 2] == '/' {
                        out.push(GlobToken::DoubleStarSlash);
                        i += 3;
                    } else {
                        out.push(GlobToken::DoubleStar);
                        i += 2;
                    }
                } else {
                    out.push(GlobToken::Star);
                    i += 1;
                }
            }
            '?' => {
                out.push(GlobToken::Question);
                i += 1;
            }
            '[' => {
                let (class, next) = parse_char_class(&chars, i)?;
                out.push(GlobToken::Class(class));
                i = next;
            }
            c => {
                out.push(GlobToken::Literal(c));
                i += 1;
            }
        }
    }
    Ok(out)
}

fn parse_char_class(chars: &[char], start: usize) -> Result<(CharClass, usize), String> {
    debug_assert_eq!(chars.get(start), Some(&'['));
    let mut i = start + 1;
    if i >= chars.len() {
        return Err("unterminated character class".to_string());
    }
    let negated = matches!(chars[i], '!' | '^');
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    while i < chars.len() {
        if chars[i] == ']' {
            if items.is_empty() {
                return Err("empty character class".to_string());
            }
            return Ok((CharClass { negated, items }, i + 1));
        }
        let first = chars[i];
        if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
            let last = chars[i + 2];
            if first > last {
                return Err(format!("invalid character range {}-{}", first, last));
            }
            items.push(CharClassItem::Range(first, last));
            i += 3;
        } else {
            items.push(CharClassItem::Char(first));
            i += 1;
        }
    }
    Err("unterminated character class".to_string())
}

fn glob_match(tokens: &[GlobToken], input: &str) -> bool {
    let chars: Vec<char> = input.chars().collect();
    let mut memo = vec![vec![None; chars.len() + 1]; tokens.len() + 1];
    glob_match_at(tokens, &chars, 0, 0, &mut memo)
}

fn glob_match_at(
    tokens: &[GlobToken],
    chars: &[char],
    ti: usize,
    ci: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(hit) = memo[ti][ci] {
        return hit;
    }
    let matched = if ti == tokens.len() {
        ci == chars.len()
    } else {
        match &tokens[ti] {
            GlobToken::Literal(ch) => {
                ci < chars.len()
                    && chars[ci] == *ch
                    && glob_match_at(tokens, chars, ti + 1, ci + 1, memo)
            }
            GlobToken::Question => {
                ci < chars.len()
                    && chars[ci] != '/'
                    && glob_match_at(tokens, chars, ti + 1, ci + 1, memo)
            }
            GlobToken::Star => {
                glob_match_at(tokens, chars, ti + 1, ci, memo)
                    || (ci < chars.len()
                        && chars[ci] != '/'
                        && glob_match_at(tokens, chars, ti, ci + 1, memo))
            }
            GlobToken::DoubleStar => {
                glob_match_at(tokens, chars, ti + 1, ci, memo)
                    || (ci < chars.len() && glob_match_at(tokens, chars, ti, ci + 1, memo))
            }
            GlobToken::DoubleStarSlash => {
                if glob_match_at(tokens, chars, ti + 1, ci, memo) {
                    true
                } else {
                    let mut j = ci;
                    let mut hit = false;
                    while j < chars.len() {
                        if chars[j] == '/' && glob_match_at(tokens, chars, ti + 1, j + 1, memo) {
                            hit = true;
                            break;
                        }
                        j += 1;
                    }
                    hit
                }
            }
            GlobToken::Class(class) => {
                ci < chars.len()
                    && class.matches(chars[ci])
                    && glob_match_at(tokens, chars, ti + 1, ci + 1, memo)
            }
        }
    };
    memo[ti][ci] = Some(matched);
    matched
}

impl CharClass {
    fn matches(&self, ch: char) -> bool {
        let hit = self.items.iter().any(|item| match item {
            CharClassItem::Char(item_ch) => *item_ch == ch,
            CharClassItem::Range(start, end) => *start <= ch && ch <= *end,
        });
        if self.negated {
            !hit
        } else {
            hit
        }
    }
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

    #[test]
    fn glob_matcher_is_match_and_patterns() {
        let m = GlobMatcher::new(&["src/*.rs".into(), "docs/**".into()]).unwrap();
        assert!(m.is_match("src/main.rs"));
        assert!(!m.is_match("src/sub/main.rs")); // * does not cross slash
        assert!(m.is_match("docs/a/b.md"));
        let raw: Vec<&str> = m.patterns().collect();
        assert_eq!(raw, vec!["src/*.rs", "docs/**"]);
    }

    #[test]
    fn question_mark_matches_single_non_slash() {
        let m = GlobMatcher::new(&["file?.txt".into()]).unwrap();
        assert!(m.is_match("file1.txt"));
        assert!(!m.is_match("file.txt"));
        assert!(!m.is_match("file12.txt"));
    }

    #[test]
    fn special_regex_chars_are_escaped_literally() {
        // dots and plus must be matched literally, not as regex metachars
        let m = GlobMatcher::new(&["a.b+c.txt".into()]).unwrap();
        assert!(m.is_match("a.b+c.txt"));
        assert!(!m.is_match("aXbXc.txt"));
    }

    #[test]
    fn trailing_double_star_matches_rest() {
        // ** not followed by '/' → matches anything including slashes
        let m = GlobMatcher::new(&["logs/**".into()]).unwrap();
        assert!(m.is_match("logs/x/y/z.log"));
        assert!(m.is_match("logs/"));
    }

    #[test]
    fn invalid_glob_returns_error() {
        // an unterminated character class produces an invalid regex
        let err = GlobMatcher::new(&["[".into()]).unwrap_err();
        assert!(matches!(err, FileAccessError::InvalidGlob { .. }));
        assert_eq!(err.code(), "file.invalid_glob");
    }

    #[test]
    fn to_posix_normalizes_backslashes() {
        assert_eq!(to_posix(Path::new("a\\b\\c")), "a/b/c");
    }

    #[test]
    fn deny_is_case_insensitive_for_secrets() {
        // On case-insensitive filesystems (macOS/Windows) the on-disk name's
        // case may differ from the deny pattern. Deny must still fire.
        let m = DenyMatcher::new(&[
            "**/.git/**".into(),
            "**/id_rsa*".into(),
            "**/*.key".into(),
            "**/.env".into(),
        ])
        .unwrap();
        assert!(m.match_raw(".GIT/config").is_some());
        assert!(m.match_raw("a/.Git/HEAD").is_some());
        assert!(m.match_raw("ID_RSA").is_some());
        assert!(m.match_raw("secrets/Config.KEY").is_some());
        assert!(m.match_raw(".ENV").is_some());
        // Non-secret paths must still pass.
        assert!(m.match_raw("src/main.rs").is_none());
    }

    #[test]
    fn glob_allowlist_stays_case_sensitive() {
        // A positive allowlist must NOT be widened by case folding.
        let m = GlobMatcher::new(&["src/*.rs".into()]).unwrap();
        assert!(m.is_match("src/main.rs"));
        assert!(!m.is_match("SRC/main.rs"));
        assert!(!m.is_match("src/MAIN.RS"));
    }
}
