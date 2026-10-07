//! Incomplete dormant restoration is evidence, not an active proxy lease.
//! Archive before retiring ownership; only matching recorded fields may supply
//! an original baseline to a later, independently authorized acquisition.
use super::macos_owned::{
    incomplete_dormant_endpoint, Field, FieldOwnership, ServiceOwnership, TransitionResult,
};
use super::*;

const INCOMPLETE_RESTORES_FILE: &str = "system_proxy_incomplete_restores.json";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct IncompleteRestores {
    records: Vec<ManagedProxyState>,
}

impl IncompleteRestores {
    fn latest_field(&self, service: &str, field: Field) -> Option<&FieldOwnership> {
        // A disabled service can be absent from a newer acquisition. Use its
        // newest recorded evidence, never an older value after a newer conflict.
        self.records.iter().rev().find_map(|record| {
            record
                .macos_services
                .iter()
                .find(|entry| entry.name == service)
                .and_then(|entry| entry.fields.iter().find(|entry| entry.field == field))
        })
    }
}

impl SystemProxyManager {
    fn incomplete_restores_path(&self) -> PathBuf {
        self.data_dir.join(INCOMPLETE_RESTORES_FILE)
    }

    fn incomplete_restores(&self) -> Result<IncompleteRestores> {
        match std::fs::read(self.incomplete_restores_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                BifrostError::Config(format!(
                    "Invalid incomplete proxy restoration record: {error}"
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(IncompleteRestores::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn reuse_incomplete_restore_baselines(
        &self,
        captured: &mut [ServiceOwnership],
    ) -> Result<()> {
        let records = self.incomplete_restores()?;
        for service in captured {
            for field in &mut service.fields {
                let Some(old) = records.latest_field(&service.name, field.field) else {
                    continue;
                };
                // The archived record grants no write authority. Its baseline
                // is useful only while current per-field evidence still agrees.
                if !field.relinquished
                    && !old.relinquished
                    && old.pending.is_none()
                    && field.last_written.equivalent(&old.last_written)
                    && incomplete_dormant_endpoint(&old.last_written, &old.before)
                {
                    field.before = old.before.clone();
                }
            }
        }
        Ok(())
    }

    pub(super) fn relinquish_archived_fields(&self, fields: &mut [ServiceOwnership]) -> Result<()> {
        let records = self.incomplete_restores()?;
        for service in fields {
            for field in &mut service.fields {
                if records.latest_field(&service.name, field.field).is_some() {
                    // Retirement ends this field's ownership. A stale runtime
                    // marker must not turn a later manual re-enable into a new
                    // legacy lease. Only a fresh acquisition can claim it again.
                    field.relinquished = true;
                }
            }
        }
        Ok(())
    }

    pub(super) fn finish_macos_restore(
        &mut self,
        state: &ManagedProxyState,
        outcome: TransitionResult,
    ) -> Result<SystemProxyDisableOutcome> {
        if outcome.incomplete_baseline {
            let mut archive = self.incomplete_restores()?;
            let unchanged_evidence = archive.records.last().is_some_and(|previous| {
                previous.original == state.original
                    && previous.target == state.target
                    && previous.macos_services == state.macos_services
            });
            if unchanged_evidence {
                // Repeated explicit cycles may leave identical recovery evidence.
                // Keep its newest generation without multiplying identical records.
                *archive
                    .records
                    .last_mut()
                    .expect("checked nonempty archive") = state.clone();
            } else if let Some(previous) = archive
                .records
                .iter_mut()
                .find(|s| s.generation == state.generation)
            {
                // An interrupted retirement may retry the same generation.
                *previous = state.clone();
            } else {
                archive.records.push(state.clone());
            }
            let bytes = serde_json::to_vec_pretty(&archive).map_err(|error| {
                BifrostError::Config(format!(
                    "Cannot serialize incomplete proxy restoration: {error}"
                ))
            })?;
            persistence::atomic_write(&self.incomplete_restores_path(), &bytes)?;
        }
        // Never retire the authoritative journal until its incomplete original
        // snapshots have another crash-durable home. Archives are not returned
        // by read_managed_ownership and cannot authorize automatic retries.
        for path in [self.backup_file_path(), self.state_file_path()] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if self.data_dir.exists() {
            persistence::sync_directory(&self.data_dir)?;
        }
        self.detach_in_place();
        if outcome.incomplete_baseline {
            return Err(BifrostError::Config(format!(
                "IncompleteRestore: owned empty/zero dormant proxy fields are disabled, but exact endpoint restoration is incomplete; original snapshots retained in {}",
                self.incomplete_restores_path().display()
            )));
        }
        Ok(if outcome.changed {
            SystemProxyDisableOutcome::Disabled
        } else if outcome.ownership_changed {
            SystemProxyDisableOutcome::OwnedByOther
        } else {
            SystemProxyDisableOutcome::NotEnabled
        })
    }
}

#[cfg(test)]
mod tests;
