//! Platform-independent macOS ownership journal and compare-before-write engine.
//! No OS calls live here; production and deterministic tests use the same engine.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use super::{ManagedProxyState, ManagedSystemProxyPhase, ProxyBackup};
use crate::{BifrostError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Field {
    Http,
    Https,
    Bypass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Protocol {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub authenticated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum Value {
    Protocol(Protocol),
    Bypass(Vec<String>),
}

impl Value {
    pub fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Protocol(a), Self::Protocol(b)) => a == b,
            (Self::Bypass(a), Self::Bypass(b)) => {
                // networksetup may normalize the order but not the meaning.
                let normalize = |v: &[String]| {
                    v.iter()
                        .map(|s| s.to_ascii_lowercase())
                        .collect::<std::collections::BTreeSet<_>>()
                };
                normalize(a) == normalize(b)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PendingWrite {
    pub before: Value,
    pub possible_after: Vec<Value>,
    /// A committed SCPreferences value is not evidence that Apply succeeded.
    #[serde(default)]
    pub needs_apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FieldOwnership {
    pub field: Field,
    pub before: Value,
    pub last_written: Value,
    #[serde(default)]
    pub pending: Option<PendingWrite>,
    #[serde(default)]
    pub relinquished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ServiceOwnership {
    pub name: String,
    pub fields: Vec<FieldOwnership>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Service {
    pub name: String,
    pub enabled: bool,
}

/// Each operation is one bounded OS transaction. Endpoint setters may
/// enable a proxy on some OS releases; both documented/read-back states are
/// journalled before the command, and the enable bit is corrected separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Operation {
    Endpoint {
        field: Field,
        host: String,
        port: u16,
    },
    Enabled {
        field: Field,
        enabled: bool,
    },
    Bypass(Vec<String>),
    /// Only disabled, unauthenticated HTTP(S); compare under the OS lock.
    DormantEndpoint {
        field: Field,
        expected: Protocol,
        desired: Protocol,
    },
}

pub(super) trait Backend {
    /// Whether this backend may invoke an authorization flow. A persisted
    /// cancellation forbids another elevated attempt until explicit user intent.
    fn requires_authorization(&self) -> bool {
        false
    }
    /// Older/fixture adapters retain the explicit incomplete-restore fallback.
    fn supports_dormant_restore(&self) -> bool {
        false
    }
    /// Include disabled services: an owned disabled service still needs cleanup.
    fn services(&mut self) -> Result<Vec<Service>>;
    fn read(&mut self, service: &str, field: Field) -> Result<Value>;
    fn write(&mut self, service: &str, operation: &Operation) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Intent {
    Apply,
    Suspend,
    Resume,
    Restore,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct TransitionResult {
    pub changed: bool,
    pub ownership_changed: bool,
    /// Routing is safe, but an empty/zero dormant endpoint was not restored.
    pub incomplete_baseline: bool,
}

pub(super) fn bypass_domains(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(super) fn capture(backend: &mut impl Backend) -> Result<Vec<ServiceOwnership>> {
    let services = backend.services()?;
    if !services.iter().any(|service| service.enabled) {
        return Err(BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".into(),
        ));
    }
    services
        .into_iter()
        .filter(|service| service.enabled)
        .map(|service| {
            let mut fields = Vec::new();
            for field in [Field::Http, Field::Https, Field::Bypass] {
                let before = backend.read(&service.name, field)?;
                // Passwords and enabled empty/zero endpoints cannot be faithfully
                // restored by this adapter. Preserve these existing owners; an
                // invalid enabled proxy may intentionally block direct traffic.
                let relinquished = matches!(&before, Value::Protocol(proxy)
                    if proxy.authenticated || (proxy.enabled && (proxy.host.is_empty() || proxy.port == 0)));
                fields.push(FieldOwnership {
                    field,
                    last_written: before.clone(),
                    before,
                    pending: None,
                    relinquished,
                });
            }
            if fields.iter().any(|field| field.relinquished) {
                // Bypass is shared by both protocols. Preserve the entire service
                // when one protocol cannot be faithfully restored.
                for field in &mut fields {
                    field.relinquished = true;
                }
            }
            Ok(ServiceOwnership {
                name: service.name,
                fields,
            })
        })
        .collect()
}

/// If state was lost while the Bifrost endpoint remained enabled, recording it
/// as the original would resurrect a dead local proxy at the next cleanup.
pub(super) fn sanitize_original_targets(services: &mut [ServiceOwnership], target: &ProxyBackup) {
    for field in services.iter_mut().flat_map(|service| &mut service.fields) {
        if let Value::Protocol(proxy) = &mut field.before {
            if proxy.port == target.port && super::proxy_hosts_match(&proxy.host, &target.host) {
                proxy.enabled = false;
            }
        }
    }
}

/// Legacy aggregate backups cannot tell us which service/protocol they describe.
/// Migrate conservatively: only disable endpoints that still match Bifrost;
/// never copy one service's corporate configuration onto another service.
pub(super) fn legacy_journal(
    os: &mut impl Backend,
    target: &ProxyBackup,
) -> Result<Vec<ServiceOwnership>> {
    os.services()?.into_iter().map(|service| {
        let mut fields = Vec::new();
        for field in [Field::Http, Field::Https, Field::Bypass] {
            let current = os.read(&service.name, field)?;
            let owned = matches!(&current, Value::Protocol(proxy)
                if !proxy.authenticated && proxy.port == target.port && super::proxy_hosts_match(&proxy.host, &target.host));
            let mut before = current.clone();
            if owned { if let Value::Protocol(proxy) = &mut before { proxy.enabled = false; } }
            fields.push(FieldOwnership { field, before, last_written: current, pending: None, relinquished: !owned });
        }
        Ok(ServiceOwnership { name: service.name, fields })
    }).collect()
}

pub(super) fn observed_owned(state: &ManagedProxyState, os: &mut impl Backend) -> Result<bool> {
    let services = os.services()?;
    let mut has_owner = false;
    for service in &state.macos_services {
        if !services.iter().any(|entry| entry.name == service.name) {
            return Ok(false);
        }
        for field in service.fields.iter().filter(|field| !field.relinquished) {
            if let Value::Protocol(proxy) = &field.last_written {
                has_owner = true;
                if state.phase() == ManagedSystemProxyPhase::Applied
                    && (!proxy.enabled
                        || proxy.port != state.target.port
                        || !super::proxy_hosts_match(&proxy.host, &state.target.host))
                {
                    return Ok(false);
                }
            }
            if field.pending.is_some()
                || !os
                    .read(&service.name, field.field)?
                    .equivalent(&field.last_written)
            {
                return Ok(false);
            }
        }
    }
    Ok(has_owner)
}

/// Start a distinct user-authorized attempt. Rotating the generation prevents
/// a delayed cancellation from an older dialog suppressing this new request.
pub(super) fn begin_explicit_acquisition(state: &mut ManagedProxyState) {
    state.authorization_suppressed = false;
    state.generation = uuid::Uuid::now_v7().to_string();
    state.set_phase(ManagedSystemProxyPhase::PendingApply);
}

/// A new explicit disable supersedes an earlier cancelled enable. This is
/// called only after the caller's accepted-intent predicate is checked.
pub(super) fn begin_explicit_disable(state: &mut ManagedProxyState) {
    state.authorization_suppressed = false;
    state.set_phase(ManagedSystemProxyPhase::Restoring);
}

/// A deliberate user acquisition can reclaim changed fields. Privilege retries
/// and automatic reconciliation must never call this function.
pub(super) fn refresh_explicit_acquisition(
    state: &mut ManagedProxyState,
    backend: &mut impl Backend,
) -> Result<()> {
    let snapshots = capture(backend)?;
    let mut renewed = false;
    for snapshot in snapshots {
        let Some(existing) = state
            .macos_services
            .iter_mut()
            .find(|service| service.name == snapshot.name)
        else {
            state.macos_services.push(snapshot);
            renewed = true;
            continue;
        };
        let authenticated = snapshot.fields.iter().any(|field| field.relinquished);
        for observed in snapshot.fields {
            let Some(field) = existing
                .fields
                .iter_mut()
                .find(|field| field.field == observed.field)
            else {
                existing.fields.push(observed);
                renewed = true;
                continue;
            };
            if authenticated {
                if !field.relinquished {
                    renewed = true;
                }
                field.relinquished = true;
            } else if field.relinquished || !field.last_written.equivalent(&observed.before) {
                *field = observed;
                renewed = true;
            }
        }
    }
    sanitize_original_targets(&mut state.macos_services, &state.target);
    if renewed {
        state.generation = uuid::Uuid::now_v7().to_string();
    }
    Ok(())
}

pub(super) fn has_active_owned_protocol(state: &ManagedProxyState) -> bool {
    state
        .macos_services
        .iter()
        .flat_map(|service| &service.fields)
        .any(|field| {
            !field.relinquished
                && field.pending.is_none()
                && matches!(&field.last_written,
            Value::Protocol(proxy) if proxy.enabled && proxy.port == state.target.port
                && super::proxy_hosts_match(&proxy.host, &state.target.host))
        })
}

fn desired(field: &FieldOwnership, target: &ProxyBackup, intent: Intent) -> Value {
    if matches!(intent, Intent::Suspend | Intent::Restore) {
        return field.before.clone();
    }
    match field.field {
        Field::Http | Field::Https => Value::Protocol(Protocol {
            enabled: true,
            host: target.host.clone(),
            port: target.port,
            authenticated: false,
        }),
        Field::Bypass => Value::Bypass(bypass_domains(&target.bypass)),
    }
}

fn next_operation(
    field: Field,
    current: &Value,
    desired: &Value,
    intent: Intent,
    supports_dormant: bool,
) -> Option<(Operation, Vec<Value>)> {
    match (current, desired) {
        (Value::Bypass(_), Value::Bypass(value)) if !current.equivalent(desired) => {
            Some((Operation::Bypass(value.clone()), vec![desired.clone()]))
        }
        (Value::Protocol(current), Value::Protocol(desired)) => {
            let unsupported_restore = matches!(intent, Intent::Suspend | Intent::Restore)
                && (desired.host.is_empty() || desired.port == 0);
            if unsupported_restore && desired.enabled {
                // Acquisition excludes these originals. A preexisting malformed
                // journal cannot authorize either direct-connect or re-enabling
                // a stale endpoint; report the unmatched baseline without writes.
                return None;
            }
            // Disable a dead target before attempting any baseline restoration.
            if !desired.enabled && current.enabled {
                let mut after = current.clone();
                after.enabled = false;
                return Some((
                    Operation::Enabled {
                        field,
                        enabled: false,
                    },
                    vec![Value::Protocol(after)],
                ));
            }
            if unsupported_restore && supports_dormant && current != desired {
                return Some((
                    Operation::DormantEndpoint {
                        field,
                        expected: current.clone(),
                        desired: desired.clone(),
                    },
                    vec![Value::Protocol(desired.clone())],
                ));
            }
            // Unsupported adapters retain the explicitly incomplete fallback.
            if (current.host != desired.host || current.port != desired.port)
                && !desired.host.is_empty()
                && desired.port != 0
            {
                let mut after = current.clone();
                after.host = desired.host.clone();
                after.port = desired.port;
                after.authenticated = false;
                let mut enabled_after = after.clone();
                enabled_after.enabled = true;
                return Some((
                    Operation::Endpoint {
                        field,
                        host: desired.host.clone(),
                        port: desired.port,
                    },
                    vec![Value::Protocol(after), Value::Protocol(enabled_after)],
                ));
            }
            if unsupported_restore {
                return None;
            }
            if current.enabled != desired.enabled {
                let mut after = current.clone();
                after.enabled = desired.enabled;
                return Some((
                    Operation::Enabled {
                        field,
                        enabled: desired.enabled,
                    },
                    vec![Value::Protocol(after)],
                ));
            }
            None
        }
        _ => None,
    }
}

/// The unsupported dormant restoration case is safe only with both flags off.
pub(super) fn incomplete_dormant_endpoint(current: &Value, original: &Value) -> bool {
    matches!((current, original), (Value::Protocol(current), Value::Protocol(original))
        if !current.enabled && !original.enabled
            && (original.host.is_empty() || original.port == 0)
            && (current.host != original.host || current.port != original.port))
}

fn reconcile(field: &mut FieldOwnership, current: &Value) -> bool {
    if let Some(pending) = field.pending.take() {
        if pending
            .possible_after
            .iter()
            .any(|after| current.equivalent(after))
        {
            field.last_written = current.clone();
            if pending.needs_apply {
                field.pending = Some(pending);
            }
        } else if !current.equivalent(&pending.before) {
            field.relinquished = true;
        }
    } else if !current.equivalent(&field.last_written) {
        field.relinquished = true;
    }
    !field.relinquished
}

/// All writes pass through this journal, including privilege retries. A failed
/// persistence, OS command, or read-back leaves durable evidence for recovery.
pub(super) fn transition(
    state: &mut ManagedProxyState,
    backend: &mut impl Backend,
    intent: Intent,
    mut persist: impl FnMut(&ManagedProxyState) -> Result<()>,
) -> Result<TransitionResult> {
    if state.authorization_suppressed && backend.requires_authorization() {
        return Err(BifrostError::Config("UserCancelled: Authorization is suppressed for this proxy generation until a new explicit enable request".into()));
    }
    let services = backend.services()?;
    if services.is_empty() {
        return Err(BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".into(),
        ));
    }
    let phase = match intent {
        Intent::Apply => ManagedSystemProxyPhase::PendingApply,
        Intent::Suspend => ManagedSystemProxyPhase::Suspending,
        Intent::Resume => ManagedSystemProxyPhase::Resuming,
        Intent::Restore => ManagedSystemProxyPhase::Restoring,
    };
    state.set_phase(phase);
    persist(state)?;
    let mut result = TransitionResult::default();
    let mut first_error = None;
    for service_index in 0..state.macos_services.len() {
        let service = state.macos_services[service_index].name.clone();
        // Removed/renamed services may reappear. Preserve the journal and retry;
        // disabled services are still present and always eligible for cleanup.
        if !services.iter().any(|entry| entry.name == service) {
            first_error.get_or_insert_with(|| {
                BifrostError::Config(format!("networksetup owned service unavailable: {service}"))
            });
            continue;
        }
        for field_index in 0..state.macos_services[service_index].fields.len() {
            let entry = &state.macos_services[service_index].fields[field_index];
            if entry.relinquished {
                result.ownership_changed = true;
                continue;
            }
            let field = entry.field;
            let desired = desired(entry, &state.target, intent);
            let attempt = (|| -> Result<()> {
                // Endpoint, enable and bypass operations each have their own
                // durable intent. Bound the loop against surprising OS behavior.
                for _ in 0..5 {
                    let current = backend.read(&service, field)?;
                    let entry = &mut state.macos_services[service_index].fields[field_index];
                    let retry_apply = entry.pending.as_ref().is_some_and(|pending| {
                        pending.needs_apply
                            && pending
                                .possible_after
                                .iter()
                                .any(|after| current.equivalent(after))
                    });
                    if !reconcile(entry, &current) {
                        result.ownership_changed = true;
                        persist(state)?;
                        return Ok(());
                    }
                    // Do not persist away the pending Apply barrier before its
                    // replacement is durable. A crash after commit must retry
                    // Apply even when networksetup already reads the new value.
                    let operation = if retry_apply {
                        match &current {
                            Value::Protocol(proxy) if backend.supports_dormant_restore() => Some((
                                Operation::DormantEndpoint {
                                    field,
                                    expected: proxy.clone(),
                                    desired: proxy.clone(),
                                },
                                vec![current.clone()],
                            )),
                            _ => {
                                return Err(BifrostError::Config(
                                    "Pending dormant proxy Apply requires a supported backend"
                                        .into(),
                                ))
                            }
                        }
                    } else {
                        persist(state)?;
                        next_operation(
                            field,
                            &current,
                            &desired,
                            intent,
                            backend.supports_dormant_restore(),
                        )
                    };
                    let Some((operation, possible_after)) = operation else {
                        if !current.equivalent(&desired) {
                            if matches!(intent, Intent::Suspend | Intent::Restore)
                                && incomplete_dormant_endpoint(&current, &desired)
                            {
                                result.incomplete_baseline = true;
                            } else {
                                return Err(BifrostError::Config(format!(
                                    "Proxy baseline is not representable for {service} {field:?}"
                                )));
                            }
                        }
                        return Ok(());
                    };
                    state.macos_services[service_index].fields[field_index].pending =
                        Some(PendingWrite {
                            before: current,
                            possible_after,
                            needs_apply: matches!(operation, Operation::DormantEndpoint { .. }),
                        });
                    persist(state)?;
                    // On a nonzero result (including zero-exit textual errors),
                    // do not assume no effect. Recovery reconciles the intent.
                    // Recheck after fsync, narrowing the race with System
                    // Settings/corporate agents (networksetup has no CAS API).
                    let before_write = backend.read(&service, field)?;
                    let entry = &mut state.macos_services[service_index].fields[field_index];
                    if !entry
                        .pending
                        .as_ref()
                        .is_some_and(|pending| before_write.equivalent(&pending.before))
                    {
                        reconcile(entry, &before_write);
                        result.ownership_changed |= entry.relinquished;
                        persist(state)?;
                        continue;
                    }
                    if let Err(error) = backend.write(&service, &operation) {
                        if matches!(operation, Operation::DormantEndpoint { .. })
                            && error.to_string().contains("ProxyOwnershipChanged:")
                        {
                            let entry =
                                &mut state.macos_services[service_index].fields[field_index];
                            entry.relinquished = true;
                            entry.pending = None;
                            result.ownership_changed = true;
                            persist(state)?;
                            return Ok(());
                        }
                        return Err(error);
                    }
                    result.changed = true;
                    let actual = backend.read(&service, field)?;
                    let entry = &mut state.macos_services[service_index].fields[field_index];
                    let confirmed = entry.pending.as_ref().is_some_and(|pending| {
                        pending
                            .possible_after
                            .iter()
                            .any(|value| actual.equivalent(value))
                    });
                    if !confirmed {
                        if !entry
                            .pending
                            .as_ref()
                            .is_some_and(|pending| actual.equivalent(&pending.before))
                        {
                            entry.relinquished = true;
                            result.ownership_changed = true;
                        }
                        persist(state)?;
                        return Err(BifrostError::Config(format!(
                            "networksetup write read-back mismatch for {service} {field:?}"
                        )));
                    }
                    entry.pending = None;
                    entry.last_written = actual;
                    persist(state)?;
                }
                Err(BifrostError::Config(format!(
                    "networksetup transition did not converge for {service} {field:?}"
                )))
            })();
            if let Err(error) = attempt {
                // A journal I/O failure is a hard barrier: no further OS
                // writes are allowed until durable storage works again.
                if error.to_string().contains("UserCancelled:") {
                    state.authorization_suppressed = true;
                    if let Err(persist_error) = persist(state) {
                        return Err(BifrostError::Config(format!(
                            "{error}; failed to persist authorization suppression: {persist_error}"
                        )));
                    }
                    return Err(error);
                }
                if matches!(error, BifrostError::Io(_)) {
                    return Err(error);
                }
                first_error.get_or_insert(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    state.set_phase(match intent {
        Intent::Apply | Intent::Resume => ManagedSystemProxyPhase::Applied,
        Intent::Suspend => ManagedSystemProxyPhase::Suspended,
        Intent::Restore => ManagedSystemProxyPhase::Restoring,
    });
    persist(state)?;
    Ok(result)
}

#[cfg(test)]
mod tests;
