//! OAuth トークン（access_token / refresh_token）の保存。
//! sync/uninote-sync-gtasks.tokens に DPAPI 暗号化バイナリで格納。
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const TOKEN_FILE: &str = "uninote-sync-gtasks.tokens";

#[derive(Serialize, Deserialize, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// access_token の有効期限切れタイムスタンプ（unix epoch 秒）
    pub expires_at: i64,
}

fn sync_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let d = cwd.join("sync");
    let _ = fs::create_dir_all(&d);
    d
}

fn token_path() -> PathBuf {
    sync_dir().join(TOKEN_FILE)
}

pub fn save(t: &Tokens) -> Result<(), String> {
    let json = serde_json::to_vec(t).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let blob = crate::dpapi::encrypt(&json)?;
    #[cfg(not(windows))]
    let blob = json;
    fs::write(token_path(), &blob).map_err(|e| format!("token 書込失敗: {e}"))
}

pub fn load() -> Result<Tokens, String> {
    let p = token_path();
    let blob = fs::read(&p)
        .map_err(|e| format!("トークン {} が読めません: {e}", p.display()))?;
    #[cfg(windows)]
    let json = crate::dpapi::decrypt(&blob)?;
    #[cfg(not(windows))]
    let json = blob;
    serde_json::from_slice(&json).map_err(|e| format!("トークン解析失敗: {e}"))
}

pub fn clear() -> Result<bool, String> {
    let p = token_path();
    if p.exists() {
        fs::remove_file(&p).map_err(|e| format!("削除失敗: {e}"))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn exists() -> bool {
    token_path().exists()
}
