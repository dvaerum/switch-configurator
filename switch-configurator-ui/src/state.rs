use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use switch_configurator::models::SwitchConfig;

#[derive(Debug, Clone)]
pub struct SwitchDraft {
    pub switch_id: String,
    pub original: SwitchConfig,
    pub edited: SwitchConfig,
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Which file each VLAN/port row came from — only populated for a draft
    /// seeded from a switch that failed validation (`merge-preview`, not
    /// `desired-config`). Empty for an ordinary healthy-switch draft, since
    /// there's nothing to attribute. `None` in a row's own lookup means "the
    /// main config" (never editable); `Some(path)` means a genuine overlay.
    pub vlan_sources: HashMap<u16, String>,
    pub port_sources: HashMap<String, String>,
}

#[derive(Clone)]
pub struct DraftStore {
    drafts: Arc<RwLock<HashMap<String, SwitchDraft>>>,
}

impl DraftStore {
    pub fn new() -> Self {
        Self {
            drafts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get(&self, switch_id: &str) -> Option<SwitchDraft> {
        self.drafts.read().await.get(switch_id).cloned()
    }

    pub async fn create(&self, switch_id: String, config: SwitchConfig) -> SwitchDraft {
        self.create_with_sources(switch_id, config, HashMap::new(), HashMap::new()).await
    }

    /// Same as `create`, but records which file each VLAN/port row came
    /// from — used when the draft is seeded from a switch that failed
    /// validation (`merge-preview`), so the editor can render the main
    /// config's rows read-only and annotate cross-file name collisions.
    pub async fn create_with_sources(
        &self,
        switch_id: String,
        config: SwitchConfig,
        vlan_sources: HashMap<u16, String>,
        port_sources: HashMap<String, String>,
    ) -> SwitchDraft {
        let draft = SwitchDraft {
            switch_id: switch_id.clone(),
            original: config.clone(),
            edited: config,
            created_at: chrono::Utc::now(),
            vlan_sources,
            port_sources,
        };
        self.drafts.write().await.insert(switch_id, draft.clone());
        draft
    }

    pub async fn update(&self, switch_id: &str, config: SwitchConfig) -> Option<SwitchDraft> {
        let mut drafts = self.drafts.write().await;
        if let Some(draft) = drafts.get_mut(switch_id) {
            draft.edited = config;
            Some(draft.clone())
        } else {
            None
        }
    }

    pub async fn discard(&self, switch_id: &str) -> bool {
        self.drafts.write().await.remove(switch_id).is_some()
    }

    pub async fn has_draft(&self, switch_id: &str) -> bool {
        self.drafts.read().await.contains_key(switch_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use switch_configurator::models::*;

    fn test_config() -> SwitchConfig {
        SwitchConfig {
            id: "sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: None,
            vlans: vec![Vlan {
                id: 10,
                name: "test".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            }],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: switch_configurator::config::Settings::default(),
        }
    }

    #[tokio::test]
    async fn test_draft_create_and_get() {
        let store = DraftStore::new();
        let config = test_config();

        let draft = store.create("sw-01".to_string(), config.clone()).await;
        assert_eq!(draft.switch_id, "sw-01");
        assert_eq!(draft.edited.vlans.len(), 1);

        let retrieved = store.get("sw-01").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().switch_id, "sw-01");
    }

    #[tokio::test]
    async fn test_draft_update() {
        let store = DraftStore::new();
        let mut config = test_config();
        store.create("sw-01".to_string(), config.clone()).await;

        config.vlans.push(Vlan {
            id: 20,
            name: "new-vlan".to_string(),
            description: None,
            ip_config: VlanIpConfig::None,
        });

        let updated = store.update("sw-01", config).await;
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().edited.vlans.len(), 2);
    }

    #[tokio::test]
    async fn test_draft_discard() {
        let store = DraftStore::new();
        store.create("sw-01".to_string(), test_config()).await;
        assert!(store.has_draft("sw-01").await);

        let removed = store.discard("sw-01").await;
        assert!(removed);
        assert!(!store.has_draft("sw-01").await);
    }

    #[tokio::test]
    async fn test_draft_create_with_sources_records_attribution() {
        let store = DraftStore::new();
        let mut vlan_sources = HashMap::new();
        vlan_sources.insert(10u16, "/etc/main.yaml".to_string());
        vlan_sources.insert(99u16, "/etc/switch-configurator/overlay.yaml".to_string());

        let draft = store.create_with_sources(
            "sw-01".to_string(),
            test_config(),
            vlan_sources,
            HashMap::new(),
        ).await;

        assert_eq!(draft.vlan_sources.get(&10).map(String::as_str), Some("/etc/main.yaml"));
        assert_eq!(draft.vlan_sources.get(&99).map(String::as_str), Some("/etc/switch-configurator/overlay.yaml"));
    }

    #[tokio::test]
    async fn test_draft_create_defaults_to_empty_sources() {
        // An ordinary healthy-switch draft (created via `create`, not
        // `create_with_sources`) has nothing to attribute.
        let store = DraftStore::new();
        let draft = store.create("sw-01".to_string(), test_config()).await;
        assert!(draft.vlan_sources.is_empty());
        assert!(draft.port_sources.is_empty());
    }

    #[tokio::test]
    async fn test_draft_discard_nonexistent() {
        let store = DraftStore::new();
        assert!(!store.discard("nonexistent").await);
    }
}
