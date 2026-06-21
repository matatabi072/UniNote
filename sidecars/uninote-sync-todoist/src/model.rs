//! UniNote の tasks.json スキーマ（本体と一致させる）。
//! 本体が知らないフィールドを追加しても破壊しないため、独自フィールドも自由に足せる。
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    Medium,
    Low,
    None,
}

fn default_priority() -> Priority {
    Priority::None
}

fn default_kind() -> String {
    "text".to_string()
}

/// UniNote の Item（タスク/メモ共通）の Todoist サイドカー側ビュー。
/// 未知フィールドを保持するため、不明値は `extra` に入る。
#[derive(Serialize, Deserialize, Clone)]
pub struct Item {
    pub id: String,
    #[serde(rename = "googleTaskId", default)]
    pub google_task_id: Option<String>,
    #[serde(rename = "taskContent")]
    pub task_content: String,
    #[serde(rename = "isCompleted", default)]
    pub is_completed: bool,
    #[serde(rename = "scheduledDateTime", default)]
    pub scheduled_date_time: Option<NaiveDateTime>,
    #[serde(default = "default_priority")]
    pub priority: Priority,
    #[serde(rename = "manualOrder", default)]
    pub manual_order: i64,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default, rename = "imageName")]
    pub image_name: Option<String>,

    /// このアダプター固有: Todoist 側の task ID（"123456789"）
    #[serde(default, rename = "todoistId")]
    pub todoist_id: Option<String>,

    /// 上記以外の任意フィールド（他アダプターが追加したものなど）を保持する
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}
