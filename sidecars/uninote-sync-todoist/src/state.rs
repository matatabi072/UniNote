//! 同期状態スナップショット（3-way merge の基準）。
//! sync/uninote-sync-todoist.state.json に保存。
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default)]
pub struct SyncState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    /// todoist_id → 前回同期時のスナップショット
    #[serde(default)]
    pub synced_tasks: BTreeMap<String, TaskSnapshot>,
}

fn default_version() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub content: String,
    #[serde(default)]
    pub is_completed: bool,
    /// "high" / "medium" / "low" / "none"
    pub priority: String,
    #[serde(default)]
    pub scheduled_date_time: Option<NaiveDateTime>,
}

fn state_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let d = cwd.join("sync");
    let _ = fs::create_dir_all(&d);
    d.join("uninote-sync-todoist.state.json")
}

pub fn load() -> SyncState {
    match fs::read_to_string(state_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => SyncState::default(),
    }
}

pub fn save(state: &SyncState) -> Result<(), String> {
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(state_path(), json).map_err(|e| format!("state 保存失敗: {e}"))
}
