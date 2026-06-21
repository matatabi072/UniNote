//! UniNote の tasks.json スキーマ。本体および他サイドカーと互換。
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

#[derive(Serialize, Deserialize, Clone)]
pub struct Item {
    pub id: String,
    /// 本体スキーマ上は googleTaskId だが、UniNote 本体は何のサービスかは知らない。
    /// Google Tasks の ID をここに格納する。
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

    /// 他アダプター（Todoist 等）が付与する独自フィールド。
    /// このアダプターは関与しないが、書き戻し時に保持する必要がある。
    #[serde(default, rename = "todoistId")]
    pub todoist_id: Option<String>,

    /// その他の未知フィールド（前方互換）
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}
