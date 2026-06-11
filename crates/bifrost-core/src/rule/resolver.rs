use crate::protocol::{Protocol, ProtocolCategory};
use crate::rule::filter::Filter;
use lru::LruCache as LruCacheImpl;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use super::context::RequestContext;
use super::template::TemplateEngine;
use super::types::Rule;
use super::{MemoryValueStore, ValueSource, ValueStore};

const DEFAULT_CACHE_CAPACITY: usize = 1000;

#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub rule: Rule,
    pub captures: Option<Vec<String>>,
    pub resolved_value: String,
}

impl ResolvedRule {
    pub fn new(
        rule: Rule,
        captures: Option<Vec<String>>,
        ctx: &RequestContext,
        values: &HashMap<String, String>,
    ) -> Self {
        let base_value = if matches!(rule.protocol, crate::protocol::Protocol::Bp) {
            match &rule.value_source {
                ValueSource::ValueRef(var_name) => values
                    .get(var_name)
                    .cloned()
                    .unwrap_or_else(|| rule.value.clone()),
                _ => rule.value.clone(),
            }
        } else if matches!(
            rule.protocol,
            crate::protocol::Protocol::File
                | crate::protocol::Protocol::RawFile
                | crate::protocol::Protocol::Tpl
        ) {
            rule.value.clone()
        } else {
            let store = MemoryValueStore::from_hashmap(values.clone());
            rule.value_source.resolve_with_fallback(&store)
        };
        let resolved_value =
            TemplateEngine::expand_with_context(&base_value, ctx, captures.as_deref(), values);

        Self {
            rule,
            captures,
            resolved_value,
        }
    }

    pub fn new_simple(
        rule: Rule,
        captures: Option<Vec<String>>,
        values: &HashMap<String, String>,
    ) -> Self {
        let ctx = RequestContext::new();
        Self::new(rule, captures, &ctx, values)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedRules {
    pub rules: Vec<ResolvedRule>,
    by_protocol: HashMap<Protocol, Vec<usize>>,
}

impl ResolvedRules {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            by_protocol: HashMap::new(),
        }
    }

    pub fn add(&mut self, resolved: ResolvedRule) {
        let idx = self.rules.len();
        let protocol = resolved.rule.protocol;
        self.rules.push(resolved);
        self.by_protocol.entry(protocol).or_default().push(idx);
    }

    pub fn get_by_protocol(&self, protocol: Protocol) -> Vec<&ResolvedRule> {
        self.by_protocol
            .get(&protocol)
            .map(|indices| indices.iter().map(|&i| &self.rules[i]).collect())
            .unwrap_or_default()
    }

    pub fn has_protocol(&self, protocol: Protocol) -> bool {
        self.by_protocol.contains_key(&protocol)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }
}

struct LruCache {
    cache: LruCacheImpl<String, ResolvedRules>,
}

impl LruCache {
    fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity)
            .unwrap_or(NonZeroUsize::new(DEFAULT_CACHE_CAPACITY).unwrap());
        Self {
            cache: LruCacheImpl::new(cap),
        }
    }

    fn peek(&self, key: &str) -> Option<ResolvedRules> {
        self.cache.peek(key).cloned()
    }

    #[cfg(test)]
    fn get(&mut self, key: &str) -> Option<ResolvedRules> {
        self.cache.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: ResolvedRules) {
        self.cache.put(key, value);
    }

    fn clear(&mut self) {
        self.cache.clear();
    }
}

pub struct RulesResolver {
    rules: Vec<Rule>,
    values: HashMap<String, String>,
    cache: Arc<RwLock<LruCache>>,
    cache_enabled: bool,
    /// Whether any rule has a filter whose match depends on per-request data that
    /// is NOT part of the base cache key (request headers/cookies, client IP).
    /// When true, the resolution cache key must also incorporate that data,
    /// otherwise two requests that differ only in those fields would share a
    /// cached result and header/cookie/IP filters would return stale matches.
    cache_key_needs_request_ctx: bool,
}

/// A filter matches against per-request data (request header/cookie/client IP)
/// that is not encoded in the base cache key. Resolutions for rule sets that use
/// such filters must key the cache on that data too.
fn rule_filter_needs_request_ctx(rule: &Rule) -> bool {
    rule.include_filters
        .iter()
        .chain(rule.exclude_filters.iter())
        .any(|f| {
            matches!(
                f,
                Filter::HeaderExists(_) | Filter::HeaderMatch { .. } | Filter::ClientIp(_)
            )
        })
}

impl RulesResolver {
    pub fn new(rules: Vec<Rule>) -> Self {
        let mut sorted_rules = rules;
        sorted_rules.sort_by_key(|b| std::cmp::Reverse(b.priority()));

        let cache_key_needs_request_ctx = sorted_rules.iter().any(rule_filter_needs_request_ctx);

        Self {
            rules: sorted_rules,
            values: HashMap::new(),
            cache: Arc::new(RwLock::new(LruCache::new(DEFAULT_CACHE_CAPACITY))),
            cache_enabled: true,
            cache_key_needs_request_ctx,
        }
    }

    pub fn with_values(mut self, values: HashMap<String, String>) -> Self {
        self.values = values;
        self
    }

    pub fn from_store(rules: Vec<Rule>, store: &dyn ValueStore) -> Self {
        let resolver = Self::new(rules);
        resolver.with_values(store.as_hashmap())
    }

    pub fn merge_from_store(&mut self, store: &dyn ValueStore) {
        for (k, v) in store.list() {
            self.values.entry(k).or_insert(v);
        }
        self.clear_cache();
    }

    pub fn values(&self) -> &HashMap<String, String> {
        &self.values
    }

    pub fn with_cache_capacity(self, capacity: usize) -> Self {
        *self.cache.write() = LruCache::new(capacity);
        self
    }

    pub fn disable_cache(mut self) -> Self {
        self.cache_enabled = false;
        self
    }

    pub fn set_value(&mut self, key: String, value: String) {
        self.values.insert(key, value);
        self.clear_cache();
    }

    pub fn add_rule(&mut self, rule: Rule) {
        let priority = rule.priority();
        if rule_filter_needs_request_ctx(&rule) {
            self.cache_key_needs_request_ctx = true;
        }
        let pos = self
            .rules
            .binary_search_by(|r| priority.cmp(&r.priority()))
            .unwrap_or_else(|e| e);
        self.rules.insert(pos, rule);
        self.clear_cache();
    }

    pub fn clear_cache(&self) {
        self.cache.write().clear();
    }

    pub fn has_response_rules_for_host(&self, host: &str) -> bool {
        let url_https = format!("https://{}", host);
        let url_http = format!("http://{}", host);
        for rule in &self.rules {
            if rule.is_disabled() || rule.is_negated() {
                continue;
            }
            let category = rule.protocol.category();
            if !matches!(
                category,
                ProtocolCategory::Response | ProtocolCategory::Both
            ) {
                continue;
            }
            if matches!(
                rule.protocol,
                Protocol::Host
                    | Protocol::XHost
                    | Protocol::Http
                    | Protocol::Https
                    | Protocol::Ws
                    | Protocol::Wss
                    | Protocol::Proxy
                    | Protocol::Redirect
                    | Protocol::File
                    | Protocol::Tpl
                    | Protocol::RawFile
                    | Protocol::Delete
                    | Protocol::Decode
                    | Protocol::UrlReplace
                    | Protocol::Pac
            ) {
                continue;
            }
            if !rule.matcher.can_trigger_tls_auto_intercept() {
                continue;
            }
            if rule.matcher.matches_host(&url_https, host)
                || rule.matcher.matches_host(&url_http, host)
            {
                return true;
            }
        }
        false
    }

    pub fn resolve(&self, ctx: &RequestContext) -> ResolvedRules {
        let cache_key = if self.cache_key_needs_request_ctx {
            // Some rule filters depend on request headers/cookies/client IP, which
            // are not in the base key. Mix a stable hash of them in so requests
            // that differ only in those fields don't collide on a stale result.
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            let mut headers: Vec<(&String, &String)> = ctx.req_headers.iter().collect();
            headers.sort();
            headers.hash(&mut hasher);
            let mut cookies: Vec<(&String, &String)> = ctx.req_cookies.iter().collect();
            cookies.sort();
            cookies.hash(&mut hasher);
            ctx.client_ip.hash(&mut hasher);
            format!(
                "{}|{}|{}|{}|{:x}",
                ctx.url,
                ctx.host,
                ctx.path,
                ctx.method,
                hasher.finish()
            )
        } else {
            format!("{}|{}|{}|{}", ctx.url, ctx.host, ctx.path, ctx.method)
        };

        tracing::trace!(
            target: "bifrost_core::rules",
            total_rules = self.rules.len(),
            url = %ctx.url,
            method = %ctx.method,
            "starting rule resolution"
        );

        if self.cache_enabled {
            if let Some(cached) = self.cache.read().peek(&cache_key) {
                tracing::trace!(
                    target: "bifrost_core::rules",
                    url = %ctx.url,
                    cached_rules = cached.rules.len(),
                    "returning cached result"
                );
                return cached;
            }
        }
        let mut result = ResolvedRules::new();
        let mut matched_protocols: HashMap<Protocol, bool> = HashMap::new();
        let active_skips = self.collect_active_skips(ctx);

        for rule in &self.rules {
            if rule.is_disabled() {
                continue;
            }

            if rule.is_negated() {
                // 对于否定规则，我们需要检查原始匹配结果（不取反）
                // 如果原始 pattern 匹配成功，则标记该协议已被匹配，阻止后续同协议规则
                //
                // 注意：rule.matcher.matches() 对于否定规则返回的是取反后的结果
                // 所以这里需要再次取反来获得原始匹配结果
                let match_result = rule.matcher.matches(&ctx.url, &ctx.host, &ctx.path);
                // 对于否定规则，matched=true 意味着原始 pattern 不匹配（因为被取反了）
                // 我们需要的是原始匹配结果，所以再次取反
                let original_matched = !match_result.matched;
                if original_matched {
                    matched_protocols.insert(rule.protocol, true);
                }
                continue;
            }

            if !rule.protocol.is_multi_match() && matched_protocols.contains_key(&rule.protocol) {
                continue;
            }

            let match_result = rule.matcher.matches(&ctx.url, &ctx.host, &ctx.path);
            tracing::trace!(
                target: "bifrost_core::rules",
                pattern = %rule.pattern,
                matched = match_result.matched,
                url = %ctx.url,
                host = %ctx.host,
                path = %ctx.path,
                file = rule.file.as_deref().unwrap_or("<unknown>"),
                line = rule.line.unwrap_or(0),
                "rule match attempt"
            );
            if !match_result.matched {
                continue;
            }
            tracing::debug!(
                target: "bifrost_core::rules",
                pattern = %rule.pattern,
                protocol = %rule.protocol.to_str(),
                value = %rule.value,
                raw = %rule.raw,
                file = rule.file.as_deref().unwrap_or("<unknown>"),
                line = rule.line.unwrap_or(0),
                "rule MATCHED"
            );

            if !rule.include_filters.is_empty()
                && !Self::matches_all_filters(&rule.include_filters, ctx)
            {
                continue;
            }

            if Self::matches_any_filter(&rule.exclude_filters, ctx) {
                tracing::debug!(
                    target: "bifrost_core::rules",
                    pattern = %rule.pattern,
                    path = %ctx.path,
                    "rule excluded by excludeFilter"
                );
                continue;
            }

            if rule.protocol != Protocol::Skip
                && Self::should_skip_rule(rule, &active_skips, &self.values, ctx)
            {
                tracing::debug!(
                    target: "bifrost_core::rules",
                    pattern = %rule.pattern,
                    protocol = %rule.protocol.to_str(),
                    "rule skipped by skip directive"
                );
                continue;
            }

            if rule.protocol == Protocol::Skip {
                continue;
            }

            let resolved =
                ResolvedRule::new(rule.clone(), match_result.captures, ctx, &self.values);
            result.add(resolved);

            if !rule.protocol.is_multi_match() {
                matched_protocols.insert(rule.protocol, true);
            }
        }

        if self.cache_enabled {
            self.cache.write().insert(cache_key, result.clone());
        }

        tracing::debug!(
            target: "bifrost_core::rules",
            url = %ctx.url,
            matched_count = result.rules.len(),
            "rule resolution completed"
        );

        result
    }

    fn matches_all_filters(filters: &[Filter], ctx: &RequestContext) -> bool {
        filters.iter().all(|f| Self::matches_filter(f, ctx))
    }

    fn matches_any_filter(filters: &[Filter], ctx: &RequestContext) -> bool {
        filters.iter().any(|f| Self::matches_filter(f, ctx))
    }

    fn matches_filter(filter: &Filter, ctx: &RequestContext) -> bool {
        match filter {
            Filter::Method(methods) => {
                let req_method = ctx.method.to_uppercase();
                methods.iter().any(|m| m.to_uppercase() == req_method)
            }
            Filter::StatusCode(range) => {
                if let Some(status) = ctx.status_code {
                    range.matches(status)
                } else {
                    false
                }
            }
            Filter::Path(matcher) => matcher.matches(&ctx.path),
            Filter::HeaderExists(name) => ctx.req_headers.contains_key(&name.to_lowercase()),
            Filter::HeaderMatch {
                name,
                pattern,
                is_request,
            } => {
                let headers = if *is_request {
                    Some(&ctx.req_headers)
                } else {
                    ctx.res_headers.as_ref()
                };
                if let Some(headers) = headers {
                    if let Some(value) = headers.get(&name.to_lowercase()) {
                        return pattern.is_match(value);
                    }
                }
                false
            }
            Filter::ClientIp(matcher) => matcher.matches(&ctx.client_ip),
            Filter::Body(_regex) => false,
            Filter::Url(matcher) => matcher.matches(&ctx.url, &ctx.host, &ctx.path),
            Filter::Custom(_key, _value) => true,
        }
    }

    fn should_skip_rule(
        rule: &Rule,
        active_skips: &[SkipRule],
        values: &HashMap<String, String>,
        ctx: &RequestContext,
    ) -> bool {
        if active_skips.is_empty() {
            return false;
        }

        let resolved_value = ResolvedRule::new(rule.clone(), None, ctx, values).resolved_value;
        active_skips
            .iter()
            .any(|skip| skip.matches(rule, &resolved_value))
    }

    fn collect_active_skips(&self, ctx: &RequestContext) -> Vec<SkipRule> {
        let mut active_skips = Vec::new();

        for rule in &self.rules {
            if rule.protocol != Protocol::Skip || rule.is_disabled() || rule.is_negated() {
                continue;
            }

            let match_result = rule.matcher.matches(&ctx.url, &ctx.host, &ctx.path);
            if !match_result.matched {
                continue;
            }

            if !rule.include_filters.is_empty()
                && !Self::matches_all_filters(&rule.include_filters, ctx)
            {
                continue;
            }

            if Self::matches_any_filter(&rule.exclude_filters, ctx) {
                continue;
            }

            if let Some(skip_rule) = SkipRule::parse(&rule.value) {
                active_skips.push(skip_rule);
            }
        }

        active_skips
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SkipRule {
    Pattern(String),
    Operation(String),
}

impl SkipRule {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(pattern) = value.strip_prefix("pattern=") {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                None
            } else {
                Some(Self::Pattern(pattern.to_string()))
            }
        } else if let Some(operation) = value.strip_prefix("operation=") {
            let operation = normalize_skip_operation(operation);
            if operation.is_empty() {
                None
            } else {
                Some(Self::Operation(operation))
            }
        } else {
            None
        }
    }

    fn matches(&self, rule: &Rule, resolved_value: &str) -> bool {
        match self {
            Self::Pattern(pattern) => &rule.pattern == pattern,
            Self::Operation(operation) => {
                let raw_operation = normalize_skip_operation(&format!(
                    "{}://{}",
                    rule.protocol.to_str(),
                    rule.value
                ));
                let resolved_operation = normalize_skip_operation(&format!(
                    "{}://{}",
                    rule.protocol.to_str(),
                    resolved_value
                ));
                operation == &raw_operation || operation == &resolved_operation
            }
        }
    }
}

fn normalize_skip_operation(operation: &str) -> String {
    let operation = operation.trim();
    let Some((protocol, value)) = operation.split_once("://") else {
        return operation.to_string();
    };

    let value = value.trim();
    let normalized_value = if value.starts_with('`') && value.ends_with('`') && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    };

    format!("{}://{}", protocol.trim(), normalized_value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::WildcardMatcher;
    use crate::rule::filter::{parse_filter, LineProps};
    use std::sync::Arc;

    fn create_test_rule(pattern: &str, protocol: Protocol, value: &str) -> Rule {
        let matcher = Arc::new(WildcardMatcher::new(pattern).unwrap());
        Rule::new(
            pattern.to_string(),
            matcher,
            protocol,
            value.to_string(),
            format!("{} {}://{}", pattern, protocol.to_str(), value),
        )
    }

    fn parse_test_rule(line: &str) -> Vec<Rule> {
        crate::rule::parser::RuleParser::new()
            .parse_line(line)
            .unwrap_or_else(|err| panic!("failed to parse test rule `{}`: {}", line, err))
    }

    fn create_test_rule_with_filters(
        pattern: &str,
        protocol: Protocol,
        value: &str,
        include_filters: Vec<Filter>,
        exclude_filters: Vec<Filter>,
    ) -> Rule {
        let matcher = Arc::new(WildcardMatcher::new(pattern).unwrap());
        Rule::new(
            pattern.to_string(),
            matcher,
            protocol,
            value.to_string(),
            format!("{} {}://{}", pattern, protocol.to_str(), value),
        )
        .with_include_filters(include_filters)
        .with_exclude_filters(exclude_filters)
    }

    fn create_test_context(url: &str, host: &str, path: &str) -> RequestContext {
        RequestContext::builder()
            .url(url)
            .host(host)
            .hostname(host)
            .path(path)
            .pathname(path)
            .build()
    }

    #[test]
    fn test_request_context_new() {
        let ctx = create_test_context("http://example.com/path", "example.com", "/path");
        assert_eq!(ctx.url, "http://example.com/path");
        assert_eq!(ctx.host, "example.com");
        assert_eq!(ctx.path, "/path");
    }

    #[test]
    fn test_resolved_rules_new() {
        let result = ResolvedRules::new();
        assert!(result.is_empty());
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_resolved_rules_add() {
        let mut result = ResolvedRules::new();
        let rule = create_test_rule("*.example.com", Protocol::Host, "127.0.0.1");
        let resolved = ResolvedRule::new_simple(rule, None, &HashMap::new());
        result.add(resolved);

        assert!(!result.is_empty());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_resolved_rules_get_by_protocol() {
        let mut result = ResolvedRules::new();

        let rule1 = create_test_rule("*.example.com", Protocol::Host, "127.0.0.1");
        let rule2 = create_test_rule("*.api.com", Protocol::Proxy, "proxy:8080");

        result.add(ResolvedRule::new_simple(rule1, None, &HashMap::new()));
        result.add(ResolvedRule::new_simple(rule2, None, &HashMap::new()));

        let host_rules = result.get_by_protocol(Protocol::Host);
        assert_eq!(host_rules.len(), 1);

        let proxy_rules = result.get_by_protocol(Protocol::Proxy);
        assert_eq!(proxy_rules.len(), 1);
    }

    #[test]
    fn test_resolved_rules_has_protocol() {
        let mut result = ResolvedRules::new();
        let rule = create_test_rule("*.example.com", Protocol::Host, "127.0.0.1");
        result.add(ResolvedRule::new_simple(rule, None, &HashMap::new()));

        assert!(result.has_protocol(Protocol::Host));
        assert!(!result.has_protocol(Protocol::Proxy));
    }

    #[test]
    fn test_rules_resolver_new() {
        let rules = vec![
            create_test_rule("*.example.com", Protocol::Host, "127.0.0.1"),
            create_test_rule("example.com", Protocol::Host, "127.0.0.2"),
        ];
        let resolver = RulesResolver::new(rules);
        assert_eq!(resolver.rule_count(), 2);
    }

    #[test]
    fn test_rules_resolver_priority_sorting() {
        let rules = vec![
            create_test_rule("*.example.com", Protocol::Host, "127.0.0.1"),
            create_test_rule("example.com", Protocol::Host, "127.0.0.2"),
        ];
        let resolver = RulesResolver::new(rules);
        assert!(resolver.rules()[0].priority() >= resolver.rules()[1].priority());
    }

    #[test]
    fn test_rules_resolver_resolve() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert!(result.has_protocol(Protocol::Host));
    }

    #[test]
    fn test_rules_resolver_no_match() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.other.com/path", "www.other.com", "/path");

        let result = resolver.resolve(&ctx);
        assert!(result.is_empty());
    }

    #[test]
    fn test_rules_resolver_with_values() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "${target}",
        )];

        let mut values = HashMap::new();
        values.insert("target".to_string(), "127.0.0.1".to_string());

        let resolver = RulesResolver::new(rules).with_values(values);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "127.0.0.1");
    }

    #[test]
    fn test_rules_resolver_with_value_ref() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ReqHeaders,
            "{authHeaders}",
        )];

        let mut values = HashMap::new();
        values.insert(
            "authHeaders".to_string(),
            "X-Auth-Token: secret-12345".to_string(),
        );

        let resolver = RulesResolver::new(rules).with_values(values);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "X-Auth-Token: secret-12345");
    }

    #[test]
    fn test_value_ref_with_parsed_rules() {
        use crate::parse_rules;

        let rules = parse_rules("test.local reqHeaders://{customHeaders}").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value, "{customHeaders}");

        let mut values = HashMap::new();
        values.insert(
            "customHeaders".to_string(),
            "X-Custom-Token: secret-12345".to_string(),
        );

        let resolver = RulesResolver::new(rules).with_values(values);

        let ctx = RequestContext::from_url("http://test.local/api");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.rules[0].resolved_value,
            "X-Custom-Token: secret-12345"
        );
    }

    #[test]
    fn test_bp_remote_url_is_preserved_as_script_reference() {
        let rule = create_test_rule(
            "bp.example.com",
            Protocol::Bp,
            "http://127.0.0.1:18080/parser.js?sha256=abc",
        );
        let resolver = RulesResolver::new(vec![rule]);
        let ctx = RequestContext::from_url("http://bp.example.com/api");

        let result = resolver.resolve(&ctx);

        assert_eq!(
            result.rules[0].resolved_value,
            "http://127.0.0.1:18080/parser.js?sha256=abc"
        );
    }

    #[test]
    fn test_bp_value_ref_expands_profile_without_remote_fetch() {
        let rule = create_test_rule("bp.example.com", Protocol::Bp, "{order_bp}");
        let mut values = HashMap::new();
        values.insert(
            "order_bp".to_string(),
            "build_in_bp?psm=foo.bar.order&idlSource=bam".to_string(),
        );
        let resolver = RulesResolver::new(vec![rule]).with_values(values);
        let ctx = RequestContext::from_url("http://bp.example.com/api");

        let result = resolver.resolve(&ctx);

        assert_eq!(
            result.rules[0].resolved_value,
            "build_in_bp?psm=foo.bar.order&idlSource=bam"
        );
    }

    #[test]
    fn test_rules_resolver_add_rule() {
        let mut resolver = RulesResolver::new(vec![]);
        assert_eq!(resolver.rule_count(), 0);

        resolver.add_rule(create_test_rule(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
        ));
        assert_eq!(resolver.rule_count(), 1);
    }

    #[test]
    fn test_rules_resolver_set_value() {
        let mut resolver = RulesResolver::new(vec![]);
        resolver.set_value("key".to_string(), "value".to_string());
        assert_eq!(resolver.values.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_rules_resolver_cache() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result1 = resolver.resolve(&ctx);
        let result2 = resolver.resolve(&ctx);

        assert_eq!(result1.len(), result2.len());
    }

    #[test]
    fn test_rules_resolver_disable_cache() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
        )];
        let resolver = RulesResolver::new(rules).disable_cache();

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_rules_resolver_clear_cache() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let _ = resolver.resolve(&ctx);
        resolver.clear_cache();

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_lru_cache_eviction() {
        let mut cache = LruCache::new(2);
        cache.insert("key1".to_string(), ResolvedRules::new());
        cache.insert("key2".to_string(), ResolvedRules::new());
        let _ = cache.get("key1");
        cache.insert("key3".to_string(), ResolvedRules::new());

        assert!(cache.get("key1").is_some());
        assert!(cache.get("key2").is_none());
        assert!(cache.get("key3").is_some());
    }

    #[test]
    fn test_multi_match_protocol() {
        let rules = vec![
            create_test_rule("*.example.com", Protocol::ReqHeaders, "header1=value1"),
            create_test_rule("*.example.com", Protocol::ReqHeaders, "header2=value2"),
        ];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_single_match_protocol() {
        let rules = vec![
            create_test_rule("*.example.com", Protocol::Host, "127.0.0.1"),
            create_test_rule("*.example.com", Protocol::Host, "127.0.0.2"),
        ];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_builtin_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::Host,
            "host-${hostname}",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "host-www.example.com");
    }

    #[test]
    fn test_url_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ResBody,
            "${url}",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context(
            "http://www.example.com/api/test",
            "www.example.com",
            "/api/test",
        );

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.rules[0].resolved_value,
            "http://www.example.com/api/test"
        );
    }

    #[test]
    fn test_path_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ResBody,
            "path=${path}",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = create_test_context(
            "http://www.example.com/api/test?foo=bar",
            "www.example.com",
            "/api/test?foo=bar",
        );

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "path=/api/test?foo=bar");
    }

    #[test]
    fn test_method_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ResBody,
            "method=${method}",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .method("POST")
            .build();

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "method=POST");
    }

    #[test]
    fn test_client_ip_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ResBody,
            "client=${clientIp}",
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .client_ip("192.168.1.100")
            .build();

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "client=192.168.1.100");
    }

    #[test]
    fn test_header_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ResBody,
            "auth=${reqHeaders.authorization}",
        )];
        let resolver = RulesResolver::new(rules);

        let mut headers = HashMap::new();
        headers.insert("authorization".to_string(), "Bearer token123".to_string());

        let ctx = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .req_headers(headers)
            .build();

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "auth=Bearer token123");
    }

    #[test]
    fn test_cookie_variable_expansion() {
        let rules = vec![create_test_rule(
            "*.example.com",
            Protocol::ResBody,
            "session=${reqCookies.session_id}",
        )];
        let resolver = RulesResolver::new(rules);

        let mut cookies = HashMap::new();
        cookies.insert("session_id".to_string(), "abc123".to_string());

        let ctx = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .req_cookies(cookies)
            .build();

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "session=abc123");
    }

    #[test]
    fn test_include_filter_method() {
        let include_filters = vec![parse_filter("m:GET").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
            include_filters,
            vec![],
        )];
        let resolver = RulesResolver::new(rules);

        let ctx_get = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .method("GET")
            .build();

        let result = resolver.resolve(&ctx_get);
        assert_eq!(result.len(), 1);

        let ctx_post = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .method("POST")
            .build();

        let result = resolver.resolve(&ctx_post);
        assert!(result.is_empty());
    }

    #[test]
    fn test_exclude_filter_path() {
        let exclude_filters = vec![parse_filter("/admin/").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
            vec![],
            exclude_filters,
        )];
        let resolver = RulesResolver::new(rules);

        let ctx_api = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .build();

        let result = resolver.resolve(&ctx_api);
        assert_eq!(result.len(), 1);

        let ctx_admin = RequestContext::builder()
            .url("http://www.example.com/admin/users")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/admin/users")
            .pathname("/admin/users")
            .build();

        let result = resolver.resolve(&ctx_admin);
        assert!(result.is_empty());
    }

    #[test]
    fn test_exclude_filter_whistle_style_wildcard_url() {
        let exclude_filters = vec![parse_filter("*/alice/*").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "*.doubao.com",
            Protocol::Http,
            "localhost:8080",
            vec![],
            exclude_filters,
        )];
        let resolver = RulesResolver::new(rules);

        let excluded = resolver.resolve(&RequestContext::from_url(
            "https://www.doubao.com/alice/commerce/sale/subscription/entry/config/?from=test",
        ));
        assert!(excluded.is_empty());

        let allowed = resolver.resolve(&RequestContext::from_url(
            "https://www.doubao.com/bob/commerce/sale/subscription/entry/config/?from=test",
        ));
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed.rules[0].resolved_value, "localhost:8080");
    }

    #[test]
    fn test_exclude_filter_whistle_style_wildcard_path_prefix() {
        let exclude_filters = vec![parse_filter("*/api").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "www.example.com",
            Protocol::Http,
            "localhost:5173",
            vec![],
            exclude_filters,
        )];
        let resolver = RulesResolver::new(rules);

        assert!(resolver
            .resolve(&RequestContext::from_url(
                "https://www.example.com/api/users"
            ))
            .is_empty());
        assert!(resolver
            .resolve(&RequestContext::from_url(
                "https://www.example.com/api?debug=1"
            ))
            .is_empty());

        let allowed = resolver.resolve(&RequestContext::from_url(
            "https://www.example.com/apiary/users",
        ));
        assert_eq!(allowed.len(), 1);
    }

    #[test]
    fn test_long_exclude_filter_chain_uses_regular_prefix_matching() {
        let excludes = [
            "/account/page/cooperate/qianchuan",
            "/garrmodlistv3",
            "/vmok-modules",
            "/qc_main_mono",
            "/qcServiceWorker",
            "/promotion-v2",
            "/product",
            "/nbs",
            "/finance",
            "/creative_config",
            "/check_login",
            "/brand_main",
            "/brand",
            "/aweme",
            "/apps",
            "/api",
            "/ad",
            "/account",
            "/_AMapService",
            "/zhitui",
            "/uni-prom",
            "/trident_v2",
            "/star",
            "/promotion",
            "/notify",
            "/mp",
            "/magic",
            "/home",
            "/doris/ad_v2",
            "/doris-report",
            "/docs",
            "/dataV2",
            "/creative-preview-apps",
            "/creation",
            "/createV2",
            "/copilot",
            "/cg_trade",
            "/cf/ml",
            "/brand_shop_window",
            "/brand_pre_sales",
            "/brand_pre_review",
            "/brand_inquiry_tool",
            "/bp",
            "/arcosite-api",
            "/alpha_sw",
            "/ttwid",
            "/webcast",
            "/uni-creation",
            "/support",
            "/sta_statement",
            "/sta_invoice",
            "/sta_deposit",
            "/sso",
            "/site",
            "/selfcreative",
            "/rule",
            "/risk-control",
            "/refund",
            "/passport",
            "/openapi",
            "/ocic",
            "/mobile",
            "/marketing",
            "/jarvis",
            "/im-linkchat",
            "/help",
            "/growth",
            "/fund_report",
            "/fund_refund",
            "/fund_recharge",
            "/ecp",
            "/e_adv",
            "/draft",
            "/doushop",
            "/credit",
            "/community_security",
            "/cognition",
            "/cip_invoice",
            "/cg_project/backend/open",
            "/cg_detain",
            "/cg_deposit/backend",
            "/cg_cont",
            "/cg_charge",
            "/cg_cert_center/back_end",
            "/cg_cert",
            "/cc-external",
            "/brand_node",
            "/brand_fe",
            "/board-next",
            "/board",
            "/app",
            "/advocacy",
            "/adstyle",
            "/account_security",
            "/account_info_help",
            "/login",
            "/tools",
            "/creative",
            "/star-pages",
        ];
        let filter_chain = excludes
            .iter()
            .map(|path| format!("excludeFilter://{}", path))
            .collect::<Vec<_>>()
            .join(" ");
        let rule_text = format!("qianchuan.jinritemai.com 10.37.102.138:8080 {filter_chain}");
        let rules = crate::parse_rules(&rule_text).unwrap();
        assert_eq!(rules[0].exclude_filters.len(), excludes.len());
        let resolver = RulesResolver::new(rules);

        for (idx, path) in excludes.iter().enumerate() {
            let url = format!("https://qianchuan.jinritemai.com{}-suffix?case={idx}", path);
            let result = resolver.resolve(&RequestContext::from_url(&url));
            assert!(
                result.is_empty(),
                "excludeFilter://{path} should exclude prefix URL {url}"
            );
        }

        let result = resolver.resolve(&RequestContext::from_url(
            "https://qianchuan.jinritemai.com/not-listed/path?case=allowed",
        ));
        assert_eq!(result.len(), 1);
        assert_eq!(result.rules[0].resolved_value, "10.37.102.138:8080");
    }

    #[test]
    fn test_combined_include_exclude_filters() {
        let include_filters = vec![parse_filter("m:GET,POST").unwrap()];
        let exclude_filters = vec![parse_filter("/health/").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
            include_filters,
            exclude_filters,
        )];
        let resolver = RulesResolver::new(rules);

        let ctx = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .method("GET")
            .build();

        let result = resolver.resolve(&ctx);
        assert_eq!(result.len(), 1);

        let ctx_health = RequestContext::builder()
            .url("http://www.example.com/health/")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/health/")
            .pathname("/health/")
            .method("GET")
            .build();

        let result = resolver.resolve(&ctx_health);
        assert!(result.is_empty());
    }

    #[test]
    fn test_include_filter_header_exists() {
        let include_filters = vec![parse_filter("h:X-Custom-Header").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
            include_filters,
            vec![],
        )];
        let resolver = RulesResolver::new(rules).disable_cache();

        let ctx_with_header = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .header("X-Custom-Header", "value")
            .build();

        let result = resolver.resolve(&ctx_with_header);
        assert_eq!(result.len(), 1);

        let ctx_without_header = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .build();

        let result = resolver.resolve(&ctx_without_header);
        assert!(result.is_empty());
    }

    #[test]
    fn test_include_filter_client_ip() {
        let include_filters = vec![parse_filter("i:192.168.0.0/16").unwrap()];
        let rules = vec![create_test_rule_with_filters(
            "*.example.com",
            Protocol::Host,
            "127.0.0.1",
            include_filters,
            vec![],
        )];
        let resolver = RulesResolver::new(rules).disable_cache();

        let ctx_match = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .client_ip("192.168.1.100")
            .build();

        let result = resolver.resolve(&ctx_match);
        assert_eq!(result.len(), 1);

        let ctx_no_match = RequestContext::builder()
            .url("http://www.example.com/api")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/api")
            .pathname("/api")
            .client_ip("10.0.0.1")
            .build();

        let result = resolver.resolve(&ctx_no_match);
        assert!(result.is_empty());
    }

    #[test]
    fn test_disabled_rule() {
        let matcher = Arc::new(WildcardMatcher::new("*.example.com").unwrap());
        let rule = Rule::new(
            "*.example.com".to_string(),
            matcher,
            Protocol::Host,
            "127.0.0.1".to_string(),
            "*.example.com host://127.0.0.1".to_string(),
        )
        .with_line_props(LineProps {
            important: false,
            disabled: true,
        });

        let resolver = RulesResolver::new(vec![rule]);

        let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

        let result = resolver.resolve(&ctx);
        assert!(result.is_empty());
    }

    #[test]
    fn test_has_response_rules_for_host_allows_explicit_domain_and_ip_scope() {
        let mut rules = Vec::new();
        rules.extend(parse_test_rule(
            "tls-auto-domain.local resHeaders://X-Auto-Tls=domain",
        ));
        rules.extend(parse_test_rule("127.0.0.1 resHeaders://X-Auto-Tls=ip"));

        let resolver = RulesResolver::new(rules);

        assert!(resolver.has_response_rules_for_host("tls-auto-domain.local"));
        assert!(resolver.has_response_rules_for_host("127.0.0.1"));
        assert!(!resolver.has_response_rules_for_host("other.local"));
    }

    #[test]
    fn test_has_response_rules_for_host_allows_host_scoped_wildcards() {
        let mut rules = Vec::new();
        rules.extend(parse_test_rule(
            "*.tls-auto.local resHeaders://X-Auto-Tls=wildcard",
        ));
        rules.extend(parse_test_rule(
            "^path.tls-auto.local/api/* resHeaders://X-Auto-Tls=path",
        ));

        let resolver = RulesResolver::new(rules);

        assert!(resolver.has_response_rules_for_host("api.tls-auto.local"));
        assert!(resolver.has_response_rules_for_host("path.tls-auto.local"));
        assert!(!resolver.has_response_rules_for_host("tls-auto.local"));
    }

    #[test]
    fn test_has_response_rules_for_host_rejects_pure_regex_and_wildcards() {
        let mut rules = Vec::new();
        rules.extend(parse_test_rule("* resHeaders://X-Auto-Tls=wildcard"));
        rules.extend(parse_test_rule(
            "*/api/* resHeaders://X-Auto-Tls=path-wildcard",
        ));
        rules.extend(parse_test_rule(
            "/regex-auto\\.local/ resHeaders://X-Auto-Tls=regex",
        ));

        let resolver = RulesResolver::new(rules);

        assert!(!resolver.has_response_rules_for_host("regex-auto.local"));
        assert!(!resolver.has_response_rules_for_host("wildcard-auto.local"));
    }

    #[test]
    fn test_important_priority_ordering() {
        let matcher1 = Arc::new(WildcardMatcher::new("*.example.com").unwrap());
        let rule1 = Rule::new(
            "*.example.com".to_string(),
            matcher1,
            Protocol::Host,
            "127.0.0.1".to_string(),
            "*.example.com host://127.0.0.1".to_string(),
        );

        let matcher2 = Arc::new(WildcardMatcher::new("*.example.com").unwrap());
        let rule2 = Rule::new(
            "*.example.com".to_string(),
            matcher2,
            Protocol::Host,
            "127.0.0.2".to_string(),
            "*.example.com host://127.0.0.2".to_string(),
        )
        .with_line_props(LineProps {
            important: true,
            disabled: false,
        });

        let resolver = RulesResolver::new(vec![rule1, rule2]);

        assert!(resolver.rules()[0].line_props.important);
        assert!(resolver.rules()[0].priority() > resolver.rules()[1].priority());
    }

    #[test]
    fn test_path_wildcard_double_star_matching() {
        use crate::matcher::PathWildcardMatcher;

        let pattern = "^path-double.local/api/**";
        let matcher = Arc::new(PathWildcardMatcher::new(pattern).unwrap());

        let rule = Rule::new(
            pattern.to_string(),
            matcher,
            Protocol::Host,
            "127.0.0.1:3000".to_string(),
            format!("{} host://127.0.0.1:3000", pattern),
        );

        let resolver = RulesResolver::new(vec![rule]);
        let ctx = RequestContext::from_url("http://path-double.local/api/users");
        let result = resolver.resolve(&ctx);

        assert_eq!(result.len(), 1, "Should match the path wildcard rule");
        assert_eq!(result.rules[0].resolved_value, "127.0.0.1:3000");
    }

    #[test]
    fn test_path_wildcard_via_rule_parser() {
        use crate::rule::parser::RuleParser;

        let rule_text = "^path-double.local/api/** http://127.0.0.1:3000";
        let parser = RuleParser::new();
        let rules = parser.parse_line(rule_text).expect("Failed to parse rule");

        assert_eq!(rules.len(), 1, "Should parse one rule");

        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url("http://path-double.local/api/users");
        let result = resolver.resolve(&ctx);

        assert_eq!(result.len(), 1, "Should match the path wildcard rule");
    }

    #[test]
    fn test_host_rule_matches_request_with_explicit_port() {
        use crate::rule::parser::RuleParser;

        let parser = RuleParser::new();
        let rules = parser
            .parse_line("127.0.0.1 reqHeaders://X-UI-Rule=alpha")
            .expect("Failed to parse rule");

        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url("http://127.0.0.1:18084/rules-check");
        let result = resolver.resolve(&ctx);

        assert_eq!(
            result.len(),
            1,
            "Host-only rule should match requests with an explicit port"
        );
        assert_eq!(result.rules[0].resolved_value, "X-UI-Rule=alpha");
    }

    #[test]
    fn test_negated_rule_does_not_block_other_patterns() {
        use crate::rule::parser::RuleParser;

        // 否定规则不应该阻止不匹配的其他规则
        let parser = RuleParser::new();
        let mut rules = parser
            .parse_line("!^path-negate.local/api/* http://127.0.0.1:3000")
            .unwrap();
        rules.extend(
            parser
                .parse_line("^path-double.local/api/** http://127.0.0.1:3000")
                .unwrap(),
        );

        let resolver = RulesResolver::new(rules);

        // 请求 path-double.local，不应该被 path-negate 的否定规则影响
        let ctx = RequestContext::from_url("http://path-double.local/api/users");
        let result = resolver.resolve(&ctx);

        assert_eq!(result.len(), 1, "Should match path-double rule");
        assert_eq!(
            result.rules[0].rule.pattern, "^path-double.local/api/**",
            "Should match the correct rule"
        );
    }

    #[test]
    fn test_negated_rule_blocks_matching_pattern() {
        use crate::rule::parser::RuleParser;

        // 否定规则应该阻止匹配的同协议规则
        let parser = RuleParser::new();
        let mut rules = parser
            .parse_line("!^path-negate.local/api/* http://127.0.0.1:3000")
            .unwrap();
        rules.extend(
            parser
                .parse_line("^path-negate.local/api/** http://127.0.0.1:3000")
                .unwrap(),
        );

        let resolver = RulesResolver::new(rules);

        // 请求 path-negate.local，应该被否定规则阻止
        let ctx = RequestContext::from_url("http://path-negate.local/api/users");
        let result = resolver.resolve(&ctx);

        assert_eq!(result.len(), 0, "Should be blocked by the negated rule");
    }

    #[test]
    fn test_multiple_different_protocols_all_match() {
        use crate::rule::parser::RuleParser;

        let parser = RuleParser::new();
        let rules = parser
            .parse_line("test.local http://127.0.0.1:3000 resBody://{test-body}")
            .unwrap();

        assert_eq!(
            rules.len(),
            2,
            "Should create 2 rules for different protocols"
        );
        assert_eq!(rules[0].protocol, Protocol::Http);
        assert_eq!(rules[1].protocol, Protocol::ResBody);

        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url("http://test.local/path");
        let result = resolver.resolve(&ctx);

        assert_eq!(result.len(), 2, "Both Http and ResBody rules should match");
        assert!(result.has_protocol(Protocol::Http));
        assert!(result.has_protocol(Protocol::ResBody));
    }

    #[test]
    fn test_rules_resolver_skip_by_operation_allows_fallback_rule() {
        use crate::rule::parser::RuleParser;

        let parser = RuleParser::new();
        let rules = parser
            .parse_line(
                "skip-operation.local http://127.0.0.1:3000 resHeaders://`X-Skip-Op:first` resHeaders://`X-Skip-Op:second` skip://operation=resHeaders://`X-Skip-Op:first`",
            )
            .unwrap();
        let resolver = RulesResolver::new(rules);

        let ctx = RequestContext::from_url("http://skip-operation.local/test");
        let result = resolver.resolve(&ctx);
        let header_rules = result.get_by_protocol(Protocol::ResHeaders);

        assert_eq!(header_rules.len(), 1);
        assert_eq!(header_rules[0].resolved_value, "X-Skip-Op:second");
    }

    #[test]
    fn test_rules_resolver_skip_by_pattern_allows_fallback_rule() {
        use crate::rule::parser::RuleParser;

        let parser = RuleParser::new();
        let mut rules = parser
            .parse_line("skip-pattern.local/api/blocked http://127.0.0.1:3000 resHeaders://`X-Skip-Pattern:blocked`")
            .unwrap();
        rules.extend(
            parser
                .parse_line("skip-pattern.local/api http://127.0.0.1:3000 resHeaders://`X-Skip-Pattern:fallback`")
                .unwrap(),
        );
        rules.extend(
            parser
                .parse_line(
                    "skip-pattern.local/api/blocked skip://pattern=skip-pattern.local/api/blocked",
                )
                .unwrap(),
        );
        let resolver = RulesResolver::new(rules);

        let ctx = RequestContext::from_url("http://skip-pattern.local/api/blocked");
        let result = resolver.resolve(&ctx);
        let header_rules = result.get_by_protocol(Protocol::ResHeaders);

        assert_eq!(header_rules.len(), 1);
        assert_eq!(header_rules[0].resolved_value, "X-Skip-Pattern:fallback");
    }

    #[test]
    fn test_path_specific_rule_takes_priority_over_domain_only_rule() {
        use crate::rule::parser::RuleParser;

        let parser = RuleParser::new();

        let mut rules = parser
            .parse_line("qianchuan.jinritemai.com/ad/api/ qianchuan.jinritemai.com/ad/")
            .unwrap();
        rules.extend(
            parser
                .parse_line("qianchuan.jinritemai.com localhost:8080")
                .unwrap(),
        );

        assert!(!rules.is_empty());

        let resolver = RulesResolver::new(rules);

        // With full path: more specific rule should win
        let ctx = RequestContext::from_url("https://qianchuan.jinritemai.com/ad/api/test");
        let result = resolver.resolve(&ctx);

        let host_rules = result.get_by_protocol(Protocol::Host);
        assert_eq!(host_rules.len(), 1);
        assert_eq!(host_rules[0].resolved_value, "qianchuan.jinritemai.com/ad/");
        assert_eq!(
            host_rules[0].rule.pattern,
            "qianchuan.jinritemai.com/ad/api/"
        );
    }

    #[test]
    fn test_domain_only_rule_matches_connect_without_path() {
        use crate::rule::parser::RuleParser;

        let parser = RuleParser::new();

        let mut rules = parser
            .parse_line("qianchuan.jinritemai.com/ad/api/ qianchuan.jinritemai.com/ad/")
            .unwrap();
        rules.extend(
            parser
                .parse_line("qianchuan.jinritemai.com localhost:8080")
                .unwrap(),
        );

        let resolver = RulesResolver::new(rules);

        // CONNECT phase: URL has no path — only domain-only rules match
        let ctx = RequestContext::from_url("https://qianchuan.jinritemai.com:443");
        let result = resolver.resolve(&ctx);

        let host_rules = result.get_by_protocol(Protocol::Host);
        assert_eq!(host_rules.len(), 1);
        assert_eq!(host_rules[0].resolved_value, "localhost:8080");
    }

    #[test]
    fn test_header_filter_not_stale_across_requests() {
        // Regression: the cache key must account for request headers when a rule
        // filters on them, so two requests differing only by header value resolve
        // independently instead of sharing a stale cached result.
        let rules = crate::rule::parser::parse_rules(
            "hc.test host://127.0.0.1:9 includeFilter://h:x-tag=match",
        )
        .unwrap();
        let resolver = RulesResolver::new(rules);

        let mut ctx_match = RequestContext::from_url("http://hc.test/");
        ctx_match.method = "GET".to_string();
        ctx_match
            .req_headers
            .insert("x-tag".to_string(), "match".to_string());
        assert_eq!(
            resolver.resolve(&ctx_match).rules.len(),
            1,
            "header value matches the filter -> rule applies"
        );

        // Same url|host|path|method, different header value -> filter must NOT match.
        let mut ctx_nomatch = RequestContext::from_url("http://hc.test/");
        ctx_nomatch.method = "GET".to_string();
        ctx_nomatch
            .req_headers
            .insert("x-tag".to_string(), "nope".to_string());
        assert_eq!(
            resolver.resolve(&ctx_nomatch).rules.len(),
            0,
            "different header value must not reuse the cached match"
        );

        // And the original matching request still resolves correctly afterwards.
        assert_eq!(resolver.resolve(&ctx_match).rules.len(), 1);
    }

    #[test]
    fn test_cache_key_fast_path_when_no_header_filters() {
        // Rule sets without header/cookie/IP filters keep the cheap cache key.
        let rules = crate::rule::parser::parse_rules("ex.test host://127.0.0.1:9").unwrap();
        let resolver = RulesResolver::new(rules);
        assert!(!resolver.cache_key_needs_request_ctx);
    }
}
