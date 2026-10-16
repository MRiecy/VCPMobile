use dashmap::DashMap;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tokio::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Acquiring,
    Cancelled,
    Committed,
}

pub(super) struct Operation {
    pub cancelled: AtomicBool,
    pub status: Mutex<Status>,
}

impl Operation {
    pub fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) {
            Err("cancelled: 附件导入已取消".into())
        } else {
            Ok(())
        }
    }
}

fn operations() -> &'static DashMap<String, Arc<Operation>> {
    static OPERATIONS: OnceLock<DashMap<String, Arc<Operation>>> = OnceLock::new();
    OPERATIONS.get_or_init(DashMap::new)
}

pub(super) fn begin(id: &str) -> Result<Arc<Operation>, String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("无效的文件 operationId".into());
    }
    match operations().entry(id.to_owned()) {
        dashmap::mapref::entry::Entry::Occupied(_) => Err("文件操作已经存在".into()),
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            let operation = Arc::new(Operation {
                cancelled: AtomicBool::new(false),
                status: Mutex::new(Status::Acquiring),
            });
            entry.insert(operation.clone());
            Ok(operation)
        }
    }
}

pub(super) fn acquire(id: Option<String>) -> Result<(String, Arc<Operation>), String> {
    match id {
        Some(id) => {
            let operation = operations()
                .get(&id)
                .map(|entry| entry.clone())
                .ok_or_else(|| "文件操作不存在或已经结束".to_string())?;
            Ok((id, operation))
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let operation = begin(&id)?;
            Ok((id, operation))
        }
    }
}

pub(super) fn finish(id: &str) {
    operations().remove(id);
}

pub(super) struct FinishOnDrop(pub String);
impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        finish(&self.0);
    }
}

#[derive(Serialize)]
pub struct CancellationResult {
    pub status: &'static str,
}

pub async fn cancel_attachment_operation(operation_id: &str) -> CancellationResult {
    let Some(operation) = operations().get(operation_id).map(|entry| entry.clone()) else {
        return CancellationResult { status: "finished" };
    };
    let mut status = operation.status.lock().await;
    if *status == Status::Committed {
        return CancellationResult {
            status: "committed",
        };
    }
    *status = Status::Cancelled;
    operation.cancelled.store(true, Ordering::Release);
    CancellationResult {
        status: "cancelled",
    }
}

#[tauri::command]
pub fn begin_file_operation(operation_id: String) -> Result<(), String> {
    begin(&operation_id).map(|_| ())
}

#[tauri::command]
pub async fn cancel_attachment_import(operation_id: String) -> Result<CancellationResult, String> {
    Ok(cancel_attachment_operation(&operation_id).await)
}

#[tauri::command]
pub fn finish_file_operation(operation_id: String) {
    finish(&operation_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_and_commit_share_the_same_arbitration() {
        let id = uuid::Uuid::new_v4().to_string();
        let op = begin(&id).expect("begin");
        assert_eq!(cancel_attachment_operation(&id).await.status, "cancelled");
        assert!(op.check().is_err());
        finish(&id);
        let id = uuid::Uuid::new_v4().to_string();
        let op = begin(&id).expect("begin");
        *op.status.lock().await = Status::Committed;
        assert_eq!(cancel_attachment_operation(&id).await.status, "committed");
        assert!(op.check().is_ok());
        finish(&id);
    }

    #[tokio::test]
    async fn cancellation_during_commit_waits_and_does_not_revoke_published_data() {
        let id = uuid::Uuid::new_v4().to_string();
        let op = begin(&id).expect("begin");
        let mut transaction = op.status.lock().await;
        let cancel_id = id.clone();
        let cancelling = tokio::spawn(async move { cancel_attachment_operation(&cancel_id).await });
        tokio::task::yield_now().await;
        assert!(
            !cancelling.is_finished(),
            "cancel must wait for the existing commit decision"
        );
        *transaction = Status::Committed;
        drop(transaction);
        assert_eq!(cancelling.await.expect("cancel task").status, "committed");
        assert!(op.check().is_ok());
        finish(&id);
    }
}
