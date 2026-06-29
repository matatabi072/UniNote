//! Google Tasks API v1 クライアント。
//! 必要に応じて access_token の自動 refresh を行う。
use crate::creds_store::ClientCredentials;
use crate::oauth;
use crate::token_store::{self, Tokens};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const API_BASE: &str = "https://tasks.googleapis.com/tasks/v1";

#[derive(Deserialize, Debug, Clone)]
pub struct GTask {
    pub id: String,
    pub title: String,
    /// "needsAction" | "completed"
    pub status: String,
    /// RFC3339 日付（時刻は常に 00:00:00.000Z で無視される）
    #[serde(default)]
    pub due: Option<String>,
    /// 完了タイムスタンプ（completed のみ）
    #[serde(default)]
    pub completed: Option<String>,
    /// 最終更新タイムスタンプ
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Deserialize)]
struct TasksList {
    #[serde(default)]
    items: Vec<GTask>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// POST /tasks（create）/ PATCH /tasks/{id}（update）共通ボディ。
/// PATCH では指定したフィールドだけ更新されるので、変更がない値は省略可能。
#[derive(Serialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct TaskMutation {
    pub title: String,
    /// "needsAction" / "completed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 期限。RFC3339 UTC（時刻は無視され日付のみ保存される）。例: "2026-06-25T00:00:00.000Z"
    /// null を送ると Google 側で「期限なし」を意味する（Option::None でフィールド省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

pub struct Client {
    creds: ClientCredentials,
    tokens: Tokens,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(creds: ClientCredentials, tokens: Tokens) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(20))
            .build();
        Self { creds, tokens, agent }
    }

    /// access_token が期限切れなら refresh して保存する。
    /// refresh_token が失効していた場合はトークンファイルを削除し、再認可を促す。
    fn ensure_token(&mut self) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if self.tokens.expires_at <= now {
            println!("[info] access_token が期限切れ → refresh");
            match oauth::refresh_access_token(&self.creds, &self.tokens) {
                Ok(new_tokens) => {
                    token_store::save(&new_tokens)?;
                    self.tokens = new_tokens;
                    Ok(())
                }
                Err(e) => {
                    // refresh_token が失効/取消された場合は、無効トークンを掃除して再認可を促す
                    let is_invalid_grant = e.contains("invalid_grant")
                        || e.contains("400")
                        || e.contains("401");
                    if is_invalid_grant {
                        let _ = token_store::clear();
                        Err(format!(
                            "認可が失効しました（refresh_token 無効）。\
                             保存済みトークンを削除しました。\
                             `--auth` で再認可してください: {e}"
                        ))
                    } else {
                        Err(e)
                    }
                }
            }
        } else {
            Ok(())
        }
    }

    /// 新規タスク作成 / 既存タスク更新の共通リクエストボディ
    /// Google Tasks の特性:
    ///   - 優先度の概念なし
    ///   - due は RFC3339 UTC（時刻部分は無視され日付のみ保存）
    ///   - status: "needsAction" | "completed"
    pub fn create_task(&mut self, body: &TaskMutation) -> Result<GTask, String> {
        self.ensure_token()?;
        let url = format!("{API_BASE}/lists/@default/tasks");
        if std::env::var("UNINOTE_SYNC_DEBUG").is_ok() {
            eprintln!(
                "[debug] POST {url} body={}",
                serde_json::to_string(body).unwrap_or_default()
            );
        }
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.tokens.access_token))
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
        serde_json::from_str::<GTask>(&text).map_err(|e| format!("JSON 解析失敗: {e}"))
    }

    /// 既存タスク更新（PATCH。Google Tasks API は PATCH/PUT 両対応だが PATCH のほうが
    /// 必要なフィールドだけ送れて安全）。
    pub fn update_task(&mut self, id: &str, body: &TaskMutation) -> Result<(), String> {
        self.ensure_token()?;
        let url = format!("{API_BASE}/lists/@default/tasks/{id}");
        if std::env::var("UNINOTE_SYNC_DEBUG").is_ok() {
            eprintln!(
                "[debug] PATCH {url} body={}",
                serde_json::to_string(body).unwrap_or_default()
            );
        }
        let resp = self
            .agent
            .request("PATCH", &url)
            .set("Authorization", &format!("Bearer {}", self.tokens.access_token))
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

    /// タスク削除。404 は冪等成功扱い。
    pub fn delete_task(&mut self, id: &str) -> Result<(), String> {
        self.ensure_token()?;
        let url = format!("{API_BASE}/lists/@default/tasks/{id}");
        let resp = self
            .agent
            .delete(&url)
            .set("Authorization", &format!("Bearer {}", self.tokens.access_token))
            .call();
        match resp {
            Ok(r) => {
                let status = r.status();
                if (200..300).contains(&status) {
                    Ok(())
                } else {
                    Err(format!("delete_task 失敗: {status}"))
                }
            }
            Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(classify_error(e)),
        }
    }

    /// @default タスクリストから完了タスクも含めて全件取得
    pub fn list_all_tasks(&mut self) -> Result<Vec<GTask>, String> {
        self.ensure_token()?;
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..50 {
            let mut url = format!(
                "{API_BASE}/lists/@default/tasks?showCompleted=true&showHidden=true&maxResults=100"
            );
            if let Some(t) = &page {
                url.push_str("&pageToken=");
                url.push_str(&oauth::urlencoded(t));
            }
            let resp = self
                .agent
                .get(&url)
                .set("Authorization", &format!("Bearer {}", self.tokens.access_token))
                .call()
                .map_err(classify_error)?;
            if resp.status() != 200 {
                return Err(format!(
                    "予期しないステータス: {} {}",
                    resp.status(),
                    resp.status_text()
                ));
            }
            let parsed: TasksList = resp
                .into_json()
                .map_err(|e| format!("JSON 解析失敗: {e}"))?;
            out.extend(parsed.items);
            match parsed.next_page_token {
                Some(t) if !t.is_empty() => page = Some(t),
                _ => break,
            }
        }
        Ok(out)
    }
}

fn classify_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => {
            "認証失敗（トークン期限切れの可能性、--auth で再認可してください）".to_string()
        }
        ureq::Error::Status(403, _) => "アクセス拒否".to_string(),
        ureq::Error::Status(429, _) => "レート制限".to_string(),
        ureq::Error::Status(c, r) => format!("HTTP {c}: {}", r.status_text()),
        ureq::Error::Transport(t) => format!("通信失敗: {t}"),
    }
}
