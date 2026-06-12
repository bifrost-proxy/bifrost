use crate::rule::types::Rule;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParseErrorSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableInfo {
    pub name: String,
    pub source: String,
    pub defined_at: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeFix {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<(usize, usize)>,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    pub line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub message: String,
    pub severity: ParseErrorSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_info: Option<VariableInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fixes: Vec<CodeFix>,
}

impl ParseError {
    pub fn new(line: usize, content: &str, message: impl Into<String>) -> Self {
        let content_len = content.len().max(1);
        Self {
            line,
            start_column: 1,
            end_column: content_len,
            message: message.into(),
            severity: ParseErrorSeverity::Error,
            suggestion: None,
            code: None,
            related_info: None,
            fixes: Vec::new(),
        }
    }

    pub fn with_range(
        line: usize,
        start_column: usize,
        end_column: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            line,
            start_column,
            end_column,
            message: message.into(),
            severity: ParseErrorSeverity::Error,
            suggestion: None,
            code: None,
            related_info: None,
            fixes: Vec::new(),
        }
    }

    pub fn with_severity(mut self, severity: ParseErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_related_info(mut self, info: VariableInfo) -> Self {
        self.related_info = Some(info);
        self
    }

    pub fn with_fix(mut self, fix: CodeFix) -> Self {
        self.fixes.push(fix);
        self
    }

    pub fn with_fixes(mut self, fixes: Vec<CodeFix>) -> Self {
        self.fixes = fixes;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    pub rules: Vec<Rule>,
    pub errors: Vec<ParseError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptReference {
    pub name: String,
    pub script_type: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub rule_count: usize,
    pub errors: Vec<ParseError>,
    pub warnings: Vec<ParseError>,
    pub defined_variables: Vec<VariableInfo>,
    #[serde(default)]
    pub script_references: Vec<ScriptReference>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_new_sets_columns_from_content_len() {
        let err = ParseError::new(3, "hello", "boom");
        assert_eq!(err.line, 3);
        assert_eq!(err.start_column, 1);
        assert_eq!(err.end_column, 5);
        assert_eq!(err.message, "boom");
        assert_eq!(err.severity, ParseErrorSeverity::Error);
        assert!(err.suggestion.is_none());
        assert!(err.code.is_none());
        assert!(err.related_info.is_none());
        assert!(err.fixes.is_empty());
    }

    #[test]
    fn parse_error_new_empty_content_floors_to_one() {
        let err = ParseError::new(1, "", "msg");
        assert_eq!(err.end_column, 1);
    }

    #[test]
    fn parse_error_with_range_sets_explicit_columns() {
        let err = ParseError::with_range(7, 2, 9, "ranged");
        assert_eq!(err.line, 7);
        assert_eq!(err.start_column, 2);
        assert_eq!(err.end_column, 9);
        assert_eq!(err.message, "ranged");
    }

    #[test]
    fn parse_error_builder_chain_sets_all_fields() {
        let info = VariableInfo {
            name: "v".into(),
            source: "src".into(),
            defined_at: Some(4),
        };
        let fix = CodeFix {
            title: "fix it".into(),
            range: Some((1, 2)),
            new_text: "replacement".into(),
        };
        let err = ParseError::new(1, "x", "m")
            .with_severity(ParseErrorSeverity::Warning)
            .with_suggestion("try this")
            .with_code("E001")
            .with_related_info(info.clone())
            .with_fix(fix.clone());

        assert_eq!(err.severity, ParseErrorSeverity::Warning);
        assert_eq!(err.suggestion.as_deref(), Some("try this"));
        assert_eq!(err.code.as_deref(), Some("E001"));
        assert_eq!(err.related_info.as_ref().unwrap().name, "v");
        assert_eq!(err.fixes.len(), 1);
        assert_eq!(err.fixes[0].title, "fix it");
    }

    #[test]
    fn parse_error_with_fixes_replaces_vec() {
        let fixes = vec![
            CodeFix {
                title: "a".into(),
                range: None,
                new_text: "x".into(),
            },
            CodeFix {
                title: "b".into(),
                range: None,
                new_text: "y".into(),
            },
        ];
        let err = ParseError::new(1, "x", "m").with_fixes(fixes);
        assert_eq!(err.fixes.len(), 2);
        assert_eq!(err.fixes[1].title, "b");
    }

    #[test]
    fn parse_result_and_validation_result_defaults() {
        let pr = ParseResult::default();
        assert!(pr.rules.is_empty());
        assert!(pr.errors.is_empty());

        let vr = ValidationResult::default();
        assert!(!vr.valid);
        assert_eq!(vr.rule_count, 0);
        assert!(vr.script_references.is_empty());
    }

    #[test]
    fn severity_serializes_lowercase() {
        let json = serde_json::to_string(&ParseErrorSeverity::Hint).unwrap();
        assert_eq!(json, "\"hint\"");
        let back: ParseErrorSeverity = serde_json::from_str("\"info\"").unwrap();
        assert_eq!(back, ParseErrorSeverity::Info);
    }
}
