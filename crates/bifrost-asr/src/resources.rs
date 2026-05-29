use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechResourceKind {
    MlxAsrModel,
    StatefulVoiceWorker,
    OfflineAsrServer,
    SherpaDiarizationCpu,
    PyannoteSidecar,
    WakeListener,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLeaseRequest {
    pub owner_module: String,
    pub owner_id: String,
    pub kind: SpeechResourceKind,
    pub model_id: Option<String>,
    pub priority: u8,
    pub preemptible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLease {
    pub lease_id: String,
    pub owner_module: String,
    pub owner_id: String,
    pub kind: SpeechResourceKind,
    pub model_id: Option<String>,
    pub priority: u8,
    pub preemptible: bool,
    pub acquired_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLeaseDecision {
    pub granted: bool,
    pub lease: Option<ResourceLease>,
    pub reason: Option<String>,
    pub blocking_lease: Option<ResourceLease>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ResourceLeaseSnapshot {
    pub leases: Vec<ResourceLease>,
    pub realtime_voice_active: bool,
    pub offline_asr_active: bool,
}

#[derive(Debug, Default)]
pub struct ResourceLeaseManager {
    leases: BTreeMap<String, ResourceLease>,
}

impl ResourceLeaseManager {
    pub fn snapshot(&self) -> ResourceLeaseSnapshot {
        let leases = self.leases.values().cloned().collect::<Vec<_>>();
        ResourceLeaseSnapshot {
            realtime_voice_active: leases.iter().any(|lease| {
                matches!(
                    lease.kind,
                    SpeechResourceKind::StatefulVoiceWorker | SpeechResourceKind::WakeListener
                ) && lease.priority >= 90
            }),
            offline_asr_active: leases.iter().any(|lease| {
                matches!(
                    lease.kind,
                    SpeechResourceKind::MlxAsrModel | SpeechResourceKind::OfflineAsrServer
                ) && lease.owner_module != "realtime_voice"
            }),
            leases,
        }
    }

    pub fn acquire(&mut self, request: ResourceLeaseRequest, now_ms: u64) -> ResourceLeaseDecision {
        if let Some(blocking) = self.blocking_lease(&request) {
            return ResourceLeaseDecision {
                granted: false,
                lease: None,
                reason: Some(format!(
                    "resource {:?} is held by {}:{}",
                    blocking.kind, blocking.owner_module, blocking.owner_id
                )),
                blocking_lease: Some(blocking.clone()),
            };
        }
        let lease = ResourceLease {
            lease_id: format!(
                "{}:{}:{:?}",
                request.owner_module, request.owner_id, request.kind
            ),
            owner_module: request.owner_module,
            owner_id: request.owner_id,
            kind: request.kind,
            model_id: request.model_id,
            priority: request.priority,
            preemptible: request.preemptible,
            acquired_at_ms: now_ms,
        };
        self.leases.insert(lease.lease_id.clone(), lease.clone());
        ResourceLeaseDecision {
            granted: true,
            lease: Some(lease),
            reason: None,
            blocking_lease: None,
        }
    }

    pub fn release(&mut self, lease_id: &str) -> bool {
        self.leases.remove(lease_id).is_some()
    }

    pub fn release_owner(&mut self, owner_module: &str, owner_id: &str) {
        self.leases
            .retain(|_, lease| lease.owner_module != owner_module || lease.owner_id != owner_id);
    }

    pub fn should_yield_for_realtime(&self, owner_module: &str, owner_id: &str) -> bool {
        let snapshot = self.snapshot();
        snapshot.realtime_voice_active
            && !snapshot.leases.iter().any(|lease| {
                lease.owner_module == owner_module
                    && lease.owner_id == owner_id
                    && lease.priority >= 90
            })
    }

    fn blocking_lease(&self, request: &ResourceLeaseRequest) -> Option<&ResourceLease> {
        self.leases
            .values()
            .filter(|lease| resource_conflicts(lease.kind, request.kind))
            .filter(|lease| {
                lease.owner_module != request.owner_module || lease.owner_id != request.owner_id
            })
            .filter(|lease| lease.priority >= request.priority || !lease.preemptible)
            .max_by_key(|lease| lease.priority)
    }
}

fn resource_conflicts(left: SpeechResourceKind, right: SpeechResourceKind) -> bool {
    use SpeechResourceKind::*;
    matches!(
        (left, right),
        (MlxAsrModel, MlxAsrModel)
            | (MlxAsrModel, StatefulVoiceWorker)
            | (StatefulVoiceWorker, MlxAsrModel)
            | (StatefulVoiceWorker, StatefulVoiceWorker)
            | (OfflineAsrServer, StatefulVoiceWorker)
            | (StatefulVoiceWorker, OfflineAsrServer)
            | (OfflineAsrServer, OfflineAsrServer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(owner: &str, priority: u8, kind: SpeechResourceKind) -> ResourceLeaseRequest {
        ResourceLeaseRequest {
            owner_module: owner.to_string(),
            owner_id: "id".to_string(),
            kind,
            model_id: None,
            priority,
            preemptible: priority < 90,
        }
    }

    #[test]
    fn resource_manager_realtime_preempts_scheduled() {
        let mut manager = ResourceLeaseManager::default();
        assert!(
            manager
                .acquire(
                    request("scheduled_task", 20, SpeechResourceKind::OfflineAsrServer),
                    1
                )
                .granted
        );
        assert!(
            manager
                .acquire(
                    request(
                        "realtime_voice",
                        100,
                        SpeechResourceKind::StatefulVoiceWorker
                    ),
                    2
                )
                .granted
        );
        assert!(manager.should_yield_for_realtime("scheduled_task", "id"));
    }

    #[test]
    fn resource_manager_blocks_lower_priority_offline_when_realtime_active() {
        let mut manager = ResourceLeaseManager::default();
        assert!(
            manager
                .acquire(
                    request(
                        "realtime_voice",
                        100,
                        SpeechResourceKind::StatefulVoiceWorker
                    ),
                    1
                )
                .granted
        );
        let decision = manager.acquire(
            request("scheduled_task", 20, SpeechResourceKind::OfflineAsrServer),
            2,
        );
        assert!(!decision.granted);
        assert!(decision.reason.unwrap().contains("realtime_voice"));
    }
}
