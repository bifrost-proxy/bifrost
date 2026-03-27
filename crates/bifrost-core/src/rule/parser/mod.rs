use crate::error::{BifrostError, Result};
use crate::matcher::factory::parse_pattern;
use crate::protocol::Protocol;
use crate::rule::filter::{parse_filter, parse_line_props, Filter, LineProps};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

use super::types::Rule;
use super::ValueStore;

mod types;

pub use types::{
    CodeFix, ParseError, ParseErrorSeverity, ParseResult, ScriptReference, ValidationResult,
    VariableInfo,
};

const INCLUDE_FILTER_PREFIX: &str = "includeFilter://";
const EXCLUDE_FILTER_PREFIX: &str = "excludeFilter://";
const LINE_PROPS_PREFIX: &str = "lineProps://";

type ParsedPatternResult = (
    Vec<String>,
    Vec<(Protocol, String)>,
    Vec<Filter>,
    Vec<Filter>,
    LineProps,
);

fn normalize_line_block_tokens(content: &str) -> String {
    let tokens: Vec<&str> = content.split_whitespace().collect();
    if tokens.len() < 2 {
        return content.to_string();
    }

    let mut patterns = Vec::new();
    let mut operations = Vec::new();

    for token in tokens {
        if token.starts_with(INCLUDE_FILTER_PREFIX)
            || token.starts_with(EXCLUDE_FILTER_PREFIX)
            || token.starts_with(LINE_PROPS_PREFIX)
        {
            operations.push(token);
            continue;
        }

        if let Some(caps) = PROTOCOL_REGEX.captures(token) {
            let proto_name = caps.get(1).unwrap().as_str();
            if Protocol::parse(proto_name).is_some()
                || Protocol::parse(Protocol::resolve_alias(proto_name)).is_some()
            {
                operations.push(token);
                continue;
            }
        }

        patterns.push(token);
    }

    if patterns.is_empty() || operations.is_empty() {
        return content.to_string();
    }

    let normalized = patterns.into_iter().chain(operations).collect::<Vec<_>>();

    normalized.join(" ")
}

lazy_static::lazy_static! {
    static ref PROTOCOL_REGEX: Regex = Regex::new(r"^([a-zA-Z][a-zA-Z0-9\-]*)://(.*)$").unwrap();
    static ref INLINE_VALUES_REGEX: Regex = Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_.\-]*)\}").unwrap();
    static ref HOST_PORT_REGEX: Regex = Regex::new(r"^(localhost|\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}|\[[:0-9a-fA-F]+\]):(\d+)(/.*)?$").unwrap();
    static ref DOMAIN_LIKE_PATTERN_REGEX: Regex =
        Regex::new(r"^(?:\*\.)?(?:[A-Za-z0-9-]+\.)+[A-Za-z0-9-]+(?::\d+)?(?:/.*)?$").unwrap();
    static ref BARE_HOST_PATH_TARGET_REGEX: Regex = Regex::new(
        r"^((localhost)|(\d{1,3}(?:\.\d{1,3}){3})|(\[[0-9A-Fa-f:]+\])|(([A-Za-z0-9-]+\.)+[A-Za-z0-9-]+))(?::\d+)?([/?#].*)$"
    ).unwrap();
}

pub struct RuleParser {
    values: HashMap<String, String>,
}

impl RuleParser {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn with_values(values: HashMap<String, String>) -> Self {
        Self { values }
    }

    pub fn from_store(store: &dyn ValueStore) -> Self {
        Self {
            values: store.as_hashmap(),
        }
    }

    pub fn set_value(&mut self, key: String, value: String) {
        self.values.insert(key, value);
    }

    pub fn merge_from_store(&mut self, store: &dyn ValueStore) {
        for (k, v) in store.list() {
            self.values.entry(k).or_insert(v);
        }
    }

    pub fn values(&self) -> &HashMap<String, String> {
        &self.values
    }

    pub fn parse_line(&self, line: &str) -> Result<Vec<Rule>> {
        parse_line_with_values(line, &self.values)
    }

    pub fn parse_rules(&self, text: &str) -> Result<Vec<Rule>> {
        parse_rules_with_values(text, &self.values)
    }

    pub fn parse_rules_with_inline_values(
        &self,
        text: &str,
    ) -> Result<(Vec<Rule>, HashMap<String, String>)> {
        let mut merged_values = self.values.clone();
        let text = extract_markdown_value_blocks(text, &mut merged_values);

        let rules = parse_rules_with_values(&text, &merged_values)?;
        let inline_values: HashMap<String, String> = merged_values
            .into_iter()
            .filter(|(k, _)| !self.values.contains_key(k))
            .collect();

        Ok((rules, inline_values))
    }

    pub fn parse_rules_tolerant(&self, text: &str) -> ParseResult {
        let mut merged_values = self.values.clone();
        let text = extract_markdown_value_blocks(text, &mut merged_values);
        parse_rules_tolerant_with_values(&text, &merged_values)
    }

    pub fn parse_rules_tolerant_with_inline_values(
        &self,
        text: &str,
    ) -> (ParseResult, HashMap<String, String>) {
        let mut merged_values = self.values.clone();
        let text = extract_markdown_value_blocks(text, &mut merged_values);

        let result = parse_rules_tolerant_with_values(&text, &merged_values);
        let inline_values: HashMap<String, String> = merged_values
            .into_iter()
            .filter(|(k, _)| !self.values.contains_key(k))
            .collect();

        (result, inline_values)
    }

    pub fn validate_rules(&self, text: &str) -> Vec<ParseError> {
        self.parse_rules_tolerant(text).errors
    }
}

impl Default for RuleParser {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_line(line: &str) -> Result<Vec<Rule>> {
    parse_line_with_values(line, &HashMap::new())
}

pub fn parse_rules(text: &str) -> Result<Vec<Rule>> {
    parse_rules_with_values(text, &HashMap::new())
}

pub fn parse_rules_tolerant(text: &str) -> ParseResult {
    parse_rules_tolerant_with_values(text, &HashMap::new())
}

pub fn validate_rules(text: &str) -> Vec<ParseError> {
    validate_rules_with_context(text, &HashMap::new()).errors
}

pub fn validate_rules_with_context(
    text: &str,
    global_values: &HashMap<String, String>,
) -> ValidationResult {
    let mut result = ValidationResult::default();

    let mut local_values: HashMap<String, String> = HashMap::new();
    let mut value_definitions: HashMap<String, (usize, usize)> = HashMap::new();

    validate_code_blocks(text, &mut result, &mut local_values, &mut value_definitions);

    let mut merged_values = global_values.clone();
    merged_values.extend(local_values.clone());

    for name in merged_values.keys() {
        let (line, source) = if let Some((start_line, _)) = value_definitions.get(name) {
            (*start_line, "local".to_string())
        } else {
            (0, "global".to_string())
        };
        result.defined_variables.push(VariableInfo {
            name: name.clone(),
            source,
            defined_at: if line > 0 { Some(line) } else { None },
        });
    }

    let clean_text = extract_markdown_value_blocks(text, &mut HashMap::new());

    for (line_num, line) in clean_text.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        validate_variable_references(trimmed, line_num, &merged_values, &mut result);
        extract_script_references(trimmed, line_num, &mut result);
        validate_filter_values(trimmed, line_num, &mut result);
        validate_protocol_values(trimmed, line_num, &mut result);

        match parse_line_with_values(trimmed, &merged_values) {
            Ok(rules) => {
                result.rule_count += rules.len();
            }
            Err(e) => {
                let error = create_detailed_parse_error(line_num, trimmed, &e);
                result.errors.push(error);
            }
        }
    }

    result.valid = result.errors.is_empty();
    result
}

fn validate_filter_values(line: &str, line_num: usize, result: &mut ValidationResult) {
    let include_filter_re = regex::Regex::new(r"includeFilter://`?\(?([^)`\s]+)\)?`?").unwrap();
    let exclude_filter_re = regex::Regex::new(r"excludeFilter://`?\(?([^)`\s]+)\)?`?").unwrap();

    for cap in include_filter_re.captures_iter(line) {
        let filter_value = &cap[1];
        if let Err(e) = crate::syntax::validate_filter_value(filter_value) {
            let warning = ParseError::with_range(line_num, 1, line.len(), e.message.clone())
                .with_severity(ParseErrorSeverity::Warning)
                .with_code("W004")
                .with_suggestion(e.suggestion.unwrap_or_default());
            result.warnings.push(warning);
        }
    }

    for cap in exclude_filter_re.captures_iter(line) {
        let filter_value = &cap[1];
        if let Err(e) = crate::syntax::validate_filter_value(filter_value) {
            let warning = ParseError::with_range(line_num, 1, line.len(), e.message.clone())
                .with_severity(ParseErrorSeverity::Warning)
                .with_code("W004")
                .with_suggestion(e.suggestion.unwrap_or_default());
            result.warnings.push(warning);
        }
    }
}

fn extract_script_references(line: &str, line_num: usize, result: &mut ValidationResult) {
    let req_script_pattern = regex::Regex::new(r"reqScript://([^\s]+)").unwrap();
    let res_script_pattern = regex::Regex::new(r"resScript://([^\s]+)").unwrap();
    let decode_script_pattern = regex::Regex::new(r"decode://([^\s]+)").unwrap();

    for cap in req_script_pattern.captures_iter(line) {
        let script_name = cap[1].to_string();
        result.script_references.push(ScriptReference {
            name: script_name,
            script_type: "request".to_string(),
            line: line_num,
        });
    }

    for cap in res_script_pattern.captures_iter(line) {
        let script_name = cap[1].to_string();
        result.script_references.push(ScriptReference {
            name: script_name,
            script_type: "response".to_string(),
            line: line_num,
        });
    }

    for cap in decode_script_pattern.captures_iter(line) {
        let script_name = cap[1].to_string();
        result.script_references.push(ScriptReference {
            name: script_name,
            script_type: "decode".to_string(),
            line: line_num,
        });
    }
}

fn validate_code_blocks(
    text: &str,
    result: &mut ValidationResult,
    local_values: &mut HashMap<String, String>,
    value_definitions: &mut HashMap<String, (usize, usize)>,
) {
    let mut in_code_block = false;
    let mut code_block_start: usize = 0;
    let mut code_block_backticks: usize = 0;
    let mut code_block_name = String::new();
    let mut code_block_content = String::new();

    for (line_num, line) in text.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        if !in_code_block {
            if trimmed.starts_with("```") {
                let backtick_count = trimmed.chars().take_while(|c| *c == '`').count();
                if backtick_count >= 3 {
                    in_code_block = true;
                    code_block_start = line_num;
                    code_block_backticks = backtick_count;
                    code_block_name = trimmed[backtick_count..].trim().to_string();
                    code_block_content.clear();
                }
            }
        } else {
            let closing = "`".repeat(code_block_backticks);
            if trimmed == closing || trimmed.starts_with(&closing) {
                in_code_block = false;

                if !code_block_name.is_empty() {
                    let name_parts: Vec<&str> = code_block_name.split_whitespace().collect();
                    let var_name = name_parts.first().unwrap_or(&"").to_string();
                    if !var_name.is_empty() {
                        local_values.insert(var_name.clone(), code_block_content.clone());
                        value_definitions.insert(var_name, (code_block_start, line_num));
                    }
                }

                code_block_name.clear();
                code_block_content.clear();
            } else {
                if !code_block_content.is_empty() {
                    code_block_content.push('\n');
                }
                code_block_content.push_str(line);
            }
        }
    }

    if in_code_block {
        let error = ParseError::with_range(
            code_block_start,
            1,
            3 + code_block_name.len(),
            format!(
                "Unclosed code block starting at line {}. Missing closing '{}' delimiter.",
                code_block_start,
                "`".repeat(code_block_backticks)
            ),
        )
        .with_code("E005")
        .with_suggestion(format!(
            "Add '{}' on a new line to close the code block.",
            "`".repeat(code_block_backticks)
        ));
        result.errors.push(error);
    }
}

fn validate_variable_references(
    line: &str,
    line_num: usize,
    merged_values: &HashMap<String, String>,
    result: &mut ValidationResult,
) {
    let var_pattern = regex::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_\-.]*)\}").unwrap();

    for cap in var_pattern.captures_iter(line) {
        let full_match = cap.get(0).unwrap();
        let var_name = &cap[1];

        if line[..full_match.start()].ends_with('$') {
            continue;
        }

        if var_name.contains('.') {
            continue;
        }

        if !merged_values.contains_key(var_name) {
            let start_col = full_match.start() + 1;
            let end_col = full_match.end();

            let mut warning = ParseError::with_range(
                line_num,
                start_col,
                end_col,
                format!("Undefined variable reference: '{}'", var_name),
            )
            .with_severity(ParseErrorSeverity::Warning)
            .with_code("W001")
            .with_suggestion(format!(
                "Define the variable '{}' using a code block:\n```{}\nyour content here\n```",
                var_name, var_name
            ));

            let similar_vars: Vec<&String> = merged_values
                .keys()
                .filter(|k| k.to_lowercase().contains(&var_name.to_lowercase()))
                .collect();

            if !similar_vars.is_empty() {
                warning.suggestion = Some(format!(
                    "Did you mean one of these? {}. Or define a new variable '{}' using a code block.",
                    similar_vars
                        .iter()
                        .map(|s| format!("'{}'", s))
                        .collect::<Vec<_>>()
                        .join(", "),
                    var_name
                ));
            }

            result.warnings.push(warning);
        }
    }

    validate_parentheses_content(line, line_num, result);
}

fn validate_parentheses_content(line: &str, line_num: usize, result: &mut ValidationResult) {
    let paren_pattern = regex::Regex::new(r"://\(([^)]*)\)").unwrap();

    for cap in paren_pattern.captures_iter(line) {
        let content = &cap[1];
        let full_match = cap.get(0).unwrap();

        if content.contains(' ') && !content.starts_with('`') && !content.ends_with('`') {
            let proto_end = line[..full_match.start()]
                .rfind(char::is_whitespace)
                .map(|i| i + 1)
                .unwrap_or(0);
            let proto_part = &line[proto_end..full_match.end()];

            let start_col = full_match.start() + 4;
            let end_col = full_match.end() - 1;

            let warning = ParseError::with_range(
                line_num,
                start_col,
                end_col,
                format!(
                    "Parentheses content contains spaces: '{}'. This may cause parsing issues.",
                    content
                ),
            )
            .with_severity(ParseErrorSeverity::Warning)
            .with_code("W002")
            .with_suggestion(format!(
                "Use a block variable instead:\n1. Define: ```myVar\n{}\n```\n2. Use: {}://{{myVar}}",
                content,
                proto_part.split("://").next().unwrap_or("protocol")
            ));

            result.warnings.push(warning);
        }
    }
}

fn validate_protocol_values(line: &str, line_num: usize, result: &mut ValidationResult) {
    let protocol_pattern = regex::Regex::new(r"(\w+)://([^\s]+)").unwrap();

    for cap in protocol_pattern.captures_iter(line) {
        let protocol_name = &cap[1];
        let value = &cap[2];
        let value_start = cap.get(2).unwrap().start();
        let value_end = cap.get(2).unwrap().end();

        if let Some(error) =
            validate_single_protocol_value(protocol_name, value, line_num, value_start, value_end)
        {
            let is_duplicate = result.warnings.iter().any(|w| {
                w.line == line_num
                    && w.start_column == error.start_column
                    && w.end_column == error.end_column
            }) || result.errors.iter().any(|e| {
                e.line == line_num
                    && e.start_column == error.start_column
                    && e.end_column == error.end_column
            });

            if !is_duplicate {
                match error.severity {
                    ParseErrorSeverity::Error => result.errors.push(error),
                    _ => result.warnings.push(error),
                }
            }
        }
    }
}

fn validate_single_protocol_value(
    protocol_name: &str,
    value: &str,
    line_num: usize,
    value_start: usize,
    value_end: usize,
) -> Option<ParseError> {
    let clean_value = value
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim_start_matches('`')
        .trim_end_matches('`');

    if clean_value.starts_with('{') && clean_value.ends_with('}') {
        return None;
    }

    let start_col = value_start + 1;
    let end_col = value_end;

    match protocol_name.to_lowercase().as_str() {
        "statuscode" | "replacestatus" => {
            validate_status_code(clean_value, line_num, start_col, end_col)
        }
        "cache" => validate_cache_value(clean_value, line_num, start_col, end_col),
        "reqdelay" | "resdelay" => validate_delay_value(clean_value, line_num, start_col, end_col),
        "reqspeed" | "resspeed" => validate_speed_value(clean_value, line_num, start_col, end_col),
        "method" => validate_http_method(clean_value, line_num, start_col, end_col),
        "dns" => validate_ip_address(clean_value, line_num, start_col, end_col),
        "host" | "xhost" => validate_host_port(clean_value, line_num, start_col, end_col),
        _ => None,
    }
}

fn validate_status_code(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    match value.parse::<u16>() {
        Ok(code) if (100..600).contains(&code) => None,
        Ok(code) => Some(
            ParseError::with_range(line_num, start_col, end_col, format!("Invalid HTTP status code: {}. Status code must be between 100 and 599.", code))
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E010")
                .with_suggestion("Common status codes: 200 (OK), 201 (Created), 204 (No Content), 301 (Moved), 302 (Found), 400 (Bad Request), 401 (Unauthorized), 403 (Forbidden), 404 (Not Found), 500 (Server Error)")
                .with_fixes(vec![
                    CodeFix { title: "Change to 200 (OK)".to_string(), range: Some((start_col, end_col)), new_text: "200".to_string() },
                    CodeFix { title: "Change to 404 (Not Found)".to_string(), range: Some((start_col, end_col)), new_text: "404".to_string() },
                    CodeFix { title: "Change to 500 (Server Error)".to_string(), range: Some((start_col, end_col)), new_text: "500".to_string() },
                ])
        ),
        Err(_) => Some(
            ParseError::with_range(line_num, start_col, end_col, format!("Invalid status code value: '{}'. Expected a number.", value))
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E010")
                .with_suggestion("Status code must be a number between 100 and 599. Example: statusCode://200")
                .with_fixes(vec![
                    CodeFix { title: "Change to 200".to_string(), range: Some((start_col, end_col)), new_text: "200".to_string() },
                    CodeFix { title: "Change to 404".to_string(), range: Some((start_col, end_col)), new_text: "404".to_string() },
                ])
        ),
    }
}

fn validate_cache_value(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    let valid_keywords = ["no", "no-cache", "no-store"];
    if valid_keywords.contains(&value.to_lowercase().as_str()) {
        return None;
    }
    match value.parse::<u64>() {
        Ok(0) => Some(
            ParseError::with_range(line_num, start_col, end_col, "Cache duration cannot be 0. Use 'no' to disable caching.".to_string())
                .with_severity(ParseErrorSeverity::Warning)
                .with_code("E011")
                .with_suggestion("Use 'no' to disable caching, or specify seconds > 0. Example: cache://3600 (1 hour)")
        ),
        Ok(_) => None,
        Err(_) => Some(
            ParseError::with_range(line_num, start_col, end_col, format!("Invalid cache value: '{}'. Expected a number (seconds) or 'no'/'no-cache'/'no-store'.", value))
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E011")
                .with_suggestion("Valid values: number (seconds), 'no', 'no-cache', 'no-store'. Example: cache://3600")
        ),
    }
}

fn validate_delay_value(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    match value.parse::<i64>() {
        Ok(ms) if ms >= 0 => None,
        Ok(ms) => Some(
            ParseError::with_range(line_num, start_col, end_col, format!("Delay cannot be negative: {}ms.", ms))
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E012")
                .with_suggestion("Delay must be a non-negative number in milliseconds. Example: reqDelay://1000 (1 second)")
        ),
        Err(_) => Some(
            ParseError::with_range(line_num, start_col, end_col, format!("Invalid delay value: '{}'. Expected a number in milliseconds.", value))
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E012")
                .with_suggestion("Delay must be a number in milliseconds. Example: reqDelay://500")
        ),
    }
}

fn validate_speed_value(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    match value.parse::<u64>() {
        Ok(0) => Some(
            ParseError::with_range(line_num, start_col, end_col, "Speed limit cannot be 0.".to_string())
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E013")
                .with_suggestion("Speed must be greater than 0 (bytes per second). Example: reqSpeed://1024 (1 KB/s)")
        ),
        Ok(_) => None,
        Err(_) => Some(
            ParseError::with_range(line_num, start_col, end_col, format!("Invalid speed value: '{}'. Expected a number in bytes per second.", value))
                .with_severity(ParseErrorSeverity::Error)
                .with_code("E013")
                .with_suggestion("Speed must be a positive number (bytes per second). Example: resSpeed://10240 (10 KB/s)")
        ),
    }
}

fn validate_http_method(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    let valid_methods = [
        "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
    ];
    if valid_methods.contains(&value.to_uppercase().as_str()) {
        None
    } else {
        Some(
            ParseError::with_range(
                line_num,
                start_col,
                end_col,
                format!("Unknown HTTP method: '{}'. ", value),
            )
            .with_severity(ParseErrorSeverity::Warning)
            .with_code("E015")
            .with_suggestion(format!(
                "Standard HTTP methods: {}. Example: method://POST",
                valid_methods.join(", ")
            )),
        )
    }
}

fn validate_ip_address(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    use std::net::IpAddr;
    if value.parse::<IpAddr>().is_ok() {
        None
    } else {
        Some(
            ParseError::with_range(
                line_num,
                start_col,
                end_col,
                format!("Invalid IP address: '{}'.", value),
            )
            .with_severity(ParseErrorSeverity::Error)
            .with_code("E016")
            .with_suggestion(
                "Expected a valid IPv4 (e.g., 192.168.1.1) or IPv6 (e.g., ::1) address.",
            ),
        )
    }
}

fn validate_host_port(
    value: &str,
    line_num: usize,
    start_col: usize,
    end_col: usize,
) -> Option<ParseError> {
    if let Some(colon_pos) = value.rfind(':') {
        let port_str = &value[colon_pos + 1..];
        if !port_str.is_empty() {
            if let Ok(port) = port_str.parse::<u32>() {
                if port > 65535 {
                    return Some(
                        ParseError::with_range(
                            line_num,
                            start_col,
                            end_col,
                            format!("Port number out of range: {}. Maximum is 65535.", port),
                        )
                        .with_severity(ParseErrorSeverity::Error)
                        .with_code("E017")
                        .with_suggestion(
                            "Port must be between 0 and 65535. Example: host://example.com:8080",
                        ),
                    );
                }
            }
        }
    }
    None
}

fn parse_line_with_values(line: &str, values: &HashMap<String, String>) -> Result<Vec<Rule>> {
    let line = line.trim();

    if line.is_empty() || line.starts_with('#') {
        return Ok(vec![]);
    }

    let line = expand_inline_values(line, values);

    let parts = split_rule_parts(&line);
    if parts.is_empty() {
        return Ok(vec![]);
    }

    let (patterns, mut protocol_values, include_filters, exclude_filters, line_props) =
        extract_pattern_and_protocols(&parts)?;

    if protocol_values.is_empty() {
        let has_url_pattern = patterns.iter().any(|p| {
            p.starts_with("http://")
                || p.starts_with("https://")
                || p.starts_with("ws://")
                || p.starts_with("wss://")
                || HOST_PORT_REGEX.is_match(p)
        });
        if has_url_pattern {
            protocol_values.push((Protocol::Passthrough, String::new()));
        } else {
            return Err(BifrostError::Parse(format!(
                "No protocol found in rule: {}",
                line
            )));
        }
    }

    let mut rules = Vec::new();
    for pattern in patterns {
        let matcher = parse_pattern(&pattern)
            .map_err(|e| BifrostError::Parse(format!("Invalid pattern '{}': {}", pattern, e)))?;
        let matcher = Arc::from(matcher);

        for (protocol, value) in &protocol_values {
            let rule = Rule::new(
                pattern.clone(),
                Arc::clone(&matcher),
                *protocol,
                value.clone(),
                line.clone(),
            )
            .with_line_props(line_props.clone())
            .with_include_filters(include_filters.clone())
            .with_exclude_filters(exclude_filters.clone());
            rules.push(rule);
        }
    }

    Ok(rules)
}

fn parse_rules_with_values(text: &str, values: &HashMap<String, String>) -> Result<Vec<Rule>> {
    let mut merged_values = values.clone();
    let text = extract_markdown_value_blocks(text, &mut merged_values);

    let mut rules = Vec::new();
    let mut current_line = String::new();
    let mut start_line_num = 1;
    let mut in_line_block = false;
    let mut line_block_content = String::new();
    let mut line_block_start = 1;

    for (line_num, line) in text.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        if in_line_block {
            if trimmed == "`" {
                in_line_block = false;
                let block_line =
                    normalize_line_block_tokens(&line_block_content.trim().replace('\n', " "));
                let parsed = parse_line_with_values(&block_line, &merged_values)?;
                for mut rule in parsed {
                    rule.line = Some(line_block_start);
                    rules.push(rule);
                }
                line_block_content.clear();
            } else {
                line_block_content.push_str(trimmed);
                line_block_content.push('\n');
            }
            continue;
        }

        if trimmed.starts_with("line`") {
            in_line_block = true;
            line_block_start = line_num;
            let after_marker = trimmed.strip_prefix("line`").unwrap_or("");
            if !after_marker.is_empty() {
                line_block_content.push_str(after_marker);
                line_block_content.push('\n');
            }
            continue;
        }

        if let Some(stripped) = trimmed.strip_suffix('\\') {
            if current_line.is_empty() {
                start_line_num = line_num;
            }
            current_line.push_str(stripped);
            current_line.push(' ');
            continue;
        }

        if !current_line.is_empty() {
            current_line.push_str(trimmed);
            let parsed = parse_line_with_values(&current_line, &merged_values)?;
            for mut rule in parsed {
                rule.line = Some(start_line_num);
                rules.push(rule);
            }
            current_line.clear();
        } else {
            let parsed = parse_line_with_values(trimmed, &merged_values)?;
            for mut rule in parsed {
                rule.line = Some(line_num);
                rules.push(rule);
            }
        }
    }

    if !current_line.is_empty() {
        let parsed = parse_line_with_values(&current_line, &merged_values)?;
        for mut rule in parsed {
            rule.line = Some(start_line_num);
            rules.push(rule);
        }
    }

    if in_line_block && !line_block_content.is_empty() {
        let block_line = normalize_line_block_tokens(&line_block_content.trim().replace('\n', " "));
        let parsed = parse_line_with_values(&block_line, &merged_values)?;
        for mut rule in parsed {
            rule.line = Some(line_block_start);
            rules.push(rule);
        }
    }

    Ok(rules)
}

fn parse_rules_tolerant_with_values(text: &str, values: &HashMap<String, String>) -> ParseResult {
    let mut merged_values = values.clone();
    let text = extract_markdown_value_blocks(text, &mut merged_values);

    let mut result = ParseResult::default();
    let mut current_line = String::new();
    let mut start_line_num = 1;
    let mut in_line_block = false;
    let mut line_block_content = String::new();
    let mut line_block_start = 1;

    let try_parse_line = |line_content: &str, line_num: usize, result: &mut ParseResult| {
        match parse_line_with_values(line_content, &merged_values) {
            Ok(parsed) => {
                for mut rule in parsed {
                    rule.line = Some(line_num);
                    result.rules.push(rule);
                }
            }
            Err(e) => {
                let trimmed = line_content.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    let error = create_detailed_parse_error(line_num, line_content, &e);
                    result.errors.push(error);
                }
            }
        }
    };

    for (line_num, line) in text.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        if in_line_block {
            if trimmed == "`" {
                in_line_block = false;
                let block_line =
                    normalize_line_block_tokens(&line_block_content.trim().replace('\n', " "));
                try_parse_line(&block_line, line_block_start, &mut result);
                line_block_content.clear();
            } else {
                line_block_content.push_str(trimmed);
                line_block_content.push('\n');
            }
            continue;
        }

        if trimmed.starts_with("line`") {
            in_line_block = true;
            line_block_start = line_num;
            let after_marker = trimmed.strip_prefix("line`").unwrap_or("");
            if !after_marker.is_empty() {
                line_block_content.push_str(after_marker);
                line_block_content.push('\n');
            }
            continue;
        }

        if let Some(stripped) = trimmed.strip_suffix('\\') {
            if current_line.is_empty() {
                start_line_num = line_num;
            }
            current_line.push_str(stripped);
            current_line.push(' ');
            continue;
        }

        if !current_line.is_empty() {
            current_line.push_str(trimmed);
            try_parse_line(&current_line, start_line_num, &mut result);
            current_line.clear();
        } else {
            try_parse_line(trimmed, line_num, &mut result);
        }
    }

    if !current_line.is_empty() {
        try_parse_line(&current_line, start_line_num, &mut result);
    }

    if in_line_block && !line_block_content.is_empty() {
        let block_line = normalize_line_block_tokens(&line_block_content.trim().replace('\n', " "));
        try_parse_line(&block_line, line_block_start, &mut result);
    }

    result
}

fn extract_markdown_value_blocks(text: &str, values: &mut HashMap<String, String>) -> String {
    if !text.contains("```") {
        return text.to_string();
    }

    let mut result = String::new();
    let mut chars = text.chars().peekable();
    let mut line_start = true;

    while let Some(c) = chars.next() {
        if line_start && c == '`' {
            let mut backtick_count = 1;
            while chars.peek() == Some(&'`') {
                chars.next();
                backtick_count += 1;
            }

            if backtick_count >= 3 {
                let mut lines_consumed = 1;

                while chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
                    chars.next();
                }

                let mut key = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == '\n' || ch == '\r' {
                        break;
                    }
                    if ch.is_whitespace() {
                        break;
                    }
                    key.push(chars.next().unwrap());
                }

                while let Some(&ch) = chars.peek() {
                    if ch == '\n' || ch == '\r' {
                        break;
                    }
                    chars.next();
                }
                if chars.peek() == Some(&'\r') {
                    chars.next();
                }
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }

                let mut content = String::new();
                let closing_pattern: String = "`".repeat(backtick_count);

                loop {
                    let mut line = String::new();
                    let mut at_eol = false;

                    while let Some(&ch) = chars.peek() {
                        if ch == '\n' || ch == '\r' {
                            at_eol = true;
                            break;
                        }
                        line.push(chars.next().unwrap());
                    }

                    let trimmed = line.trim();
                    if trimmed == closing_pattern || trimmed.starts_with(&closing_pattern) {
                        lines_consumed += 1;
                        if chars.peek() == Some(&'\r') {
                            chars.next();
                        }
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        break;
                    }

                    lines_consumed += 1;

                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&line);

                    if at_eol {
                        if chars.peek() == Some(&'\r') {
                            chars.next();
                        }
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                    }

                    if chars.peek().is_none() {
                        break;
                    }
                }

                if !key.is_empty() {
                    values.insert(key, content);
                }

                for _ in 0..lines_consumed {
                    result.push('\n');
                }

                line_start = true;
                continue;
            } else {
                for _ in 0..backtick_count {
                    result.push('`');
                }
            }
        }

        result.push(c);
        line_start = c == '\n';
    }

    result
}

fn create_detailed_parse_error(
    line_num: usize,
    line_content: &str,
    error: &BifrostError,
) -> ParseError {
    let error_msg = error.to_string();
    let trimmed = line_content.trim();
    let leading_spaces = line_content.len() - line_content.trim_start().len();

    if error_msg.contains("No protocol found") {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let second_part = parts[1];
            let second_start = line_content.find(second_part).unwrap_or(0) + 1;
            let second_end = second_start + second_part.len();

            return ParseError::with_range(line_num, second_start, second_end, &error_msg)
                .with_code("E001")
                .with_suggestion(format!(
                    "Add a protocol prefix like 'http://', 'host://', or use 'passthrough://' for direct forwarding. Example: '{} http://localhost:8000/' or '{} passthrough://'",
                    parts[0], parts[0]
                ));
        }
        return ParseError::new(line_num, trimmed, &error_msg)
            .with_code("E001")
            .with_suggestion(
                "Rule format: <pattern> <protocol>://<target>. Example: 'example.com http://localhost:8000/'",
            );
    }

    if error_msg.contains("Unknown protocol") {
        if let Some(proto_match) = PROTOCOL_REGEX.captures(trimmed) {
            let proto = proto_match.get(1).map(|m| m.as_str()).unwrap_or("");
            let proto_start = trimmed.find(&format!("{}://", proto)).unwrap_or(0);
            let start_col = leading_spaces + proto_start + 1;
            let end_col = start_col + proto.len() + 3;

            return ParseError::with_range(line_num, start_col, end_col, &error_msg)
                .with_code("E002")
                .with_suggestion(format!(
                    "Unknown protocol '{}'. Supported protocols: http, https, host, file, redirect, rewrite, reqHeaders, resHeaders, reqBody, resBody, statusCode, passthrough, tlsIntercept, tlsPassthrough, tunnel, ws, wss, etc.",
                    proto
                ));
        }
    }

    if error_msg.contains("No pattern found") {
        return ParseError::new(line_num, trimmed, &error_msg)
            .with_code("E003")
            .with_suggestion(
                "Rule must start with a pattern (domain, URL path, or regex). Example: 'example.com http://localhost:8000/'",
            );
    }

    if error_msg.contains("Invalid regex") {
        if let Some(start) = trimmed.find('/') {
            if let Some(end) = trimmed[start + 1..].find('/') {
                let regex_end = start + end + 2;
                return ParseError::with_range(
                    line_num,
                    leading_spaces + start + 1,
                    leading_spaces + regex_end + 1,
                    &error_msg,
                )
                .with_code("E004")
                .with_suggestion("Check regex syntax. Common issues: unescaped special characters, unbalanced parentheses, invalid quantifiers.");
            }
        }
    }

    ParseError::new(line_num, trimmed, &error_msg)
        .with_code("E000")
        .with_suggestion("Check rule syntax. Format: <pattern> <protocol>://<value>")
}

fn expand_inline_values(line: &str, values: &HashMap<String, String>) -> String {
    let mut result = line.to_string();
    let max_iterations = 10;

    for _ in 0..max_iterations {
        let mut changed = false;
        let current = result.clone();

        for caps in INLINE_VALUES_REGEX.captures_iter(&current) {
            let full_match = caps.get(0).unwrap().as_str();
            let match_start = caps.get(0).unwrap().start();

            if match_start > 0 && current.as_bytes()[match_start - 1] == b'$' {
                continue;
            }

            let key = caps.get(1).unwrap().as_str();

            if let Some(value) = values.get(key) {
                if value.contains('\n') || value.contains('\r') {
                    continue;
                }
                let replacement = if value.contains(' ') || value.contains('\t') {
                    format!("`{}`", value)
                } else {
                    value.clone()
                };
                result = result.replacen(full_match, &replacement, 1);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    result
}

fn split_rule_parts(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_regex = false;
    let mut in_backtick = false;
    let mut brace_depth = 0;
    let mut paren_depth = 0;
    let mut prev_char: Option<char> = None;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '`' if !in_regex => {
                in_backtick = !in_backtick;
                current.push(c);
            }
            '/' if !in_regex
                && !in_backtick
                && current.is_empty()
                && brace_depth == 0
                && paren_depth == 0 =>
            {
                in_regex = true;
                current.push(c);
            }
            '/' if in_regex && prev_char != Some('\\') => {
                current.push(c);
                if let Some(&next) = chars.peek() {
                    if next == 'i' {
                        current.push(chars.next().unwrap());
                    }
                }
                if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
                in_regex = false;
            }
            '{' if !in_regex && !in_backtick => {
                brace_depth += 1;
                current.push(c);
            }
            '}' if !in_regex && !in_backtick && brace_depth > 0 => {
                brace_depth -= 1;
                current.push(c);
            }
            '(' if !in_regex && !in_backtick => {
                paren_depth += 1;
                current.push(c);
            }
            ')' if !in_regex && !in_backtick && paren_depth > 0 => {
                paren_depth -= 1;
                current.push(c);
            }
            ' ' | '\t' if !in_regex && !in_backtick && brace_depth == 0 && paren_depth == 0 => {
                if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
        prev_char = Some(c);
    }

    if !current.is_empty() {
        parts.push(current);
    }

    let mut merged = Vec::new();
    for part in parts {
        let should_merge = merged.last().is_some_and(|previous: &String| {
            previous.contains("://") && !looks_like_rule_part(&part)
        });

        if should_merge {
            if let Some(previous) = merged.last_mut() {
                previous.push(' ');
                previous.push_str(&part);
            }
        } else {
            merged.push(part);
        }
    }

    merged
}

fn looks_like_rule_part(part: &str) -> bool {
    let trimmed = part.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with('/') {
        return true;
    }

    if trimmed.contains("://")
        || trimmed.starts_with('*')
        || trimmed.starts_with('!')
        || trimmed.starts_with('^')
        || trimmed.starts_with('$')
        || HOST_PORT_REGEX.is_match(trimmed)
        || is_bare_host_target_with_path(trimmed)
    {
        return true;
    }

    looks_like_host_pattern(trimmed)
}

fn looks_like_host_pattern(part: &str) -> bool {
    if part.eq_ignore_ascii_case("localhost") {
        return true;
    }

    if part.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }

    DOMAIN_LIKE_PATTERN_REGEX.is_match(part)
}

fn is_target_address(value: &str) -> bool {
    let host_part = if let Some(idx) = value.find('/') {
        &value[..idx]
    } else {
        value
    };
    let host_without_port = if let Some(idx) = host_part.rfind(':') {
        &host_part[..idx]
    } else {
        host_part
    };
    host_without_port == "localhost"
        || host_without_port.starts_with("127.")
        || host_without_port
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.')
        || host_without_port.starts_with('[')
}

fn urls_match_for_passthrough(pattern_url: &str, target_url: &str) -> bool {
    let normalize = |url: &str| -> String {
        let url = url.trim_end_matches('/');
        url.to_lowercase()
    };
    normalize(pattern_url) == normalize(target_url)
}

fn check_passthrough_without_protocol(part: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }

    for pattern in patterns {
        let protocol_prefix = if pattern.starts_with("https://") {
            Some("https://")
        } else if pattern.starts_with("http://") {
            Some("http://")
        } else if pattern.starts_with("wss://") {
            Some("wss://")
        } else if pattern.starts_with("ws://") {
            Some("ws://")
        } else {
            None
        };

        if let Some(prefix) = protocol_prefix {
            let reconstructed_url = format!("{}{}", prefix, part);
            if urls_match_for_passthrough(pattern, &reconstructed_url) {
                return true;
            }
        }
    }
    false
}

fn is_bare_host_target_with_path(part: &str) -> bool {
    if part.contains("://") {
        return false;
    }

    if part.starts_with('!')
        || part.starts_with('*')
        || part.starts_with('$')
        || part.starts_with('^')
        || (part.starts_with('/') && part.ends_with('/'))
        || (part.starts_with('/') && part.ends_with("/i"))
    {
        return false;
    }

    BARE_HOST_PATH_TARGET_REGEX.is_match(part)
}

fn extract_pattern_and_protocols(parts: &[String]) -> Result<ParsedPatternResult> {
    if parts.is_empty() {
        return Err(BifrostError::Parse("Empty rule".to_string()));
    }

    let mut patterns = Vec::new();
    let mut protocol_values = Vec::new();
    let mut include_filters = Vec::new();
    let mut exclude_filters = Vec::new();
    let mut line_props = LineProps::default();

    for part in parts.iter() {
        if let Some(stripped) = part.strip_prefix(INCLUDE_FILTER_PREFIX) {
            let filter_value = strip_backticks(stripped);
            if let Some(filter) = parse_filter(&filter_value) {
                include_filters.push(filter);
            }
            continue;
        }

        if let Some(stripped) = part.strip_prefix(EXCLUDE_FILTER_PREFIX) {
            let filter_value = strip_backticks(stripped);
            if let Some(filter) = parse_filter(&filter_value) {
                exclude_filters.push(filter);
            }
            continue;
        }

        if let Some(stripped) = part.strip_prefix(LINE_PROPS_PREFIX) {
            let props_value = strip_backticks(stripped);
            line_props = parse_line_props(&props_value);
            continue;
        }

        if let Some(caps) = PROTOCOL_REGEX.captures(part) {
            let proto_name = caps.get(1).unwrap().as_str();
            let raw_value = caps.get(2).unwrap().as_str();
            let value = if raw_value.starts_with('`') && raw_value.ends_with('`') {
                raw_value[1..raw_value.len() - 1].to_string()
            } else {
                raw_value.to_string()
            };

            if let Some(protocol) = Protocol::parse(proto_name) {
                if (protocol == Protocol::Http
                    || protocol == Protocol::Https
                    || protocol == Protocol::Ws
                    || protocol == Protocol::Wss
                    || protocol == Protocol::Tunnel)
                    && !is_target_address(&value)
                {
                    let reconstructed_url = format!("{}://{}", proto_name.to_lowercase(), value);
                    let is_same_as_pattern = patterns.iter().any(|p: &String| {
                        let pattern_url = if p.starts_with("http://")
                            || p.starts_with("https://")
                            || p.starts_with("ws://")
                            || p.starts_with("wss://")
                        {
                            p.clone()
                        } else {
                            format!("{}://{}", proto_name.to_lowercase(), p)
                        };
                        urls_match_for_passthrough(&pattern_url, &reconstructed_url)
                    });
                    if is_same_as_pattern {
                        protocol_values.push((Protocol::Passthrough, String::new()));
                    } else if !patterns.is_empty() {
                        protocol_values.push((protocol, value));
                    } else {
                        patterns.push(part.clone());
                    }
                } else if (protocol == Protocol::Http
                    || protocol == Protocol::Https
                    || protocol == Protocol::Ws
                    || protocol == Protocol::Wss
                    || protocol == Protocol::Tunnel)
                    && patterns.is_empty()
                {
                    patterns.push(part.clone());
                } else {
                    protocol_values.push((protocol, value));
                }
            } else {
                let resolved = Protocol::resolve_alias(proto_name);
                if let Some(protocol) = Protocol::parse(resolved) {
                    protocol_values.push((protocol, value));
                } else {
                    patterns.push(part.clone());
                }
            }
        } else if HOST_PORT_REGEX.is_match(part) {
            if patterns.is_empty() {
                patterns.push(part.clone());
            } else {
                protocol_values.push((Protocol::Host, part.clone()));
            }
        } else {
            let is_passthrough = check_passthrough_without_protocol(part, &patterns);
            if is_passthrough {
                protocol_values.push((Protocol::Passthrough, String::new()));
            } else if !patterns.is_empty()
                && protocol_values.is_empty()
                && is_bare_host_target_with_path(part)
            {
                protocol_values.push((Protocol::Host, part.clone()));
            } else {
                patterns.push(part.clone());
            }
        }
    }

    if patterns.is_empty() {
        return Err(BifrostError::Parse("No pattern found in rule".to_string()));
    }

    Ok((
        patterns,
        protocol_values,
        include_filters,
        exclude_filters,
        line_props,
    ))
}

fn strip_backticks(s: &str) -> String {
    if s.starts_with('`') && s.ends_with('`') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_rule() {
        let rules = parse_line("example.com host://127.0.0.1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "127.0.0.1");
    }

    #[test]
    fn test_parse_multi_protocol_rule() {
        let rules = parse_line("example.com host://127.0.0.1 reqHeaders://{test=1}").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[1].protocol, Protocol::ReqHeaders);
    }

    #[test]
    fn test_parse_comment_line() {
        let rules = parse_line("# this is a comment").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_parse_empty_line() {
        let rules = parse_line("").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_parse_whitespace_line() {
        let rules = parse_line("   \t  ").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_parse_wildcard_pattern() {
        let rules = parse_line("*.example.com host://127.0.0.1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "*.example.com");
    }

    #[test]
    fn test_parse_regex_pattern() {
        let rules = parse_line("/example\\.com/ host://127.0.0.1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "/example\\.com/");
    }

    #[test]
    fn test_parse_regex_pattern_case_insensitive() {
        let rules = parse_line("/example\\.com/i host://127.0.0.1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "/example\\.com/i");
    }

    #[test]
    fn test_parse_rules_multiline() {
        let text = r#"
# Comment line
example.com host://127.0.0.1
*.api.com proxy://proxy.local:8080

192.168.1.1 passthrough://
"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 3);
    }

    #[test]
    fn test_parse_rules_continuation() {
        let text = r#"example.com \
host://127.0.0.1 \
reqHeaders://{test=1}"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].line, Some(1));
        assert_eq!(rules[1].line, Some(1));
    }

    #[test]
    fn test_parse_with_inline_values() {
        let mut values = HashMap::new();
        values.insert("config.json".to_string(), "value_from_json".to_string());

        let parser = RuleParser::with_values(values);
        let result = parser.parse_line("example.com host://{config.json}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_split_rule_parts() {
        let parts = split_rule_parts("example.com host://127.0.0.1");
        assert_eq!(parts, vec!["example.com", "host://127.0.0.1"]);
    }

    #[test]
    fn test_split_rule_parts_with_regex() {
        let parts = split_rule_parts("/test\\.com/ host://127.0.0.1");
        assert_eq!(parts, vec!["/test\\.com/", "host://127.0.0.1"]);
    }

    #[test]
    fn test_split_rule_parts_with_regex_case_insensitive() {
        let parts = split_rule_parts("/test\\.com/i host://127.0.0.1");
        assert_eq!(parts, vec!["/test\\.com/i", "host://127.0.0.1"]);
    }

    #[test]
    fn test_split_rule_parts_with_regex_escaped_slash() {
        let parts = split_rule_parts(r"/example\.com\/api\/v[0-9]+/ http://localhost:8000/");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], r"/example\.com\/api\/v[0-9]+/");
        assert_eq!(parts[1], "http://localhost:8000/");
    }

    #[test]
    fn test_split_rule_parts_with_regex_multiple_escaped_slashes() {
        let parts = split_rule_parts(r"/a\/b\/c\/d/ host://127.0.0.1");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], r"/a\/b\/c\/d/");
        assert_eq!(parts[1], "host://127.0.0.1");
    }

    #[test]
    fn test_protocol_alias_resolution() {
        let rules = parse_line("example.com hosts://127.0.0.1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].protocol, Protocol::Host);
    }

    #[test]
    fn test_parse_ip_pattern() {
        let rules = parse_line("192.168.1.1 passthrough://").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "192.168.1.1");
    }

    #[test]
    fn test_parse_cidr_pattern() {
        let rules = parse_line("192.168.0.0/16 proxy://proxy.local:8080").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "192.168.0.0/16");
    }

    #[test]
    fn test_parse_invalid_rule_no_protocol() {
        let result = parse_line("example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_rule_parser_set_value() {
        let mut parser = RuleParser::new();
        parser.set_value("key".to_string(), "value".to_string());
        assert_eq!(parser.values.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_expand_inline_values() {
        let mut values = HashMap::new();
        values.insert("data.json".to_string(), "expanded_value".to_string());

        let result = expand_inline_values("test {data.json} end", &values);
        assert_eq!(result, "test expanded_value end");
    }

    #[test]
    fn test_expand_inline_values_no_match() {
        let values = HashMap::new();
        let result = expand_inline_values("test {unknown.json} end", &values);
        assert_eq!(result, "test {unknown.json} end");
    }

    #[test]
    fn test_parse_negated_pattern() {
        let rules = parse_line("!*.example.com passthrough://").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].is_negated());
    }

    #[test]
    fn test_parse_multiple_protocols() {
        let rules =
            parse_line("example.com host://127.0.0.1 proxy://proxy:8080 reqDelay://1000").unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[1].protocol, Protocol::Proxy);
        assert_eq!(rules[2].protocol, Protocol::ReqDelay);
    }

    #[test]
    fn test_parse_rules_with_line_numbers() {
        let text = "line1.com host://1\nline2.com host://2\nline3.com host://3";
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules[0].line, Some(1));
        assert_eq!(rules[1].line, Some(2));
        assert_eq!(rules[2].line, Some(3));
    }

    #[test]
    fn test_rule_parser_default() {
        let parser = RuleParser::default();
        assert!(parser.values.is_empty());
    }

    #[test]
    fn test_expand_inline_values_varname() {
        let mut values = HashMap::new();
        values.insert("myResponse".to_string(), r#"{"ok":true}"#.to_string());

        let result = expand_inline_values("test {myResponse} end", &values);
        assert_eq!(result, r#"test {"ok":true} end"#);
    }

    #[test]
    fn test_expand_inline_values_nested() {
        let mut values = HashMap::new();
        values.insert("inner".to_string(), "resolved_inner".to_string());
        values.insert("outer".to_string(), "prefix_{inner}_suffix".to_string());

        let result = expand_inline_values("{outer}", &values);
        assert_eq!(result, "prefix_resolved_inner_suffix");
    }

    #[test]
    fn test_expand_inline_values_preserve_template_vars() {
        let mut values = HashMap::new();
        values.insert(
            "response".to_string(),
            r#"{"url":"${url}","time":${now}}"#.to_string(),
        );

        let result = expand_inline_values("{response}", &values);
        assert_eq!(result, r#"{"url":"${url}","time":${now}}"#);
    }

    #[test]
    fn test_expand_inline_values_skip_template_syntax() {
        let values = HashMap::new();
        let result = expand_inline_values("${host} and {varName}", &values);
        assert_eq!(result, "${host} and {varName}");
    }

    #[test]
    fn test_expand_inline_values_multiple_same_var() {
        let mut values = HashMap::new();
        values.insert("val".to_string(), "X".to_string());

        let result = expand_inline_values("{val}-{val}-{val}", &values);
        assert_eq!(result, "X-X-X");
    }

    #[test]
    fn test_expand_inline_values_deep_nested() {
        let mut values = HashMap::new();
        values.insert("a".to_string(), "{b}".to_string());
        values.insert("b".to_string(), "{c}".to_string());
        values.insert("c".to_string(), "final".to_string());

        let result = expand_inline_values("{a}", &values);
        assert_eq!(result, "final");
    }

    #[test]
    fn test_parse_with_varname_values() {
        let mut values = HashMap::new();
        values.insert("myHost".to_string(), "127.0.0.1:8080".to_string());

        let parser = RuleParser::with_values(values);
        let rules = parser.parse_line("example.com host://{myHost}").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value, "127.0.0.1:8080");
    }

    #[test]
    fn test_parse_with_nested_template() {
        let mut values = HashMap::new();
        values.insert(
            "mockBody".to_string(),
            r#"{"host":"${hostname}","path":"${path}"}"#.to_string(),
        );

        let parser = RuleParser::with_values(values);
        let rules = parser
            .parse_line("example.com resBody://{mockBody}")
            .unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value, r#"{"host":"${hostname}","path":"${path}"}"#);
    }

    #[test]
    fn test_parse_bare_host_port() {
        let rules = parse_line("example.com 127.0.0.1:3000").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "127.0.0.1:3000");
    }

    #[test]
    fn test_parse_bare_host_port_with_path() {
        let rules = parse_line("example.com 127.0.0.1:3000/api").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "127.0.0.1:3000/api");
    }

    #[test]
    fn test_parse_bare_host_port_with_deep_path() {
        let rules = parse_line("example.com localhost:8080/api/v1/users").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "localhost:8080/api/v1/users");
    }

    #[test]
    fn test_parse_bare_host_port_with_other_protocols() {
        let rules = parse_line("example.com 127.0.0.1:3000/api reqHeaders://{x-test=1}").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "127.0.0.1:3000/api");
        assert_eq!(rules[1].protocol, Protocol::ReqHeaders);
    }

    #[test]
    fn test_parse_bare_domain_with_path_as_host_target() {
        let rules =
            parse_line("gamingpop-boe.byteintl.net/manager gamingpop-boe.byteintl.net/manager")
                .unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "gamingpop-boe.byteintl.net/manager");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "gamingpop-boe.byteintl.net/manager");
    }

    #[test]
    fn test_parse_bare_domain_with_path_and_query_as_host_target() {
        let rules = parse_line("example.com/api target.example.com/api/v1?debug=1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com/api");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "target.example.com/api/v1?debug=1");
    }

    #[test]
    fn test_parse_protocol_first_multiple_patterns_still_supported() {
        let rules = parse_line("proxy://127.0.0.1:8080 www.example.com api.example.com").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].pattern, "www.example.com");
        assert_eq!(rules[0].protocol, Protocol::Proxy);
        assert_eq!(rules[1].pattern, "api.example.com");
        assert_eq!(rules[1].protocol, Protocol::Proxy);
    }

    #[test]
    fn test_host_port_regex() {
        assert!(HOST_PORT_REGEX.is_match("127.0.0.1:3000"));
        assert!(HOST_PORT_REGEX.is_match("127.0.0.1:3000/api"));
        assert!(HOST_PORT_REGEX.is_match("localhost:8080"));
        assert!(HOST_PORT_REGEX.is_match("localhost:8080/path/to/api"));
        assert!(HOST_PORT_REGEX.is_match("[::1]:8080"));
        assert!(HOST_PORT_REGEX.is_match("[::1]:8080/api"));
        assert!(!HOST_PORT_REGEX.is_match("example.com"));
        assert!(!HOST_PORT_REGEX.is_match("host://127.0.0.1:3000"));
        assert!(!HOST_PORT_REGEX.is_match("*.example.com"));
        assert!(
            !HOST_PORT_REGEX.is_match("my-server.local:3000"),
            "Domain names with port should be treated as patterns, not host targets"
        );
        assert!(
            !HOST_PORT_REGEX.is_match("portmatch.local:8080"),
            "Domain names with port should be treated as patterns, not host targets"
        );
    }

    #[test]
    fn test_parse_domain_with_port_as_pattern() {
        let rules = parse_line("portmatch.local:8080 http://127.0.0.1:3000").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "portmatch.local:8080");
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[0].value, "127.0.0.1:3000");
    }

    #[test]
    fn test_parse_full_url_as_pattern() {
        let rules =
            parse_line("https://full-match.local:443/api/v1 http://127.0.0.1:3000").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://full-match.local:443/api/v1");
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[0].value, "127.0.0.1:3000");
    }

    #[test]
    fn test_parse_include_filter() {
        let rules = parse_line("example.com host://127.0.0.1 includeFilter://m:GET").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].include_filters.len(), 1);
        assert!(rules[0].exclude_filters.is_empty());
    }

    #[test]
    fn test_parse_exclude_filter() {
        let rules = parse_line("example.com host://127.0.0.1 excludeFilter:///admin/").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].include_filters.is_empty());
        assert_eq!(rules[0].exclude_filters.len(), 1);
    }

    #[test]
    fn test_parse_multiple_filters() {
        let rules = parse_line(
            "example.com host://127.0.0.1 includeFilter://m:GET includeFilter:///api/ excludeFilter:///admin/",
        )
        .unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].include_filters.len(), 2);
        assert_eq!(rules[0].exclude_filters.len(), 1);
    }

    #[test]
    fn test_parse_line_props_important() {
        let rules = parse_line("example.com host://127.0.0.1 lineProps://important").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].line_props.important);
        assert!(!rules[0].line_props.disabled);
    }

    #[test]
    fn test_parse_line_props_disabled() {
        let rules = parse_line("example.com host://127.0.0.1 lineProps://disabled").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(!rules[0].line_props.important);
        assert!(rules[0].line_props.disabled);
    }

    #[test]
    fn test_parse_line_props_multiple() {
        let rules =
            parse_line("example.com host://127.0.0.1 lineProps://important,disabled").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].line_props.important);
        assert!(rules[0].line_props.disabled);
    }

    #[test]
    fn test_important_priority() {
        let rules1 = parse_line("example.com host://127.0.0.1").unwrap();
        let rules2 = parse_line("example.com host://127.0.0.1 lineProps://important").unwrap();

        assert!(rules2[0].priority() > rules1[0].priority());
        assert!(rules2[0].priority() >= 10000);
    }

    #[test]
    fn test_parse_filters_with_backticks() {
        let rules =
            parse_line("example.com host://127.0.0.1 includeFilter://`m:GET,POST`").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].include_filters.len(), 1);
    }

    #[test]
    fn test_parse_line_block_basic() {
        let text = r#"line`
host://127.0.0.1
example.com
`"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].protocol, Protocol::Host);
    }

    #[test]
    fn test_parse_line_block_with_filters() {
        let text = r#"line`
host://127.0.0.1
example.com
includeFilter://m:GET
excludeFilter:///admin/
`"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].include_filters.len(), 1);
        assert_eq!(rules[0].exclude_filters.len(), 1);
    }

    #[test]
    fn test_parse_line_block_with_line_props() {
        let text = r#"line`
host://127.0.0.1
example.com
lineProps://important
`"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].line_props.important);
    }

    #[test]
    fn test_parse_combined_filters_and_props() {
        let rules = parse_line(
            "example.com host://127.0.0.1 includeFilter://m:GET excludeFilter:///admin/ lineProps://important",
        )
        .unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].include_filters.len(), 1);
        assert_eq!(rules[0].exclude_filters.len(), 1);
        assert!(rules[0].line_props.important);
    }

    #[test]
    fn test_is_disabled() {
        let rules1 = parse_line("example.com host://127.0.0.1").unwrap();
        let rules2 = parse_line("example.com host://127.0.0.1 lineProps://disabled").unwrap();

        assert!(!rules1[0].is_disabled());
        assert!(rules2[0].is_disabled());
    }

    #[test]
    fn test_extract_markdown_value_blocks() {
        let mut values = HashMap::new();
        let text = r#"
# Comment
example.com resBody://{my_response}

``` my_response
{"status":"ok"}
```
"#;
        let result = extract_markdown_value_blocks(text, &mut values);
        assert_eq!(
            values.get("my_response"),
            Some(&r#"{"status":"ok"}"#.to_string())
        );
        assert!(!result.contains("```"));
    }

    #[test]
    fn test_extract_markdown_value_blocks_multiple() {
        let mut values = HashMap::new();
        let text = r#"
example.com resBody://{response1}
another.com resBody://{response2}

``` response1
content1
```

``` response2
content2
```
"#;
        let result = extract_markdown_value_blocks(text, &mut values);
        assert_eq!(values.get("response1"), Some(&"content1".to_string()));
        assert_eq!(values.get("response2"), Some(&"content2".to_string()));
        assert!(!result.contains("```"));
    }

    #[test]
    fn test_extract_markdown_value_blocks_multiline() {
        let mut values = HashMap::new();
        let text = r#"
example.com resBody://{json_data}

``` json_data
{
  "name": "test",
  "value": 123
}
```
"#;
        let result = extract_markdown_value_blocks(text, &mut values);
        let expected = r#"{
  "name": "test",
  "value": 123
}"#;
        assert_eq!(values.get("json_data"), Some(&expected.to_string()));
        assert!(!result.contains("```"));
    }

    #[test]
    fn test_parse_rules_with_markdown_value_blocks() {
        let text = r#"
example.com resBody://{custom_response}

``` custom_response
{"custom":"response"}
```
"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "example.com");
        assert_eq!(rules[0].value, r#"{"custom":"response"}"#);
    }

    #[test]
    fn test_parse_rules_with_multiple_markdown_blocks() {
        let text = r#"
test1.local resBody://{body1}
test2.local resBody://{body2}

``` body1
first content
```

``` body2
second content
```
"#;
        let rules = parse_rules(text).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].value, "first content");
        assert_eq!(rules[1].value, "second content");
    }

    #[test]
    fn test_extract_markdown_blocks_overwrite_existing() {
        let mut values = HashMap::new();
        values.insert("existing".to_string(), "original".to_string());

        let text = r#"
``` existing
new_value
```
"#;
        extract_markdown_value_blocks(text, &mut values);
        assert_eq!(values.get("existing"), Some(&"new_value".to_string()));
    }

    #[test]
    fn test_parse_paren_content_with_space() {
        let rules = parse_line("example.com reqHeaders://(X-Custom: value)").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].protocol, Protocol::ReqHeaders);
        assert_eq!(rules[0].value, "(X-Custom: value)");
    }

    #[test]
    fn test_parse_multiple_protocols_with_paren_content() {
        let rules =
            parse_line("*.api.test tlsIntercept:// reqHeaders://(X-Intercepted: true)").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].protocol, Protocol::TlsIntercept);
        assert_eq!(rules[1].protocol, Protocol::ReqHeaders);
        assert_eq!(rules[1].value, "(X-Intercepted: true)");
    }

    #[test]
    fn test_split_rule_parts_preserves_paren_content() {
        let parts = split_rule_parts("example.com reqHeaders://(X-Header: with space)");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "example.com");
        assert_eq!(parts[1], "reqHeaders://(X-Header: with space)");
    }

    #[test]
    fn test_split_rule_parts_multiple_protocols_with_paren() {
        let parts = split_rule_parts("*.test tlsIntercept:// reqHeaders://(Auth: Bearer token)");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "*.test");
        assert_eq!(parts[1], "tlsIntercept://");
        assert_eq!(parts[2], "reqHeaders://(Auth: Bearer token)");
    }

    #[test]
    fn test_split_rule_parts_protocol_then_pattern() {
        let parts = split_rule_parts("host://127.0.0.1 example.com");
        assert_eq!(parts, vec!["host://127.0.0.1", "example.com"]);
    }

    #[test]
    fn test_split_rule_parts_protocol_then_multiple_patterns() {
        let parts = split_rule_parts("proxy://127.0.0.1:8080 www.example.com api.example.com");
        assert_eq!(
            parts,
            vec![
                "proxy://127.0.0.1:8080",
                "www.example.com",
                "api.example.com"
            ]
        );
    }

    #[test]
    fn test_parse_passthrough_same_url() {
        let rules = parse_line("https://example.com/api/ https://example.com/api/").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com/api/");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
        assert_eq!(rules[0].value, "");
    }

    #[test]
    fn test_parse_passthrough_same_url_without_trailing_slash() {
        let rules = parse_line("https://example.com/api https://example.com/api").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com/api");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
        assert_eq!(rules[0].value, "");
    }

    #[test]
    fn test_parse_passthrough_with_mismatched_trailing_slash() {
        let rules = parse_line("https://example.com/api/ https://example.com/api").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com/api/");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
        assert_eq!(rules[0].value, "");
    }

    #[test]
    fn test_parse_passthrough_in_multi_rule_file() {
        let content = r#"
# BOE
https://example.com/api/ https://example.com/api/
https://example.com http://localhost:8000/
wss://example.com/ ws://localhost:8000/
"#;
        let rules = parse_rules(content).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
        assert_eq!(rules[1].protocol, Protocol::Http);
        assert_eq!(rules[2].protocol, Protocol::Ws);
    }

    #[test]
    fn test_parse_different_url_not_passthrough() {
        let rules = parse_line("https://example.com/api/ http://localhost:8000/").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_ne!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_passthrough_without_protocol_prefix() {
        let rules = parse_line("https://example.com/api/ example.com/api/").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com/api/");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
        assert_eq!(rules[0].value, "");
    }

    #[test]
    fn test_parse_passthrough_without_protocol_prefix_no_trailing_slash() {
        let rules = parse_line("https://example.com/api example.com/api").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com/api");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_passthrough_without_protocol_domain_only() {
        let rules = parse_line("https://example.com example.com").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_passthrough_ws_without_protocol() {
        let rules = parse_line("wss://example.com/ws example.com/ws").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "wss://example.com/ws");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_not_passthrough_different_path() {
        let rules = parse_line("https://example.com/api/ example.com/other/").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://example.com/api/");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "example.com/other/");
    }

    #[test]
    fn test_parse_passthrough_with_valid_redirect() {
        let rules =
            parse_line("https://example.com/api/ example.com/api/ http://localhost:8000/").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
        assert_eq!(rules[1].protocol, Protocol::Http);
    }

    #[test]
    fn test_parse_rules_tolerant_with_errors() {
        let content = r#"
# Valid rules
example.com http://localhost:8000/
test.local host://127.0.0.1:3000

# Invalid rule - no protocol
invalid.com another.com

# Another valid rule
valid.com http://localhost:9000/
"#;
        let result = parse_rules_tolerant(content);
        assert_eq!(result.rules.len(), 3);
        assert_eq!(result.errors.len(), 1);

        let error = &result.errors[0];
        assert!(error.message.contains("No protocol"));
        assert_eq!(error.line, 7);
        assert!(error.start_column > 0);
        assert!(error.end_column > error.start_column);
        assert!(error.suggestion.is_some());
        assert!(error.code.is_some());
        assert_eq!(error.code.as_ref().unwrap(), "E001");
    }

    #[test]
    fn test_parse_rules_tolerant_all_valid() {
        let content = r#"
example.com http://localhost:8000/
test.local host://127.0.0.1:3000
"#;
        let result = parse_rules_tolerant(content);
        assert_eq!(result.rules.len(), 2);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_parse_rules_tolerant_with_passthrough() {
        let content = r#"
# Passthrough rule
https://example.com/api/ https://example.com/api/
# Redirect rule
https://example.com http://localhost:8000/
"#;
        let result = parse_rules_tolerant(content);
        assert_eq!(result.rules.len(), 2);
        assert!(result.errors.is_empty());
        assert_eq!(result.rules[0].protocol, Protocol::Passthrough);
        assert_eq!(result.rules[1].protocol, Protocol::Http);
    }

    #[test]
    fn test_validate_rules_returns_errors() {
        let content = r#"
valid.com http://localhost:8000/
invalid.com another.com
"#;
        let errors = validate_rules(content);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 3);
    }

    #[test]
    fn test_validate_rules_no_errors() {
        let content = r#"
valid.com http://localhost:8000/
test.local host://127.0.0.1:3000
"#;
        let errors = validate_rules(content);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_parse_error_detailed_info() {
        let content = "  invalid.com another.com";
        let result = parse_rules_tolerant(content);
        assert_eq!(result.errors.len(), 1);

        let error = &result.errors[0];
        assert_eq!(error.line, 1);
        assert!(error.start_column > 2);
        assert!(error.suggestion.is_some());
        let suggestion = error.suggestion.as_ref().unwrap();
        assert!(suggestion.contains("http://") || suggestion.contains("passthrough://"));
    }

    #[test]
    fn test_parse_error_severity() {
        let content = "invalid.com another.com";
        let result = parse_rules_tolerant(content);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].severity, ParseErrorSeverity::Error);
    }

    #[test]
    fn test_parse_error_code() {
        let content = "example.com unknownproto://value";
        let result = parse_rules_tolerant(content);
        if !result.errors.is_empty() {
            let error = &result.errors[0];
            assert!(error.code.is_some());
        }
    }

    #[test]
    fn test_valid_rules_file() {
        let content = include_str!("../valid_rules.bifrost");

        println!("\n========== 验证有效规则文件 ==========");
        println!("文件内容行数: {}", content.lines().count());

        let result = parse_rules_tolerant(content);

        println!("\n--- 解析结果 ---");
        println!("成功解析规则数: {}", result.rules.len());
        println!("错误数量: {}", result.errors.len());

        if !result.errors.is_empty() {
            println!("\n--- 错误详情 ---");
            for error in &result.errors {
                println!(
                    "  Line {}, Col {}-{} [{}] ({}): {}",
                    error.line,
                    error.start_column,
                    error.end_column,
                    format!("{:?}", error.severity).to_lowercase(),
                    error.code.as_deref().unwrap_or("N/A"),
                    error.message
                );
                if let Some(ref suggestion) = error.suggestion {
                    println!("    建议: {}", suggestion);
                }
            }
        }

        println!("\n--- 已解析的规则协议统计 ---");
        let mut protocol_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for rule in &result.rules {
            *protocol_counts
                .entry(rule.protocol.to_str().to_string())
                .or_insert(0) += 1;
        }
        let mut sorted: Vec<_> = protocol_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (protocol, count) in sorted {
            println!("  {}: {}", protocol, count);
        }

        assert!(
            result.errors.is_empty(),
            "有效规则文件不应该有解析错误，但发现 {} 个错误",
            result.errors.len()
        );
        assert!(
            result.rules.len() > 50,
            "有效规则文件应该包含大量规则，但只解析到 {} 条",
            result.rules.len()
        );

        println!("\n✓ 有效规则文件验证通过！共 {} 条规则", result.rules.len());
    }

    #[test]
    fn test_invalid_rules_file() {
        let content = include_str!("../invalid_rules.bifrost");

        println!("\n========== 验证无效规则文件 ==========");
        println!("文件内容行数: {}", content.lines().count());

        let result = parse_rules_tolerant(content);

        println!("\n--- 解析结果 ---");
        println!("成功解析规则数: {}", result.rules.len());
        println!("错误数量: {}", result.errors.len());

        println!("\n--- 错误详情 (Monaco 编辑器格式) ---");
        for error in &result.errors {
            println!(
                "  Line {}, Col {}-{} [{}] ({}): {}",
                error.line,
                error.start_column,
                error.end_column,
                format!("{:?}", error.severity).to_lowercase(),
                error.code.as_deref().unwrap_or("N/A"),
                error.message
            );
            if let Some(ref suggestion) = error.suggestion {
                println!("    建议: {}", suggestion);
            }
            println!();
        }

        println!("--- 容错解析验证 ---");
        println!("在错误规则之间成功解析的有效规则:");
        for rule in &result.rules {
            println!(
                "  ✓ {} -> {}://{}",
                rule.pattern,
                rule.protocol.to_str(),
                rule.value
            );
        }

        assert!(!result.errors.is_empty(), "无效规则文件应该包含解析错误");
        assert!(
            result.rules.len() >= 7,
            "容错解析应该成功解析有效规则，但只解析到 {} 条",
            result.rules.len()
        );

        println!(
            "\n✓ 无效规则文件验证通过！检测到 {} 个错误，容错解析了 {} 条有效规则",
            result.errors.len(),
            result.rules.len()
        );
    }

    #[test]
    fn test_validate_unclosed_code_block() {
        let content = r#"
valid.com http://localhost:8000/

``` jsonResponse
{"code": 0}
"#;
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试未闭合代码块检测 ===");
        println!("错误数量: {}", result.errors.len());
        for error in &result.errors {
            println!(
                "  Line {}: [{}] {}",
                error.line,
                error.code.as_deref().unwrap_or("N/A"),
                error.message
            );
        }

        assert!(!result.valid, "未闭合代码块应该被检测为无效");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == Some("E005".to_string())),
            "应该有 E005 未闭合代码块错误"
        );
    }

    #[test]
    fn test_validate_undefined_variable() {
        let content = r#"
valid.com http://localhost:8000/
ref.test resBody://{undefinedVar}
"#;
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试未定义变量检测 ===");
        println!("警告数量: {}", result.warnings.len());
        for warning in &result.warnings {
            println!(
                "  Line {}, Col {}-{}: [{}] {}",
                warning.line,
                warning.start_column,
                warning.end_column,
                warning.code.as_deref().unwrap_or("N/A"),
                warning.message
            );
            if let Some(ref suggestion) = warning.suggestion {
                println!("    建议: {}", suggestion);
            }
        }

        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == Some("W001".to_string())),
            "应该有 W001 未定义变量警告"
        );
    }

    #[test]
    fn test_validate_defined_variable() {
        let content = r#"
ref.test resBody://{myResponse}

``` myResponse
{"code": 0, "data": null}
```
"#;
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试已定义变量 ===");
        println!("规则数量: {}", result.rule_count);
        println!("错误数量: {}", result.errors.len());
        println!("警告数量: {}", result.warnings.len());
        println!("定义的变量: {:?}", result.defined_variables);

        assert!(result.valid, "使用已定义变量应该是有效的");
        assert!(
            result
                .defined_variables
                .iter()
                .any(|v| v.name == "myResponse"),
            "应该检测到 myResponse 变量定义"
        );
    }

    #[test]
    fn test_validate_with_global_values() {
        let content = r#"
ref.test resBody://{globalVar}
"#;
        let mut global_values = HashMap::new();
        global_values.insert("globalVar".to_string(), "global content".to_string());

        let result = validate_rules_with_context(content, &global_values);

        println!("\n=== 测试全局变量引用 ===");
        println!("规则数量: {}", result.rule_count);
        println!("警告数量: {}", result.warnings.len());
        println!("定义的变量: {:?}", result.defined_variables);

        let global_var = result
            .defined_variables
            .iter()
            .find(|v| v.name == "globalVar");
        assert!(global_var.is_some(), "应该包含全局变量");
        assert_eq!(
            global_var.unwrap().source,
            "global",
            "变量来源应该是 global"
        );
    }

    #[test]
    fn test_validate_skip_template_variables() {
        let content = "tpl.test resHeaders://`(X-Time: ${now})`\n";
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试模板变量不被误报 ===");
        println!("警告数量: {}", result.warnings.len());

        assert!(
            !result.warnings.iter().any(|w| w.message.contains("now")),
            "模板变量 ${{now}} 不应该被当作未定义变量"
        );
    }

    #[test]
    fn test_validate_parentheses_with_spaces() {
        let content = r#"
space.test resBody://(hello world)
space2.test resHeaders://(X-Key: value with spaces)
"#;
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试小括号内空格检测 ===");
        println!("警告数量: {}", result.warnings.len());
        for warning in &result.warnings {
            println!(
                "  Line {}: [{}] {}",
                warning.line,
                warning.code.as_deref().unwrap_or("N/A"),
                warning.message
            );
            if let Some(ref suggestion) = warning.suggestion {
                println!("    建议: {}", suggestion);
            }
        }

        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == Some("W002".to_string())),
            "应该有 W002 小括号内空格警告"
        );
    }

    #[test]
    fn test_validate_parentheses_without_spaces() {
        let content = r#"
valid.test resBody://({"code":0})
valid2.test resHeaders://(X-Key:value)
"#;
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试无空格的小括号内容 ===");
        println!("警告数量: {}", result.warnings.len());

        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.code == Some("W002".to_string())),
            "无空格的小括号内容不应该有 W002 警告"
        );
    }

    #[test]
    fn test_validate_script_references() {
        let content = r#"
api.test reqScript://myRequestHandler
api2.test resScript://myResponseHandler
combined.test reqScript://reqHandler resScript://resHandler
"#;
        let result = validate_rules_with_context(content, &HashMap::new());

        println!("\n=== 测试脚本引用提取 ===");
        println!("规则数量: {}", result.rule_count);
        println!("脚本引用数量: {}", result.script_references.len());
        for script_ref in &result.script_references {
            println!(
                "  Line {}: {} script '{}'",
                script_ref.line, script_ref.script_type, script_ref.name
            );
        }

        assert_eq!(result.script_references.len(), 4, "应该检测到 4 个脚本引用");

        let req_scripts: Vec<_> = result
            .script_references
            .iter()
            .filter(|s| s.script_type == "request")
            .collect();
        let res_scripts: Vec<_> = result
            .script_references
            .iter()
            .filter(|s| s.script_type == "response")
            .collect();

        assert_eq!(req_scripts.len(), 2, "应该有 2 个请求脚本引用");
        assert_eq!(res_scripts.len(), 2, "应该有 2 个响应脚本引用");
    }

    #[test]
    fn test_parse_wss_to_wss_forward() {
        let rules = parse_line("wss://a.com wss://echo.websocket.org").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "wss://a.com");
        assert_eq!(rules[0].protocol, Protocol::Wss);
        assert_eq!(rules[0].value, "echo.websocket.org");
    }

    #[test]
    fn test_parse_ws_to_ws_forward() {
        let rules = parse_line("ws://a.com ws://echo.websocket.org").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "ws://a.com");
        assert_eq!(rules[0].protocol, Protocol::Ws);
        assert_eq!(rules[0].value, "echo.websocket.org");
    }

    #[test]
    fn test_parse_wss_to_different_wss_with_path() {
        let rules = parse_line("wss://a.com/path wss://b.com/other").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "wss://a.com/path");
        assert_eq!(rules[0].protocol, Protocol::Wss);
        assert_eq!(rules[0].value, "b.com/other");
    }

    #[test]
    fn test_parse_http_to_http_forward() {
        let rules = parse_line("http://a.com http://b.com").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://a.com");
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[0].value, "b.com");
    }

    #[test]
    fn test_parse_https_to_https_forward() {
        let rules = parse_line("https://a.com https://b.com").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://a.com");
        assert_eq!(rules[0].protocol, Protocol::Https);
        assert_eq!(rules[0].value, "b.com");
    }

    #[test]
    fn test_parse_single_http_localhost_url_as_pattern() {
        let rules = parse_line("http://127.0.0.1:8889").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://127.0.0.1:8889");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_single_http_ip_url_as_pattern() {
        let rules = parse_line("http://192.168.1.1:3000").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://192.168.1.1:3000");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_single_https_localhost_url_as_pattern() {
        let rules = parse_line("https://127.0.0.1:443").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://127.0.0.1:443");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_single_http_localhost_name_as_pattern() {
        let rules = parse_line("http://localhost:8080").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://localhost:8080");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_single_bare_ip_port_as_pattern() {
        let rules = parse_line("127.0.0.1:8889").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "127.0.0.1:8889");
        assert_eq!(rules[0].protocol, Protocol::Passthrough);
    }

    #[test]
    fn test_parse_single_http_ip_url_with_target() {
        let rules = parse_line("http://127.0.0.1:8889 http://127.0.0.1:9999").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://127.0.0.1:8889");
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[0].value, "127.0.0.1:9999");
    }

    #[test]
    fn test_parse_http_ip_url_forward_to_localhost() {
        let rules = parse_line("http://192.168.1.100:8080 http://localhost:3000").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://192.168.1.100:8080");
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[0].value, "localhost:3000");
    }

    #[test]
    fn test_parse_bare_ip_port_forward_to_target() {
        let rules = parse_line("127.0.0.1:8889 http://127.0.0.1:9999").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "127.0.0.1:8889");
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[0].value, "127.0.0.1:9999");
    }

    #[test]
    fn test_parse_bare_ip_port_forward_to_host() {
        let rules = parse_line("127.0.0.1:8889 host://127.0.0.1:9999").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "127.0.0.1:8889");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "127.0.0.1:9999");
    }

    #[test]
    fn test_parse_https_ip_url_forward() {
        let rules = parse_line("https://10.0.0.1:443 https://10.0.0.2:8443").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "https://10.0.0.1:443");
        assert_eq!(rules[0].protocol, Protocol::Https);
        assert_eq!(rules[0].value, "10.0.0.2:8443");
    }

    #[test]
    fn test_parse_http_localhost_forward_to_host_protocol() {
        let rules = parse_line("http://localhost:8080 host://192.168.1.1:3000").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "http://localhost:8080");
        assert_eq!(rules[0].protocol, Protocol::Host);
        assert_eq!(rules[0].value, "192.168.1.1:3000");
    }

    #[test]
    fn test_validate_single_http_ip_url_no_errors() {
        let content = r#"
http://127.0.0.1:8889
http://127.0.0.1:99900
http://192.168.1.1:3000
https://127.0.0.1:443
http://localhost:8080
127.0.0.1:8889
"#;
        let errors = validate_rules(content);
        assert!(
            errors.is_empty(),
            "Expected no errors for single URL rules, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_validate_ip_url_forward_rules_no_errors() {
        let content = r#"
http://127.0.0.1:8889 http://127.0.0.1:9999
http://192.168.1.100:8080 http://localhost:3000
127.0.0.1:8889 http://127.0.0.1:9999
127.0.0.1:8889 host://127.0.0.1:9999
https://10.0.0.1:443 https://10.0.0.2:8443
http://localhost:8080 host://192.168.1.1:3000
"#;
        let errors = validate_rules(content);
        assert!(
            errors.is_empty(),
            "Expected no errors for IP URL forward rules, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}
