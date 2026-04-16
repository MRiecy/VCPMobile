use crate::vcp_modules::sync_types::{DiffResult, EntityState};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum DiffAction {
    Pull,
    Push,
    Delete { deleted_at: i64 },
    PushDelete { deleted_at: i64 },
    Skip,
}

#[derive(Debug, Clone)]
pub struct ManifestDiff {
    pub actions: Vec<(String, DiffAction)>,
}

impl ManifestDiff {
    pub fn compute(local_items: &[EntityState], remote_items: &[EntityState]) -> Self {
        let local_map: HashMap<&str, &EntityState> =
            local_items.iter().map(|e| (e.id.as_str(), e)).collect();

        let remote_map: HashMap<&str, &EntityState> =
            remote_items.iter().map(|e| (e.id.as_str(), e)).collect();

        let mut actions = Vec::new();
        let mut processed_ids = std::collections::HashSet::new();

        for remote in remote_items {
            let id = remote.id.as_str();
            processed_ids.insert(id.to_string());

            if let Some(local) = local_map.get(id) {
                let diff = DiffResult::compute(local, remote);
                let action = match diff {
                    DiffResult::Skip => DiffAction::Skip,
                    DiffResult::Pull => DiffAction::Pull,
                    DiffResult::Push => DiffAction::Push,
                    DiffResult::Arbitrated { action } => match action {
                        crate::vcp_modules::sync_types::ArbitratedAction::Pull => DiffAction::Pull,
                        crate::vcp_modules::sync_types::ArbitratedAction::Push => DiffAction::Push,
                    },
                };
                actions.push((id.to_string(), action));
            } else {
                actions.push((id.to_string(), DiffAction::Pull));
            }
        }

        for local in local_items {
            let id = local.id.as_str();
            if !processed_ids.contains(id) {
                if let Some(_remote) = remote_map.get(id) {
                } else {
                    actions.push((id.to_string(), DiffAction::Push));
                }
            }
        }

        Self { actions }
    }

    pub fn compute_with_deletion(
        local_items: &[EntityState],
        remote_items: &[EntityState],
        local_deleted: &HashMap<String, i64>,
        remote_deleted: &HashMap<String, i64>,
    ) -> Self {
        let local_map: HashMap<&str, &EntityState> =
            local_items.iter().map(|e| (e.id.as_str(), e)).collect();

        let mut actions = Vec::new();
        let mut processed_ids = std::collections::HashSet::new();

        for remote in remote_items {
            let id = remote.id.as_str();
            processed_ids.insert(id.to_string());

            if let Some(&deleted_at) = remote_deleted.get(id) {
                if !local_deleted.contains_key(id) {
                    actions.push((id.to_string(), DiffAction::Delete { deleted_at }));
                }
                continue;
            }

            if let Some(local) = local_map.get(id) {
                if let Some(&deleted_at) = local_deleted.get(id) {
                    actions.push((id.to_string(), DiffAction::PushDelete { deleted_at }));
                    continue;
                }

                let diff = DiffResult::compute(local, remote);
                let action = match diff {
                    DiffResult::Skip => DiffAction::Skip,
                    DiffResult::Pull => DiffAction::Pull,
                    DiffResult::Push => DiffAction::Push,
                    DiffResult::Arbitrated { action } => match action {
                        crate::vcp_modules::sync_types::ArbitratedAction::Pull => DiffAction::Pull,
                        crate::vcp_modules::sync_types::ArbitratedAction::Push => DiffAction::Push,
                    },
                };
                actions.push((id.to_string(), action));
            } else {
                actions.push((id.to_string(), DiffAction::Pull));
            }
        }

        for local in local_items {
            let id = local.id.as_str();
            if !processed_ids.contains(id) {
                if let Some(&deleted_at) = local_deleted.get(id) {
                    if !remote_deleted.contains_key(id) {
                        actions.push((id.to_string(), DiffAction::PushDelete { deleted_at }));
                    }
                } else {
                    actions.push((id.to_string(), DiffAction::Push));
                }
            }
        }

        Self { actions }
    }

    pub fn get_pull_ids(&self) -> Vec<String> {
        self.actions
            .iter()
            .filter(|(_, a)| matches!(a, DiffAction::Pull))
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn get_push_ids(&self) -> Vec<String> {
        self.actions
            .iter()
            .filter(|(_, a)| matches!(a, DiffAction::Push))
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn get_delete_ids(&self) -> Vec<(String, i64)> {
        self.actions
            .iter()
            .filter_map(|(id, a)| {
                if let DiffAction::Delete { deleted_at } = a {
                    Some((id.clone(), *deleted_at))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_push_delete_ids(&self) -> Vec<(String, i64)> {
        self.actions
            .iter()
            .filter_map(|(id, a)| {
                if let DiffAction::PushDelete { deleted_at } = a {
                    Some((id.clone(), *deleted_at))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn skip_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|(_, a)| matches!(a, DiffAction::Skip))
            .count()
    }

    pub fn to_json(&self) -> serde_json::Value {
        let results: Vec<serde_json::Value> = self
            .actions
            .iter()
            .map(|(id, action)| {
                let action_str = match action {
                    DiffAction::Pull => "PULL",
                    DiffAction::Push => "PUSH",
                    DiffAction::Delete { deleted_at } => "DELETE",
                    DiffAction::PushDelete { deleted_at } => "PUSH_DELETE",
                    DiffAction::Skip => "SKIP",
                };

                let mut obj = serde_json::json!({
                    "id": id,
                    "action": action_str,
                });

                if let DiffAction::Delete { deleted_at } = action {
                    obj["deletedAt"] = serde_json::json!(deleted_at);
                } else if let DiffAction::PushDelete { deleted_at } = action {
                    obj["deletedAt"] = serde_json::json!(deleted_at);
                }

                obj
            })
            .collect();

        serde_json::json!(results)
    }
}
