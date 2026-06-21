//! 同期状態（gtasks v1 PULL only 用、簡易版）。
//! sync/uninote-sync-gtasks.state.json に保存。
//!
//! 役割: ユーザーがローカルで削除したタスクが、次回 sync で remote から復活しないように
//! tombstones（墓標）を保持する。
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
}

fn default_version() -> u32 {
    1
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
