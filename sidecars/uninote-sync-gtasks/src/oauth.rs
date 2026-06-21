//! OAuth 2.0 Installed App フロー（ループバック・コールバック）。
//!
//! 流れ:
//! 1. 127.0.0.1 で空きポートにバインドして HTTP サーバ起動
//! 2. ブラウザを Google の認可 URL に飛ばす（redirect_uri に上記ポート）
//! 3. ユーザーが同意すると Google が redirect_uri に code=... で GET してくる
//! 4. サーバが code を受け取り、ブラウザに「閉じてOK」HTML を返す
//! 5. code をトークンエンドポイントに POST して access/refresh_token を取得
use crate::creds_store::ClientCredentials;
use crate::token_store::Tokens;
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{SystemTime, UNIX_EPOCH};

const SCOPE: &str = "https://www.googleapis.com/auth/tasks";
const AUTH_BASE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

/// 認可フロー全体を実行し、得たトークンを返す（保存はしない）。
pub fn run_authorization(creds: &ClientCredentials) -> Result<Tokens, String> {
    // 1) 空きポート確保
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("ローカルポート確保失敗: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/");
    let state_token = uuid::Uuid::new_v4().to_string();

    // 2) ブラウザを認可URLへ
    let auth_url = format!(
        "{AUTH_BASE}?client_id={cid}&redirect_uri={redir}&response_type=code&scope={scope}\
         &state={state}&access_type=offline&prompt=consent",
        cid = urlencoded(&creds.client_id),
        redir = urlencoded(&redirect_uri),
        scope = urlencoded(SCOPE),
        state = urlencoded(&state_token),
    );
    println!("[info] ブラウザで認可画面を開きます…");
    println!("[info] ブラウザが開かない場合は次のURLを手動で開いてください:");
    println!("       {auth_url}");
    let _ = open_url(&auth_url);

    // 3) コールバック待ち。ブラウザによっては favicon.ico を先に要求してくる等で
    //    無関係なリクエストが先に来る可能性があるため、code を含むリクエストが届くまで accept を繰り返す。
    println!("[info] Google からのコールバックを待機中… (127.0.0.1:{port})");
    let code = loop {
        let (mut stream, _) = listener
            .accept()
            .map_err(|e| format!("コールバック受信失敗: {e}"))?;

        // HTTP リクエストヘッダを `\r\n\r\n` まで（または上限まで）読み切る
        let mut buf: Vec<u8> = Vec::with_capacity(2048);
        let mut tmp = [0u8; 1024];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 32_768 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let req = String::from_utf8_lossy(&buf).to_string();
        let path = req
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();

        // OAuth コールバックでないリクエスト（favicon.ico 等）は 404 を返して継続
        if !path.contains("code=") && !path.contains("error=") {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }

        // Google からのエラー応答
        if path.contains("error=") {
            let err_msg = path
                .split('&')
                .find_map(|p| p.strip_prefix("?error=").or_else(|| p.strip_prefix("error=")))
                .unwrap_or("(unknown)");
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            return Err(format!("Google が認可を拒否しました: {err_msg}"));
        }

        let (code, returned_state) = parse_query(&path);
        if returned_state.as_deref() != Some(state_token.as_str()) {
            let _ = stream.write_all(
                b"HTTP/1.1 400 Bad Request\r\n\r\nstate mismatch. Close and retry.",
            );
            return Err("state トークン不一致（CSRF対策）".into());
        }
        let Some(code_val) = code else {
            let _ = stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\nno code in callback");
            continue; // 同じセッションでまた来るかもしれないので継続
        };

        let body = "<!doctype html><meta charset=utf-8><title>UniNote</title>\
                     <body style='font-family:sans-serif;padding:2em'>\
                     <h2>UniNote: Google Tasks 認可完了</h2>\
                     <p>このタブは閉じて構いません。</p></body>"
            .as_bytes();
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.write_all(body);
        break code_val;
    };

    // 4) コードをトークンに交換
    println!("[info] コードをトークンに交換中…");
    let form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("client_id", creds.client_id.as_str()),
        ("client_secret", creds.client_secret.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
    ];
    let resp = ureq::post(TOKEN_URL)
        .send_form(&form)
        .map_err(|e| format!("トークン交換失敗: {e}"))?;
    let tr: TokenResponse = resp
        .into_json()
        .map_err(|e| format!("トークン応答解析失敗: {e}"))?;
    let refresh = tr
        .refresh_token
        .ok_or("refresh_token が返ってきません（prompt=consent 指定の確認を）")?;

    Ok(Tokens {
        access_token: tr.access_token,
        refresh_token: refresh,
        expires_at: now_unix() + tr.expires_in - 60, // 60秒マージン
    })
}

/// refresh_token を使って access_token を再取得。refresh_token は保持。
pub fn refresh_access_token(creds: &ClientCredentials, tokens: &Tokens) -> Result<Tokens, String> {
    let form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", tokens.refresh_token.as_str()),
        ("client_id", creds.client_id.as_str()),
        ("client_secret", creds.client_secret.as_str()),
    ];
    let resp = ureq::post(TOKEN_URL)
        .send_form(&form)
        .map_err(|e| format!("refresh 失敗: {e}"))?;
    let tr: TokenResponse = resp
        .into_json()
        .map_err(|e| format!("refresh 応答解析失敗: {e}"))?;
    Ok(Tokens {
        access_token: tr.access_token,
        refresh_token: tokens.refresh_token.clone(), // refresh は変わらない
        expires_at: now_unix() + tr.expires_in - 60,
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_query(path: &str) -> (Option<String>, Option<String>) {
    let q = match path.split_once('?') {
        Some((_, q)) => q,
        None => return (None, None),
    };
    let mut code = None;
    let mut state = None;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let v = url_decode(v);
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                _ => {}
            }
        }
    }
    (code, state)
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(h) =
                    u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    out.push(h as char);
                    i += 3;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

pub(crate) fn urlencoded(s: &str) -> String {
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

#[cfg(windows)]
fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::Command;
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::Command;
    Command::new("xdg-open").arg(url).spawn().map(|_| ())
}
