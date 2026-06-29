//! 同期状態（gtasks v2 双方向用）。
//! sync/uninote-sync-gtasks.state.json に保存。
//!
//! 役割:
//!   - tombstones: ローカル削除されたタスクが remote から復活しないよう抑止
//!   - synced_tasks: 3-way merge 用の前回同期スナップショット
//!     （ローカル変更 / リモート変更 / 両方変更 を判別する基準）
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

const STATE_FILE: &str = "uninote-sync-gtasks.state.json";

#[derive(Serialize, Deserialize, Default)]
pub struct SyncState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    /// 前回 sync 終了時にローカルに存在していた googleTaskId 一覧
    #[serde(default)]
    pub last_known_ids: BTreeSet<String>,
    /// ローカル削除されたとみなす googleTaskId（再追加防止）
    #[serde(default)]
    pub tombstones: BTreeSet<String>,
    /// 3-way merge 用スナップショット (google_task_id → 前回同期時の状態)
    #[serde(default)]
    pub synced_tasks: BTreeMap<String, TaskSnapshot>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub title: String,
    #[serde(default)]
    pub is_completed: bool,
    #[serde(default)]
    pub scheduled_date_time: Option<NaiveDateTime>,
}

fn default_version() -> u32 {
    2
}

fn state_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let d = cwd.join("sync");
    let _ = fs::create_dir_all(&d);
    d.join(STATE_FILE)
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
