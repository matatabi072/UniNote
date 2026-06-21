# uninote-sync-todoist

[UniNote](../../) 用 **Todoist 双方向同期サイドカー**（約 1.6MB の単一 exe）。3-way merge で Todoist API v1 と双方向同期。

UniNote モノレポの一部。本体の使い方や全体構成は [親 README](../../README.md) を参照してください。

## ✨ 機能

- 双方向同期（CREATE / UPDATE / 完了状態 / DELETE）
- 3-way merge（state.json スナップショットと比較）
- 競合解決: REMOTE WINS + 警告ログ
- ローカル削除 → Todoist 側にも DELETE 反映（誤復活防止）
- DPAPI トークン暗号化
- dry-run モード対応

## 🚀 セットアップ（CLI）

本体の GUI ウィザード（**設定 → 外部連携 → 🔗 外部連携の設定**）から行うのが推奨です。CLI なら：

```bash
# 1) トークン取得 → https://app.todoist.com/app/settings/integrations/developer
uninote-sync-todoist --set-token "<YOUR_API_TOKEN>"
# 2) 同期実行
uninote-sync-todoist
```

## 🖥 CLI

```
uninote-sync-todoist                  双方向同期実行
uninote-sync-todoist --dry-run        実通信せず計画だけ表示
uninote-sync-todoist --set-token T    DPAPI 暗号化保存
uninote-sync-todoist --clear-token    保存済みトークン削除
uninote-sync-todoist --status         状態表示
uninote-sync-todoist --help           ヘルプ
```

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
| `uninote-sync-todoist.exe` | 本体 |
| `uninote-sync-todoist.token` | DPAPI 暗号化トークン |
| `uninote-sync-todoist.state.json` | 同期スナップショット |

## 📄 ライセンス
[MIT License](../../LICENSE) — workspace 共通
