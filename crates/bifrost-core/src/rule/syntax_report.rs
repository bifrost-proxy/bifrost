use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{
    expand_rule_references_strict, validate_rules_with_context, ParseError, RuleReferenceError,
    ScriptReference, ValidationResult, VariableInfo,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleSyntaxGuidance {
    pub summary: String,
    pub retryable: bool,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSyntaxReport {
    pub valid: bool,
    pub rule_count: usize,
    pub errors: Vec<ParseError>,
    pub warnings: Vec<ParseError>,
    pub defined_variables: Vec<VariableInfo>,
    #[serde(default)]
    pub script_references: Vec<ScriptReference>,
    pub guidance: RuleSyntaxGuidance,
}

impl RuleSyntaxReport {
    pub fn from_validation_result(result: ValidationResult) -> Self {
        let mut report = Self {
            valid: result.valid,
            rule_count: result.rule_count,
            errors: result.errors,
            warnings: result.warnings,
            defined_variables: result.defined_variables,
            script_references: result.script_references,
            guidance: RuleSyntaxGuidance {
                summary: String::new(),
                retryable: false,
                next_actions: Vec::new(),
            },
        };
        report.refresh_guidance();
        report
    }

    pub fn from_reference_error(
        source_name: &str,
        original_content: &str,
        error: RuleReferenceError,
    ) -> Self {
        let (error_line, error_end_column) = if error.source() == source_name {
            let end_column = original_content
                .lines()
                .nth(error.line().saturating_sub(1))
                .map(|line| line.len().max(1))
                .unwrap_or(1);
            (error.line(), end_column)
        } else {
            (1, 1)
        };
        let suggestion = match &error {
            RuleReferenceError::Missing { name, .. } => {
                format!("Create or sync rule '{name}', fix the @ reference name, or remove this reference line.")
            }
            RuleReferenceError::Cycle { .. } => {
                "Remove the cyclic @ reference chain before enabling or saving this rule."
                    .to_string()
            }
        };
        let reference_error =
            ParseError::with_range(error_line, 1, error_end_column, error.to_string())
                .with_code("E020")
                .with_suggestion(suggestion);
        let mut report = Self {
            valid: false,
            rule_count: 0,
            errors: vec![reference_error],
            warnings: Vec::new(),
            defined_variables: Vec::new(),
            script_references: Vec::new(),
            guidance: RuleSyntaxGuidance {
                summary: String::new(),
                retryable: true,
                next_actions: Vec::new(),
            },
        };
        report.refresh_guidance();
        report
    }

    pub fn refresh_guidance(&mut self) {
        self.valid = self.errors.is_empty();
        self.guidance = build_guidance(self.valid, self.errors.len(), self.warnings.len());
    }
}

pub fn validate_rule_syntax_report(
    source_name: &str,
    content: &str,
    reference_catalog: &HashMap<String, String>,
    global_values: &HashMap<String, String>,
) -> RuleSyntaxReport {
    let expanded = match expand_rule_references_strict(source_name, content, reference_catalog) {
        Ok(expanded) => expanded,
        Err(error) => return RuleSyntaxReport::from_reference_error(source_name, content, error),
    };

    RuleSyntaxReport::from_validation_result(validate_rules_with_context(&expanded, global_values))
}

fn build_guidance(valid: bool, error_count: usize, warning_count: usize) -> RuleSyntaxGuidance {
    if !valid {
        let plural = if error_count == 1 { "" } else { "s" };
        return RuleSyntaxGuidance {
            summary: format!("Fix {error_count} rule syntax error{plural} before retrying this save."),
            retryable: true,
            next_actions: vec![
                "Read syntax.errors[].line, syntax.errors[].message, and syntax.errors[].suggestion.".to_string(),
                "Edit the rule content, then retry the same create or update request.".to_string(),
            ],
        };
    }

    if warning_count > 0 {
        let plural = if warning_count == 1 { "" } else { "s" };
        return RuleSyntaxGuidance {
            summary: format!("Rule syntax is valid with {warning_count} warning{plural}."),
            retryable: false,
            next_actions: vec![
                "Review syntax.warnings[] to improve the rule, but the rule can be saved."
                    .to_string(),
            ],
        };
    }

    RuleSyntaxGuidance {
        summary: "Rule syntax is valid.".to_string(),
        retryable: false,
        next_actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_rule_returns_success_guidance() {
        let mut catalog = HashMap::new();
        catalog.insert(
            "demo".to_string(),
            "example.com host://127.0.0.1:3000".to_string(),
        );
        let report =
            validate_rule_syntax_report("demo", &catalog["demo"], &catalog, &HashMap::new());

        assert!(report.valid);
        assert_eq!(report.rule_count, 1);
        assert!(report.errors.is_empty());
        assert_eq!(report.guidance.summary, "Rule syntax is valid.");
        assert!(!report.guidance.retryable);
        assert!(report.guidance.next_actions.is_empty());
    }

    #[test]
    fn missing_reference_returns_e020_guidance() {
        let mut catalog = HashMap::new();
        catalog.insert("entry".to_string(), "@missing-shared".to_string());
        let report =
            validate_rule_syntax_report("entry", &catalog["entry"], &catalog, &HashMap::new());

        assert!(!report.valid);
        assert_eq!(report.rule_count, 0);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].code.as_deref(), Some("E020"));
        assert!(report.guidance.retryable);
        assert!(report.guidance.summary.contains("Fix 1 rule syntax error"));
    }
}
