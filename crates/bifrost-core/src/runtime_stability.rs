use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePressureLevel {
    #[default]
    Normal = 0,
    Degraded = 1,
    Critical = 2,
}

impl ResourcePressureLevel {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Degraded,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

static RESOURCE_PRESSURE: AtomicU8 = AtomicU8::new(ResourcePressureLevel::Normal as u8);

pub fn current_resource_pressure() -> ResourcePressureLevel {
    ResourcePressureLevel::from_u8(RESOURCE_PRESSURE.load(Ordering::Acquire))
}

pub fn publish_resource_pressure(level: ResourcePressureLevel) -> ResourcePressureLevel {
    ResourcePressureLevel::from_u8(RESOURCE_PRESSURE.swap(level as u8, Ordering::AcqRel))
}

pub fn payload_persistence_allowed() -> bool {
    current_resource_pressure() == ResourcePressureLevel::Normal
}

pub fn heavy_tasks_allowed() -> bool {
    current_resource_pressure() == ResourcePressureLevel::Normal
}

pub fn scripts_allowed() -> bool {
    current_resource_pressure() == ResourcePressureLevel::Normal
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressureInputs {
    pub rss_bytes: u64,
    pub system_used_memory_bytes: u64,
    pub total_memory_bytes: u64,
    pub fd_count: u64,
    pub fd_limit: u64,
    pub active_connections: u64,
    pub connection_limit: u64,
    pub queue_depth: u64,
    pub queue_capacity: u64,
    pub scheduler_heartbeat_age_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressureThresholds {
    pub memory_degraded_ratio: f64,
    pub memory_critical_ratio: f64,
    pub fd_degraded_ratio: f64,
    pub fd_critical_ratio: f64,
    pub connection_degraded_ratio: f64,
    pub connection_critical_ratio: f64,
    pub queue_degraded_ratio: f64,
    pub queue_critical_ratio: f64,
    pub scheduler_degraded_ms: u64,
    pub scheduler_critical_ms: u64,
    pub normal_samples_to_recover: u8,
}

impl Default for PressureThresholds {
    fn default() -> Self {
        Self {
            memory_degraded_ratio: 0.70,
            memory_critical_ratio: 0.85,
            fd_degraded_ratio: 0.70,
            fd_critical_ratio: 0.90,
            connection_degraded_ratio: 0.80,
            connection_critical_ratio: 0.95,
            queue_degraded_ratio: 0.80,
            queue_critical_ratio: 0.95,
            scheduler_degraded_ms: 2_000,
            scheduler_critical_ms: 5_000,
            normal_samples_to_recover: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourcePressureController {
    thresholds: PressureThresholds,
    level: ResourcePressureLevel,
    consecutive_normal_samples: u8,
}

impl Default for ResourcePressureController {
    fn default() -> Self {
        Self::new(PressureThresholds::default())
    }
}

impl ResourcePressureController {
    pub fn new(thresholds: PressureThresholds) -> Self {
        Self {
            thresholds,
            level: ResourcePressureLevel::Normal,
            consecutive_normal_samples: 0,
        }
    }

    pub fn level(&self) -> ResourcePressureLevel {
        self.level
    }

    pub fn observe(&mut self, input: PressureInputs) -> ResourcePressureLevel {
        let observed = self.classify(input);
        if observed >= self.level {
            self.level = observed;
            self.consecutive_normal_samples = 0;
            return self.level;
        }

        if observed == ResourcePressureLevel::Normal {
            self.consecutive_normal_samples = self.consecutive_normal_samples.saturating_add(1);
            if self.consecutive_normal_samples >= self.thresholds.normal_samples_to_recover.max(1) {
                self.level = ResourcePressureLevel::Normal;
                self.consecutive_normal_samples = 0;
            }
        } else {
            self.level = observed;
            self.consecutive_normal_samples = 0;
        }
        self.level
    }

    fn classify(&self, input: PressureInputs) -> ResourcePressureLevel {
        let ratios = [
            ratio(input.system_used_memory_bytes, input.total_memory_bytes),
            ratio(input.fd_count, input.fd_limit),
            ratio(input.active_connections, input.connection_limit),
            ratio(input.queue_depth, input.queue_capacity),
        ];
        let critical_thresholds = [
            self.thresholds.memory_critical_ratio,
            self.thresholds.fd_critical_ratio,
            self.thresholds.connection_critical_ratio,
            self.thresholds.queue_critical_ratio,
        ];
        if input.scheduler_heartbeat_age_ms >= self.thresholds.scheduler_critical_ms
            || ratios
                .iter()
                .zip(critical_thresholds)
                .any(|(value, threshold)| value.is_some_and(|value| value >= threshold))
        {
            return ResourcePressureLevel::Critical;
        }

        let degraded_thresholds = [
            self.thresholds.memory_degraded_ratio,
            self.thresholds.fd_degraded_ratio,
            self.thresholds.connection_degraded_ratio,
            self.thresholds.queue_degraded_ratio,
        ];
        let degraded_signals = ratios
            .iter()
            .zip(degraded_thresholds)
            .filter(|(value, threshold)| value.is_some_and(|value| value >= *threshold))
            .count();
        if input.scheduler_heartbeat_age_ms >= self.thresholds.scheduler_degraded_ms
            || degraded_signals > 0
        {
            ResourcePressureLevel::Degraded
        } else {
            ResourcePressureLevel::Normal
        }
    }
}

fn ratio(value: u64, limit: u64) -> Option<f64> {
    (limit > 0).then(|| value as f64 / limit as f64)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeHealthSnapshot {
    pub pid: u32,
    pub scheduler_heartbeat_age_ms: u64,
    pub pressure: ResourcePressureLevel,
    pub rss_bytes: u64,
    pub cpu_percent: f32,
    pub fd_count: u64,
    pub fd_limit: u64,
    pub active_connections: u64,
    pub connection_limit: u64,
    pub queue_depth: u64,
    pub queue_capacity: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> PressureInputs {
        PressureInputs {
            rss_bytes: 10,
            system_used_memory_bytes: 10,
            total_memory_bytes: 100,
            fd_count: 10,
            fd_limit: 100,
            active_connections: 10,
            connection_limit: 100,
            queue_depth: 10,
            queue_capacity: 100,
            scheduler_heartbeat_age_ms: 100,
        }
    }

    #[test]
    fn pressure_uses_soft_and_hard_thresholds() {
        let mut controller = ResourcePressureController::default();
        let mut input = healthy();
        input.fd_count = 75;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Degraded);
        input.fd_count = 95;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Critical);
    }

    #[test]
    fn pressure_recovery_requires_consecutive_normal_samples() {
        let thresholds = PressureThresholds {
            normal_samples_to_recover: 2,
            ..PressureThresholds::default()
        };
        let mut controller = ResourcePressureController::new(thresholds);
        let mut input = healthy();
        input.queue_depth = 99;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Critical);
        assert_eq!(
            controller.observe(healthy()),
            ResourcePressureLevel::Critical
        );
        assert_eq!(controller.observe(healthy()), ResourcePressureLevel::Normal);
    }

    #[test]
    fn scheduler_lag_is_a_pressure_signal() {
        let mut controller = ResourcePressureController::default();
        let mut input = healthy();
        input.scheduler_heartbeat_age_ms = 5_000;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Critical);
    }

    #[test]
    fn system_memory_pressure_is_not_confused_with_process_rss() {
        let mut controller = ResourcePressureController::default();
        let mut input = healthy();
        input.rss_bytes = 1;
        input.system_used_memory_bytes = 90;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Critical);
    }

    #[test]
    fn published_levels_and_partial_recovery_cover_all_states() {
        let original = current_resource_pressure();
        publish_resource_pressure(ResourcePressureLevel::Degraded);
        assert_eq!(current_resource_pressure(), ResourcePressureLevel::Degraded);
        assert!(!payload_persistence_allowed());
        assert!(!heavy_tasks_allowed());
        assert!(!scripts_allowed());
        publish_resource_pressure(ResourcePressureLevel::Critical);
        assert_eq!(current_resource_pressure(), ResourcePressureLevel::Critical);

        let thresholds = PressureThresholds {
            normal_samples_to_recover: 3,
            ..PressureThresholds::default()
        };
        let mut controller = ResourcePressureController::new(thresholds);
        let mut input = healthy();
        input.fd_count = 95;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Critical);
        input.fd_count = 75;
        assert_eq!(controller.observe(input), ResourcePressureLevel::Degraded);
        assert_eq!(controller.level(), ResourcePressureLevel::Degraded);

        publish_resource_pressure(original);
    }
}
