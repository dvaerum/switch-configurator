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
        let draft = SwitchDraft {
            switch_id: switch_id.clone(),
            original: config.clone(),
            edited: config,
            created_at: chrono::Utc::now(),
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
    async fn test_draft_discard_nonexistent() {
        let store = DraftStore::new();
        assert!(!store.discard("nonexistent").await);
    }
}
