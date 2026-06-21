use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 重要度（タスクモードで使用。メモは既定 none）
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    Medium,
    Low,
    None,
}

impl Priority {
    pub fn rank(self) -> u8 {
        match self {
            Priority::High => 3,
            Priority::Medium => 2,
            Priority::Low => 1,
            Priority::None => 0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Priority::High => "高",
            Priority::Medium => "中",
            Priority::Low => "低",
            Priority::None => "なし",
        }
    }
}

fn default_priority() -> Priority {
    Priority::None
}

/// メモの種類（テキスト / 画像）。既定は Text（既存 JSON 互換）。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    #[default]
    Text,
    Image,
}

/// タスク・メモ共通の1件データ。tasks.json / notes.json 両方と互換。
/// 画像メモでは task_content に "images/{uuid}.{ext}" の相対パスを格納する。
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
    /// メモの種類（テキスト/画像）。未指定は Text。
    #[serde(default)]
    pub kind: ItemKind,
    /// 画像メモのときの元ファイル名（表示用）
    #[serde(default, rename = "imageName")]
    pub image_name: Option<String>,
}

impl Item {
    pub fn new_task(content: String, order: i64) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            google_task_id: None,
            task_content: content,
            is_completed: false,
            scheduled_date_time: None,
            priority: Priority::None,
            manual_order: order,
            updated_at: Utc::now().to_rfc3339(),
            kind: ItemKind::Text,
            image_name: None,
        }
    }

    pub fn new_note(order: i64) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            google_task_id: None,
            task_content: String::new(),
            is_completed: false,
            scheduled_date_time: None,
            priority: Priority::None,
            manual_order: order,
            updated_at: Utc::now().to_rfc3339(),
            kind: ItemKind::Text,
            image_name: None,
        }
    }

    pub fn new_image(rel_path: String, original_name: String, order: i64) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            google_task_id: None,
            task_content: rel_path,
            is_completed: false,
            scheduled_date_time: None,
            priority: Priority::None,
            manual_order: order,
            updated_at: Utc::now().to_rfc3339(),
            kind: ItemKind::Image,
            image_name: Some(original_name),
        }
    }

    pub fn is_image(&self) -> bool {
        self.kind == ItemKind::Image
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now().to_rfc3339();
    }

    /// メモ一覧用タイトル（先頭の空でない行、最大28文字）
    pub fn title(&self) -> String {
        let first = self
            .task_content
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("");
        if first.is_empty() {
            "(無題)".to_string()
        } else {
            first.chars().take(28).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_json_fields() {
        let t = Item::new_task("会議".to_string(), 0);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"taskContent\":\"会議\""));
        assert!(json.contains("\"googleTaskId\":null"));
        assert!(json.contains("\"isCompleted\":false"));
        assert!(json.contains("\"scheduledDateTime\":null"));
        assert!(json.contains("\"priority\":\"none\""));
        assert!(json.contains("\"manualOrder\":0"));
    }

    #[test]
    fn note_json_compatible_with_task() {
        let n = Item::new_note(0);
        let json = serde_json::to_string(&n).unwrap();
        // tasks.json と同じフィールド構成
        assert!(json.contains("\"taskContent\":"));
        assert!(json.contains("\"googleTaskId\":null"));
    }

    #[test]
    fn reads_simpletask_json() {
        let sample = r#"[{
            "id":"x","googleTaskId":null,"taskContent":"買い物",
            "isCompleted":false,"scheduledDateTime":"2026-06-20T15:00:00",
            "priority":"high","manualOrder":2,"updatedAt":"2026-06-19T00:00:00Z"
        }]"#;
        let v: Vec<Item> = serde_json::from_str(sample).unwrap();
        assert_eq!(v[0].task_content, "買い物");
        assert_eq!(v[0].priority, Priority::High);
    }
}
