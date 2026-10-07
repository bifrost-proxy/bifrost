use super::*;

impl AdminState {
    pub fn with_rules_storage(mut self, storage: RulesStorage) -> Self {
        self.rules_storage = storage;
        self
    }

    pub fn with_values_storage(mut self, storage: ValuesStorage) -> Self {
        self.values_storage = Some(Arc::new(ParkingRwLock::new(storage)));
        self
    }

    pub fn with_auth_db(mut self, db: AuthDb) -> Self {
        self.auth_db = Some(Arc::new(db));
        self
    }

    pub fn with_traffic_db_store(mut self, store: TrafficDbStore) -> Self {
        self.traffic_db_store = Some(Arc::new(store));
        self
    }

    pub fn with_traffic_db_store_shared(mut self, store: SharedTrafficDbStore) -> Self {
        self.traffic_db_store = Some(store);
        self
    }

    pub fn with_metrics_collector(mut self, collector: MetricsCollector) -> Self {
        self.metrics_collector = Arc::new(collector);
        self
    }

    pub fn with_access_control(mut self, access_control: SharedAccessControl) -> Self {
        self.access_control = Some(access_control);
        self
    }

    pub fn with_body_store(mut self, body_store: SharedBodyStore) -> Self {
        self.body_store = Some(body_store);
        self
    }

    pub fn with_ws_payload_store(mut self, store: SharedWsPayloadStore) -> Self {
        self.ws_payload_store = Some(store);
        self
    }

    pub fn with_frame_store(mut self, frame_store: FrameStore) -> Self {
        self.frame_store = Some(Arc::new(frame_store));
        self
    }

    pub fn with_frame_store_shared(mut self, frame_store: SharedFrameStore) -> Self {
        self.frame_store = Some(frame_store);
        self
    }

    pub fn with_ca_cert_path(mut self, ca_cert_path: PathBuf) -> Self {
        self.ca_cert_path = Some(ca_cert_path);
        self
    }

    pub fn with_system_proxy_manager(mut self, manager: SystemProxyManager) -> Self {
        self.system_proxy_manager = Some(Arc::new(RwLock::new(manager)));
        self
    }

    pub fn with_system_proxy_manager_shared(mut self, manager: SharedSystemProxyManager) -> Self {
        self.system_proxy_manager = Some(manager);
        self
    }

    pub fn with_runtime_config(mut self, config: RuntimeConfig) -> Self {
        self.runtime_config = Arc::new(RwLock::new(config));
        self
    }

    pub fn with_runtime_config_shared(mut self, config: SharedRuntimeConfig) -> Self {
        self.runtime_config = config;
        self
    }

    pub fn with_connection_registry(mut self, registry: ConnectionRegistry) -> Self {
        self.connection_registry = Arc::new(registry);
        self
    }

    pub fn with_connection_registry_shared(mut self, registry: SharedConnectionRegistry) -> Self {
        self.connection_registry = registry;
        self
    }

    pub fn with_config_manager(mut self, manager: ConfigManager) -> Self {
        self.config_manager = Some(Arc::new(manager));
        self
    }

    pub fn with_config_manager_shared(mut self, manager: SharedConfigManager) -> Self {
        self.config_manager = Some(manager);
        self
    }

    pub fn with_system_proxy_lifecycle_helper_shared(
        mut self,
        helper: SharedSystemProxyLifecycleHelperState,
    ) -> Self {
        self.system_proxy_lifecycle_helper = Some(helper);
        self
    }

    pub fn with_tray_launch_callback(mut self, callback: SharedTrayLaunchCallback) -> Self {
        self.tray_launch_callback = Some(callback);
        self
    }

    pub fn request_tray_launch(&self) -> bool {
        if let Some(callback) = &self.tray_launch_callback {
            callback();
            true
        } else {
            false
        }
    }

    pub fn with_system_proxy_runtime_flags_shared(
        mut self,
        desired_enabled: SharedSystemProxyRuntimeFlag,
        enabled_flag: SharedSystemProxyRuntimeFlag,
    ) -> Self {
        self.system_proxy_desired_enabled = Some(desired_enabled);
        self.system_proxy_enabled_flag = Some(enabled_flag);
        self
    }

    pub fn set_system_proxy_runtime_desired_enabled(&self, enabled: bool) -> Option<bool> {
        self.system_proxy_desired_enabled
            .as_ref()
            .map(|flag| flag.swap(enabled, Ordering::AcqRel))
    }

    pub fn store_system_proxy_runtime_desired_enabled(&self, enabled: bool) {
        if let Some(flag) = &self.system_proxy_desired_enabled {
            flag.store(enabled, Ordering::Release);
        }
    }

    pub fn store_system_proxy_runtime_managed(&self, managed: bool) {
        if let Some(flag) = &self.system_proxy_enabled_flag {
            flag.store(managed, Ordering::Release);
        }
    }
}
