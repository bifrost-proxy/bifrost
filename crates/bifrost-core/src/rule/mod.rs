mod context;
pub mod filter;
mod group;
mod parser;
mod references;
mod replace;
mod resolver;
mod syntax_report;
mod template;
mod types;
pub mod value_source;
mod value_store;

pub use context::{RequestContext, RequestContextBuilder};
pub use filter::{parse_filter, parse_line_props, Filter, LineProps};
pub use group::{RuleGroup, RuleGroupManager};
pub use parser::{
    extract_inline_variables, parse_line, parse_rules, parse_rules_tolerant, validate_rules,
    validate_rules_with_context, CodeFix, ParseError, ParseErrorSeverity, ParseResult, RuleParser,
    ScriptReference, ValidationResult, VariableInfo,
};
pub use references::{
    expand_rule_references, expand_rule_references_strict, expand_rule_references_to_result,
    rule_reference_name, RuleReferenceError,
};
pub use replace::parse_json_replace_pairs;
pub use resolver::{ResolvedRule, ResolvedRules, RulesResolver};
pub use syntax_report::{validate_rule_syntax_report, RuleSyntaxGuidance, RuleSyntaxReport};
pub use template::TemplateEngine;
pub use types::Rule;
pub use value_source::ValueSource;
pub use value_store::{
    create_shared_store, CompositeValueStore, MemoryValueStore, SharedValueStore, ValueStore,
};
