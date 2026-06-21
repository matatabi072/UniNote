//! トークン保存。サイドカー exe と同じフォルダの sync/uninote-sync-todoist.token に
//! DPAPI で暗号化したバイナリを格納する。
//! 開発・テスト用に環境変数 UNINOTE_TODOIST_TOKEN もフォールバックでサポート。
use std::fs;
use std::path::PathBuf;

const TOKEN_FILE: &str = "uninote-sync-todoist.token";
const ENV_VAR: &str = "UNINOTE_TODOIST_TOKEN";

/// データフォルダ（exe と同じディレクトリ）配下の sync/ を返す。
/// UniNote 本体から呼ばれた時は working_dir = UniNote のデータフォルダ。
fn sync_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let d = cwd.join("sync");
    let _ = fs::create_dir_all(&d);
    d
}

fn token_path() -> PathBuf {
    sync_dir().join(TOKEN_FILE)
}

pub fn save(token: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let enc = crate::dpapi::encrypt(token.as_bytes())?;
        fs::write(token_path(), &enc).map_err(|e| format!("書き込み失敗: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = token;
        Err("非Windows環境はサポートされません".into())
    }
}

pub fn load() -> Result<String, String> {
    // 1) 環境変数優先（開発/CI用）
    if let Ok(t) = std::env::var(ENV_VAR) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    // 2) 暗号化ファイル
    #[cfg(windows)]
    {
        let p = token_path();
        let bytes = fs::read(&p).map_err(|e| {
            format!(
                "トークンファイル {} が読めません: {e}",
                p.display()
            )
        })?;
        let plain = crate::dpapi::decrypt(&bytes)?;
        let s = String::from_utf8(plain).map_err(|_| "UTF-8 不正".to_string())?;
        Ok(s.trim().to_string())
    }
    #[cfg(not(windows))]
    {
        Err("トークンが取得できません".into())
    }
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
    if std::env::var(ENV_VAR).is_ok() {
        return true;
    }
    token_path().exists()
}
