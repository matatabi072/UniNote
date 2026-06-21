# uninote-sync-gtasks

[UniNote](../../) 用 **Google Tasks 同期サイドカー**（v1: PULL only、約 1.6MB の単一 exe）。

UniNote モノレポの一部。本体の使い方や全体構成は [親 README](../../README.md) を参照してください。

## ✨ 機能

- OAuth 2.0 Installed App（127.0.0.1 ループバック）
- access_token 自動 refresh、refresh_token 失効時はトークン削除して再認可案内
- ローカル削除追跡（tombstones）で `state.json` を維持
- DPAPI で credentials/tokens を暗号化保存
- ページネーション対応、完了タスクも取り込み

## 🚀 セットアップ

本体の GUI ウィザード（**設定 → 外部連携 → 🔗 外部連携の設定 → Google Tasks タブ**）から行うのが推奨です。Cloud Console での OAuth クライアント作成手順もウィザード内に表示されます。

### Google Cloud Console での事前設定（要約）

1. https://console.cloud.google.com/ にログイン → プロジェクト作成
2. **APIとサービス → ライブラリ**: 「Google Tasks API」を有効化
3. **APIとサービス → OAuth同意画面**: 対象「外部」、スコープ `.../auth/tasks`、自分の Gmail をテストユーザーに追加
4. **APIとサービス → 認証情報**: OAuth クライアント ID（**デスクトップアプリ**）を作成し、Client ID / Secret をコピー

### CLI でセットアップする場合

```bash
uninote-sync-gtasks --setup "<CLIENT_ID>" "<CLIENT_SECRET>"
uninote-sync-gtasks --auth      # ブラウザで認可フロー
uninote-sync-gtasks             # 同期実行
```

「このアプリは確認されていません」警告は自分のテストアプリのため正常です。「詳細 → 移動」で続行可。

## 🖥 CLI

```
uninote-sync-gtasks                       同期実行（PULL）
uninote-sync-gtasks --setup CID SECRET    OAuth クレデンシャル保存
uninote-sync-gtasks --auth                ブラウザで認可フロー
uninote-sync-gtasks --clear-auth          トークン削除（クレデンシャル保持）
uninote-sync-gtasks --clear-all           全削除
uninote-sync-gtasks --status              状態表示
uninote-sync-gtasks --help                ヘルプ
```

## 🔄 同期内容（v1）

- 対象: 既定のタスクリスト（`@default`）のみ
- 方向: Google Tasks → ローカル（PULL only）
- `id`→`googleTaskId`、`title`→`taskContent`、`status:completed`→`isCompleted:true`、`due`→`scheduledDateTime`（時刻は 9:00 正規化）
- ローカル削除は tombstones で追跡し、remote から再追加されない

## 🔚 終了コード

| Code | 意味 |
|------|------|
| 0 | 成功 |
| 1 | 一般失敗 |
| 2 | 設定不足 |
| 3 | 通信失敗 |
| 4 | 認証失敗 |

## 🗂 生成ファイル（`sync/` 配下）

| ファイル | 内容 |
|----------|------|
| `uninote-sync-gtasks.exe` | 本体 |
| `uninote-sync-gtasks.client` | DPAPI 暗号化 client_id/secret |
| `uninote-sync-gtasks.tokens` | DPAPI 暗号化 access/refresh tokens |
| `uninote-sync-gtasks.state.json` | tombstones（削除追跡） |

## 🚧 ロードマップ

- v2: 双方向同期（push CREATE/UPDATE/DELETE）
- v3: 複数タスクリスト対応

## 📄 ライセンス
[MIT License](../../LICENSE) — workspace 共通
