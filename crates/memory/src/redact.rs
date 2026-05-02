use regex::Regex;
use std::sync::OnceLock;

/// 对长期记忆写入内容执行敏感信息脱敏。
pub struct Redactor {
    rules: &'static [Regex],
}

impl Redactor {
    /// 创建默认脱敏器。
    pub fn new() -> Self {
        Self {
            rules: default_rules(),
        }
    }

    /// 返回脱敏后的文本。
    pub fn redact(&self, input: &str) -> String {
        let mut output = input.to_string();
        for rule in self.rules {
            output = rule.replace_all(&output, "<REDACTED>").to_string();
        }
        output
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

fn default_rules() -> &'static [Regex] {
    static RULES: OnceLock<Vec<Regex>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            Regex::new(r"sk-[A-Za-z0-9]{20,}").expect("valid sk regex"),
            Regex::new(r"ghp_[A-Za-z0-9_]{20,}").expect("valid ghp regex"),
            Regex::new(r"AIza[A-Za-z0-9_-]{20,}").expect("valid google api regex"),
            Regex::new(r"Bearer\s+[A-Za-z0-9\._-]+").expect("valid bearer regex"),
            Regex::new(r"\b[A-Za-z0-9+/]{32,}={0,2}").expect("valid base64 regex"),
            Regex::new(r"(?i)\bpassword\s*=\s*[^&\s]+").expect("valid password regex"),
            Regex::new(r"(?i)\btoken\s*=\s*[^&\s]+").expect("valid token regex"),
            Regex::new(r"(?i)\bapi[_-]?key\s*[:=]\s*[^&\s]+").expect("valid api key regex"),
            Regex::new(r"BF-[A-F0-9]{16}").expect("valid bifrost device code regex"),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redact(input: &str) -> String {
        Redactor::new().redact(input)
    }

    #[test]
    fn redacts_openai_style_secret() {
        assert_eq!(
            redact("key sk-abcdefghijklmnopqrstuvwxyz"),
            "key <REDACTED>"
        );
    }

    #[test]
    fn redacts_github_token() {
        assert_eq!(
            redact("token ghp_abcdefghijklmnopqrstuvwxyz123456"),
            "token <REDACTED>"
        );
    }

    #[test]
    fn redacts_google_api_key() {
        assert_eq!(redact("AIzaabcdefghijklmnopqrstuvwxyz123456"), "<REDACTED>");
    }

    #[test]
    fn redacts_bearer_token() {
        assert_eq!(redact("Bearer abc.def-ghi_jkl"), "<REDACTED>");
    }

    #[test]
    fn redacts_long_base64() {
        assert_eq!(redact("YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo="), "<REDACTED>");
    }

    #[test]
    fn redacts_password_assignment() {
        assert_eq!(redact("password=hunter2"), "<REDACTED>");
    }

    #[test]
    fn redacts_token_assignment() {
        assert_eq!(redact("token=abc123"), "<REDACTED>");
    }

    #[test]
    fn redacts_api_key_assignment() {
        assert_eq!(redact("api_key: abc123"), "<REDACTED>");
        assert_eq!(redact("apikey=abc123"), "<REDACTED>");
    }

    #[test]
    fn redacts_bifrost_device_code() {
        assert_eq!(redact("BF-ABCDEF0123456789"), "<REDACTED>");
    }

    #[test]
    fn leaves_regular_text_unchanged() {
        assert_eq!(
            redact("remember project preference"),
            "remember project preference"
        );
    }
}
