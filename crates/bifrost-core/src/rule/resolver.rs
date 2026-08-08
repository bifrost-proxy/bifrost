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
use super::{parse_rule_key_value_pairs, MemoryValueStore, ValueSource, ValueStore};

const DEFAULT_CACHE_CAPACITY: usize = 1000;

#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub rule: Rule,
    pub captures: Option<Vec<String>>,
    pub resolved_value: String,
    resolved_key_value_pairs: Option<Vec<(String, String)>>,
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
        } else if matches!(rule.protocol, crate::protocol::Protocol::Redirect) {
            match &rule.value_source {
                ValueSource::ParenContent(content) => content.clone(),
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
        // Key/value separators are syntax in the authored source, not in
        // template output. Parse first, then expand each field independently.
        let resolved_key_value_pairs = if matches!(
            rule.protocol,
            crate::protocol::Protocol::ReqHeaders
                | crate::protocol::Protocol::ResHeaders
                | crate::protocol::Protocol::ReqCookies
                | crate::protocol::Protocol::ResCookies
                | crate::protocol::Protocol::Trailers
        ) {
            parse_rule_key_value_pairs(&base_value, &rule.value_source).and_then(|pairs| {
                // Attribute-bearing response cookies are structured JSON and
                // must continue through the dedicated response-cookie parser.
                if rule.protocol == crate::protocol::Protocol::ResCookies
                    && response_cookie_json_has_attributes(&base_value)
                {
                    return None;
                }
                Some(
                    pairs
                        .into_iter()
                        .map(|(name, value)| {
                            (
                                TemplateEngine::expand_with_context(
                                    &name,
                                    ctx,
                                    captures.as_deref(),
                                    values,
                                ),
                                TemplateEngine::expand_with_context(
                                    &value,
                                    ctx,
                                    captures.as_deref(),
                                    values,
                                ),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
        } else {
            None
        };
        let resolved_value =
            TemplateEngine::expand_with_context(&base_value, ctx, captures.as_deref(), values);

        Self {
            rule,
            captures,
            resolved_value,
            resolved_key_value_pairs,
        }
    }

    /// Return request/response headers parsed before template expansion.
    ///
    /// Expanding each parsed name/value independently ensures an `&` produced by
    /// `${url}` or `${reqHeaders.*}` remains data and cannot become a separator.
    pub fn header_pairs(&self) -> Option<&[(String, String)]> {
        matches!(
            self.rule.protocol,
            crate::protocol::Protocol::ReqHeaders | crate::protocol::Protocol::ResHeaders
        )
        .then_some(self.resolved_key_value_pairs.as_deref())
        .flatten()
    }

    /// Return source-aware pairs for headers, cookies and trailers.
    pub fn key_value_pairs(&self) -> Option<&[(String, String)]> {
        self.resolved_key_value_pairs.as_deref()
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

fn response_cookie_json_has_attributes(value: &str) -> bool {
    let trimmed = value.trim();
    let content = if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .is_some_and(|value| {
            value
                .as_object()
                .is_some_and(|object| object.values().any(serde_json::Value::is_object))
        })
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateMatch {
    rule_index: usize,
    captures: Option<Vec<String>>,
    is_negated: bool,
}

struct LruCache {
    cache: LruCacheImpl<String, Vec<CandidateMatch>>,
}

impl LruCache {
    fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity)
            .unwrap_or(NonZeroUsize::new(DEFAULT_CACHE_CAPACITY).unwrap());
        Self {
            cache: LruCacheImpl::new(cap),
        }
    }

    fn peek(&self, key: &str) -> Option<Vec<CandidateMatch>> {
        self.cache.peek(key).cloned()
    }

    #[cfg(test)]
    fn get(&mut self, key: &str) -> Option<Vec<CandidateMatch>> {
        self.cache.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: Vec<CandidateMatch>) {
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
}

impl RulesResolver {
    pub fn new(rules: Vec<Rule>) -> Self {
        let mut sorted_rules = rules;
        sorted_rules.sort_by_key(|b| std::cmp::Reverse(b.priority()));

        Self {
            rules: sorted_rules,
            values: HashMap::new(),
            cache: Arc::new(RwLock::new(LruCache::new(DEFAULT_CACHE_CAPACITY))),
            cache_enabled: true,
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
            if rule.matcher.matches_host_scope(&url_https, host)
                || rule.matcher.matches_host_scope(&url_http, host)
            {
                return true;
            }
        }
        false
    }

    pub fn has_breakpoint_rules_for_host(&self, host: &str) -> bool {
        let url_https = format!("https://{}", host);
        let url_http = format!("http://{}", host);
        for rule in &self.rules {
            if rule.is_disabled()
                || rule.is_negated()
                || rule.protocol != Protocol::Breakpoint
                || !rule.matcher.can_trigger_tls_auto_intercept()
            {
                continue;
            }
            if rule.matcher.matches_host_scope(&url_https, host)
                || rule.matcher.matches_host_scope(&url_http, host)
            {
                return true;
            }
        }
        false
    }

    pub fn has_tls_auto_intercept_route_rules_for_host(&self, host: &str) -> bool {
        let url_https = format!("https://{}", host);
        let url_wss = format!("wss://{}", host);
        for rule in &self.rules {
            if rule.is_disabled() || rule.is_negated() {
                continue;
            }
            if !matches!(
                rule.protocol,
                Protocol::Host
                    | Protocol::XHost
                    | Protocol::Http
                    | Protocol::Https
                    | Protocol::Ws
                    | Protocol::Wss
            ) {
                continue;
            }
            if !rule.matcher.can_trigger_tls_auto_intercept() {
                continue;
            }
            if rule.matcher.matches_host_scope(&url_https, host)
                || rule.matcher.matches_host_scope(&url_wss, host)
            {
                return true;
            }
        }
        false
    }

    pub fn resolve(&self, ctx: &RequestContext) -> ResolvedRules {
        tracing::trace!(
            target: "bifrost_core::rules",
            total_rules = self.rules.len(),
            url = %ctx.url,
            method = %ctx.method,
            "starting rule resolution"
        );

        let candidates = self.get_matcher_candidates(ctx);
        let result = self.apply_filter_gate(&candidates, ctx);

        tracing::debug!(
            target: "bifrost_core::rules",
            url = %ctx.url,
            matched_count = result.rules.len(),
            "rule resolution completed"
        );

        result
    }

    /// Resolve without consulting or populating the candidate cache. Used for
    /// response-phase re-resolution after the upstream response has been attached to the
    /// context. Response data is evaluated by the filter gate below.
    pub fn resolve_uncached(&self, ctx: &RequestContext) -> ResolvedRules {
        tracing::trace!(
            target: "bifrost_core::rules",
            total_rules = self.rules.len(),
            url = %ctx.url,
            method = %ctx.method,
            "starting uncached rule resolution"
        );

        let candidates = self.collect_matcher_candidates(ctx);
        let result = self.apply_filter_gate(&candidates, ctx);

        tracing::debug!(
            target: "bifrost_core::rules",
            url = %ctx.url,
            matched_count = result.rules.len(),
            "uncached rule resolution completed"
        );

        result
    }

    fn get_matcher_candidates(&self, ctx: &RequestContext) -> Vec<CandidateMatch> {
        // The candidate cache intentionally excludes method and headers: those are
        // request-scoped filter inputs and must be evaluated in Phase B.
        let cache_key = format!("{}|{}|{}", ctx.url, ctx.host, ctx.path);

        if self.cache_enabled {
            if let Some(cached) = self.cache.read().peek(&cache_key) {
                tracing::trace!(
                    target: "bifrost_core::rules",
                    url = %ctx.url,
                    cached_candidates = cached.len(),
                    "returning cached matcher candidates"
                );
                return cached;
            }
        }

        let candidates = self.collect_matcher_candidates(ctx);

        if self.cache_enabled {
            self.cache.write().insert(cache_key, candidates.clone());
        }

        candidates
    }

    fn collect_matcher_candidates(&self, ctx: &RequestContext) -> Vec<CandidateMatch> {
        let mut candidates = Vec::new();

        for (rule_index, rule) in self.rules.iter().enumerate() {
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
                "rule matcher attempt"
            );

            if rule.is_negated() {
                if !match_result.matched {
                    candidates.push(CandidateMatch {
                        rule_index,
                        captures: None,
                        is_negated: true,
                    });
                }
                continue;
            }

            if match_result.matched {
                candidates.push(CandidateMatch {
                    rule_index,
                    captures: match_result.captures,
                    is_negated: false,
                });
            }
        }

        candidates
    }

    fn apply_filter_gate(
        &self,
        candidates: &[CandidateMatch],
        ctx: &RequestContext,
    ) -> ResolvedRules {
        let mut result = ResolvedRules::new();
        let mut matched_protocols: HashMap<Protocol, bool> = HashMap::new();
        let active_skips = self.collect_active_skips_from(candidates, ctx);

        for candidate in candidates {
            let Some(rule) = self.rules.get(candidate.rule_index) else {
                continue;
            };

            if rule.is_disabled() {
                continue;
            }

            if candidate.is_negated {
                matched_protocols.insert(rule.protocol, true);
                continue;
            }

            if !rule.protocol.is_multi_match() && matched_protocols.contains_key(&rule.protocol) {
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
                "rule matcher candidate matched"
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
                ResolvedRule::new(rule.clone(), candidate.captures.clone(), ctx, &self.values);
            result.add(resolved);

            tracing::debug!(
                target: "bifrost_core::rules",
                pattern = %rule.pattern,
                protocol = %rule.protocol.to_str(),
                value = %rule.value,
                raw = %rule.raw,
                file = rule.file.as_deref().unwrap_or("<unknown>"),
                line = rule.line.unwrap_or(0),
                "rule selected"
            );

            if !rule.protocol.is_multi_match() {
                matched_protocols.insert(rule.protocol, true);
            }
        }

        result
    }

    /// Response-dependent filters (status code and response headers) cannot be evaluated
    /// before the upstream response exists. They are skipped during request-phase
    /// resolution so routing/request operations on the same rule can still run, then
    /// evaluated during response-phase re-resolution once `set_response` has populated
    /// the context.
    fn filter_is_evaluable(filter: &Filter, ctx: &RequestContext) -> bool {
        match filter {
            Filter::StatusCode(_) => ctx.status_code.is_some(),
            Filter::HeaderMatch {
                is_request: false, ..
            } => ctx.res_headers.is_some(),
            _ => true,
        }
    }

    fn matches_all_filters(filters: &[Filter], ctx: &RequestContext) -> bool {
        filters
            .iter()
            .filter(|f| Self::filter_is_evaluable(f, ctx))
            .all(|f| Self::matches_filter(f, ctx))
    }

    fn matches_any_filter(filters: &[Filter], ctx: &RequestContext) -> bool {
        filters
            .iter()
            .filter(|f| Self::filter_is_evaluable(f, ctx))
            .any(|f| Self::matches_filter(f, ctx))
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

    fn collect_active_skips_from(
        &self,
        candidates: &[CandidateMatch],
        ctx: &RequestContext,
    ) -> Vec<SkipRule> {
        let mut active_skips = Vec::new();

        for candidate in candidates {
            if candidate.is_negated {
                continue;
            }

            let Some(rule) = self.rules.get(candidate.rule_index) else {
                continue;
            };

            if rule.is_disabled() {
                continue;
            }

            if rule.protocol != Protocol::Skip {
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
mod tests;
