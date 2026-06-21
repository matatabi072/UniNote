//! Todoist 統合 API v1 クライアント。
//! 旧 REST v2（/rest/v2/tasks）は HTTP 410 を返すため使用しない。
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://api.todoist.com/api/v1";

#[derive(Deserialize, Debug, Clone)]
pub struct TodoistDue {
    /// "2026-09-15" 形式（日付のみ）
    #[serde(default)]
    pub date: Option<String>,
    /// "2026-09-15T09:00:00.000000Z" 形式（時刻あり、UTC）
    #[serde(default)]
    pub datetime: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TodoistTask {
    pub id: String,
    pub content: String,
    /// 完了状態。v1 では `checked` あるいは `completed_at != null` で判定するため両対応。
    #[serde(default, alias = "checked")]
    pub is_completed: bool,
    #[serde(default)]
    pub completed_at: Option<String>,
    /// 1 (normal/lowest) - 4 (highest) ※UI 表記とは逆順
    #[serde(default = "default_priority_value")]
    pub priority: i32,
    #[serde(default)]
    pub due: Option<TodoistDue>,
}

fn default_priority_value() -> i32 {
    1
}

/// v1 のページング応答。 旧 v2 互換のために生配列も受理する。
#[derive(Deserialize)]
#[serde(untagged)]
enum TasksResponse {
    Paged {
        results: Vec<TodoistTask>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
    Array(Vec<TodoistTask>),
}

pub struct Client {
    token: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(token: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(20))
            .build();
        Self { token, agent }
    }

    pub fn list_active_tasks(&self) -> Result<Vec<TodoistTask>, String> {
        let mut out: Vec<TodoistTask> = Vec::new();
        let mut cursor: Option<String> = None;
        // ページネーションを最大 50 回まで（暴走防止）
        for _ in 0..50 {
            let url = match &cursor {
                Some(c) => format!(
                    "{API_BASE}/tasks?limit=200&cursor={}",
                    urlencoded(c)
                ),
                None => format!("{API_BASE}/tasks?limit=200"),
            };
            let resp = self
                .agent
                .get(&url)
                .set("Authorization", &format!("Bearer {}", self.token))
                .call()
                .map_err(classify_error)?;
            if resp.status() != 200 {
                return Err(format!(
                    "予期しないステータス: {} {}",
                    resp.status(),
                    resp.status_text()
                ));
            }
            let parsed: TasksResponse = resp
                .into_json()
                .map_err(|e| format!("JSON 解析失敗: {e}"))?;
            match parsed {
                TasksResponse::Paged { mut results, next_cursor } => {
                    out.append(&mut results);
                    if let Some(c) = next_cursor {
                        cursor = Some(c);
                        continue;
                    }
                    break;
                }
                TasksResponse::Array(mut v) => {
                    out.append(&mut v);
                    break;
                }
            }
        }
        // completed_at から完了状態を補正
        for t in out.iter_mut() {
            if !t.is_completed && t.completed_at.is_some() {
                t.is_completed = true;
            }
        }
        Ok(out)
    }
}

/// POST /tasks (新規作成) / POST /tasks/{id} (更新) のリクエストボディ
#[derive(Serialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct TaskMutation {
    pub content: String,
    pub priority: i32,
    /// 時刻付き期限。RFC3339 UTC。例: "2026-06-25T15:00:00Z"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_datetime: Option<String>,
    /// 日付のみ期限。"YYYY-MM-DD"。due_datetime と同時指定不可
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
}

impl Client {
    /// 新規タスクを作成し、Todoist 側 ID を含めた完全データを返す
    pub fn create_task(&self, body: &TaskMutation) -> Result<TodoistTask, String> {
        let url = format!("{API_BASE}/tasks");
        if std::env::var("UNINOTE_SYNC_DEBUG").is_ok() {
            eprintln!(
                "[debug] POST {url} body={}",
                serde_json::to_string(body).unwrap_or_default()
            );
        }
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
            .map_err(classify_error)?;
        let status = resp.status();
        let text = resp.into_string().map_err(|e| format!("読込失敗: {e}"))?;
        if std::env::var("UNINOTE_SYNC_DEBUG").is_ok() {
            eprintln!("[debug] response {status}: {text}");
        }
        if !(200..300).contains(&status) {
            return Err(format!("create_task 失敗: {status} body={text}"));
        }
        serde_json::from_str::<TodoistTask>(&text)
            .map_err(|e| format!("JSON 解析失敗: {e}"))
    }

    /// 既存タスクのフィールドを更新（完了状態以外。完了は close_task/reopen_task で）
    pub fn update_task(&self, id: &str, body: &TaskMutation) -> Result<(), String> {
        let url = format!("{API_BASE}/tasks/{id}");
        if std::env::var("UNINOTE_SYNC_DEBUG").is_ok() {
            eprintln!(
                "[debug] POST {url} body={}",
                serde_json::to_string(body).unwrap_or_default()
            );
        }
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
            .map_err(classify_error)?;
        let status = resp.status();
        let text = resp.into_string().map_err(|e| format!("読込失敗: {e}"))?;
        if std::env::var("UNINOTE_SYNC_DEBUG").is_ok() {
            eprintln!("[debug] response {status}: {text}");
        }
        if !(200..300).contains(&status) {
            return Err(format!("update_task 失敗: {status} body={text}"));
        }
        Ok(())
    }

    /// タスクを完了状態にする
    pub fn close_task(&self, id: &str) -> Result<(), String> {
        let url = format!("{API_BASE}/tasks/{id}/close");
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Length", "0")
            .call()
            .map_err(classify_error)?;
        if !(200..300).contains(&resp.status()) {
            return Err(format!(
                "close_task 失敗: {} {}",
                resp.status(),
                resp.status_text()
            ));
        }
        Ok(())
    }

    /// タスクを削除する
    pub fn delete_task(&self, id: &str) -> Result<(), String> {
        let url = format!("{API_BASE}/tasks/{id}");
        let resp = self
            .agent
            .delete(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call();
        match resp {
            Ok(r) => {
                if (200..300).contains(&r.status()) {
                    Ok(())
                } else {
                    Err(format!("delete_task 失敗: {}", r.status()))
                }
            }
            // 既に存在しない (404) は冪等として成功扱い
            Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(classify_error(e)),
        }
    }

    /// 完了タスクをアクティブに戻す
    pub fn reopen_task(&self, id: &str) -> Result<(), String> {
        let url = format!("{API_BASE}/tasks/{id}/reopen");
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Length", "0")
            .call()
            .map_err(classify_error)?;
        if !(200..300).contains(&resp.status()) {
            return Err(format!(
                "reopen_task 失敗: {} {}",
                resp.status(),
                resp.status_text()
            ));
        }
        Ok(())
    }
}

/// 簡易 URL エンコーダ（カーソル文字列用、英数+ - _ . ~ のみそのまま）
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn classify_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => "認証失敗（トークンを確認してください）".to_string(),
        ureq::Error::Status(403, _) => "アクセス拒否".to_string(),
        ureq::Error::Status(429, _) => "レート制限。少し待って再実行してください".to_string(),
        ureq::Error::Status(code, r) => format!("HTTP {code}: {}", r.status_text()),
        ureq::Error::Transport(t) => format!("通信失敗: {t}"),
    }
}
