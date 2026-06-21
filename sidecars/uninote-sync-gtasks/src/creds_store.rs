//! OAuth クレデンシャル（client_id / client_secret）保存。
//! 秘匿性はトークンより低い（誰でも自分の Cloud プロジェクト作成可能）が、
//! 一応 DPAPI で軽く保護する。設定ファイルは sync/uninote-sync-gtasks.client にバイナリ保存。
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CLIENT_FILE: &str = "uninote-sync-gtasks.client";

#[derive(Serialize, Deserialize, Clone)]
pub struct ClientCredentials {
    pub client_id: String,
    pub client_secret: String,
}

fn sync_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let d = cwd.join("sync");
    let _ = fs::create_dir_all(&d);
    d
}

fn client_path() -> PathBuf {
    sync_dir().join(CLIENT_FILE)
}

pub fn save(creds: &ClientCredentials) -> Result<(), String> {
    let json = serde_json::to_vec(creds).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let blob = crate::dpapi::encrypt(&json)?;
    #[cfg(not(windows))]
    let blob = json;
    fs::write(client_path(), &blob).map_err(|e| format!("client 書込失敗: {e}"))
}

pub fn load() -> Result<ClientCredentials, String> {
    let p = client_path();
    let blob = fs::read(&p).map_err(|e| {
        format!(
            "クレデンシャル {} が読めません: {e}",
            p.display()
        )
    })?;
    #[cfg(windows)]
    let json = crate::dpapi::decrypt(&blob)?;
    #[cfg(not(windows))]
    let json = blob;
    serde_json::from_slice(&json).map_err(|e| format!("クレデンシャル解析失敗: {e}"))
}

pub fn clear() -> Result<bool, String> {
    let p = client_path();
    if p.exists() {
        fs::remove_file(&p).map_err(|e| format!("削除失敗: {e}"))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn exists() -> bool {
    client_path().exists()
}
