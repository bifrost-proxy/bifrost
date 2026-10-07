//! Per-manager OS boundary. Unit-test fixtures retain one in-memory OS snapshot
//! across acquisition, retries, recovery and Drop instead of bypassing logic.
use super::macos_command::{NetworkSetup, Privilege};
use super::macos_owned::{Backend, Field, Operation, Service, Value};
use super::*;

pub(super) enum MacosBackend {
    Native(NetworkSetup),
    #[cfg(test)]
    Mock {
        state: SharedMockState,
        privilege: Privilege,
    },
}

impl MacosBackend {
    pub(super) fn native(privilege: Privilege) -> Self {
        Self::Native(NetworkSetup::new(privilege))
    }
}

impl Backend for MacosBackend {
    fn requires_authorization(&self) -> bool {
        match self {
            Self::Native(os) => os.requires_authorization(),
            #[cfg(test)]
            Self::Mock { privilege, .. } => !matches!(privilege, Privilege::Direct),
        }
    }
    fn services(&mut self) -> Result<Vec<Service>> {
        match self {
            Self::Native(os) => os.services(),
            #[cfg(test)]
            Self::Mock { state, .. } => with_mock(state, |os| Ok(os.services.clone())),
        }
    }
    fn read(&mut self, service: &str, field: Field) -> Result<Value> {
        match self {
            Self::Native(os) => os.read(service, field),
            #[cfg(test)]
            Self::Mock { state, .. } => with_mock(state, |os| {
                os.reads += 1;
                os.values
                    .get(&(service.into(), field as u8))
                    .cloned()
                    .ok_or_else(|| BifrostError::Config("Unknown mock macOS proxy field".into()))
            }),
        }
    }
    fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
        match self {
            Self::Native(os) => os.write(service, operation),
            #[cfg(test)]
            Self::Mock { state, .. } => with_mock(state, |os| os.write(service, operation)),
        }
    }
}

impl SystemProxyManager {
    pub(super) fn macos_backend(&self, privilege: Privilege) -> Result<MacosBackend> {
        #[cfg(test)]
        if self.skip_os_proxy_io {
            let mut state = self
                .mock_macos_state
                .lock()
                .map_err(|_| BifrostError::Config("Mock macOS proxy state poisoned".into()))?;
            if state.is_none() {
                let journal = match self.load_managed_state() {
                    Ok(state) => Some(state),
                    Err(BifrostError::Io(error))
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        None
                    }
                    Err(error) => return Err(error),
                };
                *state = Some(MockMacosState::from_journal(journal.as_ref()));
            }
            return Ok(MacosBackend::Mock {
                state: self.mock_macos_state.clone(),
                privilege,
            });
        }
        Ok(MacosBackend::native(privilege))
    }
}

#[cfg(test)]
pub(super) type SharedMockState = std::sync::Arc<std::sync::Mutex<Option<MockMacosState>>>;

#[cfg(test)]
fn with_mock<T>(
    state: &SharedMockState,
    operation: impl FnOnce(&mut MockMacosState) -> Result<T>,
) -> Result<T> {
    let mut state = state
        .lock()
        .map_err(|_| BifrostError::Config("Mock macOS proxy state poisoned".into()))?;
    operation(
        state.as_mut().ok_or_else(|| {
            BifrostError::Config("Mock macOS proxy state is uninitialized".into())
        })?,
    )
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(super) struct MockMacosState {
    pub services: Vec<Service>,
    pub values: std::collections::BTreeMap<(String, u8), Value>,
    pub writes: Vec<(String, Operation)>,
    pub reads: usize,
    pub next_write_error: Option<String>,
}

#[cfg(test)]
impl MockMacosState {
    pub(super) const SERVICE: &'static str = "Bifrost mock service";

    pub(super) fn from_journal(journal: Option<&ManagedProxyState>) -> Self {
        let mut state = Self {
            services: Vec::new(),
            values: Default::default(),
            writes: Vec::new(),
            reads: 0,
            next_write_error: None,
        };
        if let Some(journal) = journal.filter(|journal| !journal.macos_services.is_empty()) {
            for service in &journal.macos_services {
                state.services.push(Service {
                    name: service.name.clone(),
                    enabled: true,
                });
                for field in &service.fields {
                    state.values.insert(
                        (service.name.clone(), field.field as u8),
                        field.last_written.clone(),
                    );
                }
            }
        } else {
            let disabled = ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            };
            let proxy = journal
                .map(|journal| {
                    if journal.applied {
                        &journal.target
                    } else {
                        &journal.original
                    }
                })
                .unwrap_or(&disabled);
            state.services.push(Service {
                name: Self::SERVICE.into(),
                enabled: true,
            });
            for field in [Field::Http, Field::Https, Field::Bypass] {
                state.values.insert(
                    (Self::SERVICE.into(), field as u8),
                    mock_value(proxy, field),
                );
            }
        }
        state
    }

    fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
        self.writes.push((service.into(), operation.clone()));
        if let Some(error) = self.next_write_error.take() {
            return Err(BifrostError::Config(error));
        }
        match operation {
            Operation::Endpoint { field, host, port } => {
                let Some(Value::Protocol(proxy)) =
                    self.values.get_mut(&(service.into(), *field as u8))
                else {
                    return Err(BifrostError::Config("Invalid mock protocol".into()));
                };
                proxy.enabled = true;
                proxy.host = host.clone();
                proxy.port = *port;
                proxy.authenticated = false;
            }
            Operation::Enabled { field, enabled } => {
                let Some(Value::Protocol(proxy)) =
                    self.values.get_mut(&(service.into(), *field as u8))
                else {
                    return Err(BifrostError::Config("Invalid mock protocol".into()));
                };
                proxy.enabled = *enabled;
            }
            Operation::Bypass(domains) => {
                self.values.insert(
                    (service.into(), Field::Bypass as u8),
                    Value::Bypass(domains.clone()),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn mock_value(proxy: &ProxyBackup, field: Field) -> Value {
    match field {
        Field::Http | Field::Https => Value::Protocol(super::macos_owned::Protocol {
            enabled: proxy.enable,
            host: proxy.host.clone(),
            port: proxy.port,
            authenticated: false,
        }),
        Field::Bypass => Value::Bypass(super::macos_owned::bypass_domains(&proxy.bypass)),
    }
}

#[cfg(test)]
pub(super) fn mock_services_for_state(
    state: &ManagedProxyState,
) -> Vec<super::macos_owned::ServiceOwnership> {
    use super::macos_owned::{FieldOwnership, ServiceOwnership};
    let current = if state.applied {
        &state.target
    } else {
        &state.original
    };
    vec![ServiceOwnership {
        name: MockMacosState::SERVICE.into(),
        fields: [Field::Http, Field::Https, Field::Bypass]
            .into_iter()
            .map(|field| FieldOwnership {
                field,
                before: mock_value(&state.original, field),
                last_written: mock_value(current, field),
                pending: None,
                relinquished: false,
            })
            .collect(),
    }]
}

#[cfg(test)]
#[cfg(not(target_os = "macos"))]
impl SystemProxyManager {
    // Aggregate platform unit fixtures use the same in-memory OS as macOS
    // fixtures. Drop must not escape through Sysproxy's native registry calls.
    pub(super) fn restore_mock_aggregate(&mut self) -> Result<()> {
        debug_assert!(self.skip_os_proxy_io);
        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                self.detach_in_place();
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if state.macos_services.is_empty() {
            state.macos_services = mock_services_for_state(&state);
        }
        let mut os = self.macos_backend(Privilege::Direct)?;
        super::macos_owned::transition(
            &mut state,
            &mut os,
            super::macos_owned::Intent::Restore,
            |state| self.write_managed_state(state),
        )?;
        self.remove_state_files();
        self.detach_in_place();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
