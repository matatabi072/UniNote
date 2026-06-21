# UniNote サイドカー契約 v1

UniNote 本体と外部サービス連携アダプター（サイドカー）の接続仕様。  
本ドキュメントに準拠して実装すれば、UniNote 本体のコードに手を入れず、独立 exe として配布・追加できます。

## 1. 設計原則

- **Local-First**: UniNote のデータ正本は常にローカル JSON ファイル。サイドカーは JSON を読み書きするだけ。
- **疎結合**: 本体とサイドカーは **JSON ファイル＝契約** で繋がる。通信プロトコルは不要。
- **独立プロセス**: サイドカーは別 exe。本体のクラッシュ耐性を保つ。
- **単方向呼び出し**: 本体がサイドカーを起動する（逆はない）。

## 2. 配置場所

```
<UniNote.exe と同じフォルダ>/
├ uninote.exe
├ tasks.json
├ notes.json
├ images/
└ sync/
   ├ uninote-sync-<service>.exe   ← ここにサイドカーを置く
   ├ uninote-sync-<service>.config.json   ← 任意（サイドカーが自由に管理）
   └ uninote-sync-<service>.log           ← 任意
```

ファイル名は `uninote-sync-<service>.exe`（小文字英数、`-` 区切り）。  
本体はこのパターンに合致する exe を自動検出します。

## 3. データファイル

### 3.1 `tasks.json` / `notes.json`

両方とも同じスキーマの **JSON 配列**。違いは「タスクの集合」か「メモの集合」か、というユーザー側の意味付けだけ。

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "googleTaskId": null,
    "taskContent": "資料を印刷する",
    "isCompleted": false,
    "scheduledDateTime": "2026-06-25T15:00:00",
    "priority": "high",
    "manualOrder": 0,
    "updatedAt": "2026-06-24T10:30:00+00:00",
    "kind": "text",
    "imageName": null
  }
]
```

### 3.2 フィールド仕様

| フィールド | 型 | 必須 | 説明 |
|----------|-----|------|------|
| `id` | string (UUID v4) | ✓ | 本体側のローカル ID。変更不可 |
| `googleTaskId` | string \| null | | 外部サービスの ID 格納用。アダプターが管理 |
| `taskContent` | string | ✓ | 本文（テキストメモ）/ タイトル（タスク）/ 相対パス（画像メモ "images/...") |
| `isCompleted` | bool | ✓ | 完了フラグ（タスク用、メモは常に false でよい） |
| `scheduledDateTime` | string (NaiveDateTime ISO8601) \| null | | 予定日時（ローカル時刻）。例: `"2026-06-25T15:00:00"` |
| `priority` | `"high"`\|`"medium"`\|`"low"`\|`"none"` | ✓ | 重要度（メモは `"none"`） |
| `manualOrder` | integer | ✓ | 手動並び順（0 始まり、重複可だが昇順推奨） |
| `updatedAt` | string (RFC3339) | ✓ | 競合解決用タイムスタンプ。`"2026-06-24T10:30:00+00:00"` |
| `kind` | `"text"`\|`"image"` | | メモ専用。未指定は `"text"` |
| `imageName` | string \| null | | 画像メモの元ファイル名（表示用） |

**外部サービス固有フィールドの追加**: 上記以外のフィールドを足しても本体は破壊しません（serde が未知フィールドを無視）。アダプターは独自フィールド（例: `notionPageId`, `todoistDue` 等）を自由に追加可能。

### 3.3 画像ファイル

画像メモの実体は `images/{uuid}.{ext}` に保存され、`taskContent` にはそこへの相対パス（例: `"images/abc123.png"`）が入る。  
サイドカーが画像を扱う場合、`taskContent` のパスを起点に絶対パス解決すること。

## 4. 書き込みルール（必須）

データファイルを書き換えるサイドカーは **必ず以下を守る**。守らないと UniNote 本体が破損ファイルを検知してバックアップから復元してしまう。

### 4.1 アトミック書き込み

```
1. <name>.json.tmp に新内容を書き出す
2. （任意）既存 <name>.json を <name>.backup.json にコピー
3. <name>.json.tmp を <name>.json にリネーム（既存を置換）
```

直接 `<name>.json` に書き込むと、書き込み途中で UniNote が読みに来た場合に破損扱いになる。

### 4.2 JSON 整形

`serde_json::to_string_pretty` 相当のインデント整形を推奨（diff 取得・手動編集を容易にする）。

### 4.3 ID とフィールド保持

- 既存 `id` は **絶対に書き換えない**
- 自分が理解できないフィールド（他アダプターが付けたものなど）は **保持して書き戻す**
- 何かを変更したら `updatedAt` を **必ず更新**（RFC3339, UTC 推奨）

## 5. サイドカーの呼び出し規約

### 5.1 起動

本体は以下のように起動する：

```
working_directory: <UniNote.exe と同じフォルダ>
command:           sync/uninote-sync-<service>.exe
arguments:         （なし - v1）
stdin:             （未使用）
stdout:            キャプチャ → 同期ログウィンドウに表示
stderr:            キャプチャ → 同期ログウィンドウに `[err]` プレフィックス付きで表示
```

サイドカーは `tasks.json` / `notes.json` を **カレントディレクトリ起点** で開けばよい。

### 5.2 出力

- **stdout**: 進捗・情報ログ（人間可読、1行1メッセージ推奨）
- **stderr**: 警告・エラー
- 終端のバッファリングは flush すること（リアルタイム表示用）

ログ行先頭の軽量タグは任意：
```
[info] 接続中: Todoist API
[sync] 取得: 12件
[sync] 送信: 3件
[warn] 重複検知: id=xxx をスキップ
```

### 5.3 終了コード

| Code | 意味 |
|------|------|
| `0` | 成功（変更ありなしを問わず） |
| `1` | 部分失敗（一部同期できたが警告あり） |
| `2` | 設定不足（config.json なし・認証情報なし等） |
| `3` | ネットワーク失敗（リトライで回復見込み） |
| `4` | 認証失敗（再認証必要） |
| `非0 上記以外` | 想定外エラー |

本体は終了後に **自動でファイル外部変更を検知 → 再読込** する。サイドカーは「本体に通知する」ような追加処理は不要。

### 5.4 設定ファイル

サイドカーが必要とする設定（APIトークン等）は **サイドカー自身が** `sync/uninote-sync-<service>.config.json` 等で管理する。本体はこれに関与しない。

セキュリティ重要事項：
- API トークン等の秘密情報は **平文 JSON で保存しない**
- Windows なら DPAPI（`CryptProtectData`）でユーザースコープ暗号化推奨
- 設定ファイルは **絶対に Git に commit させない**（`.gitignore` 案内をユーザーに）

## 6. 競合解決の指針

UniNote 本体は「**最終更新勝ち（updatedAt 比較）**」をベースに、未保存編集ありの場合のみダイアログを出す。サイドカーは同じルールで動作することが望ましい：

- ローカル `updatedAt` > リモート → ローカルを送信
- リモート `updatedAt` > ローカル → リモートを取り込み
- 同一 → 何もしない
- 削除 vs 更新の衝突 → **更新側を優先**（データ消失を避ける）

`googleTaskId` 等の外部 ID マッピングフィールドはアダプター内で永続化する。

## 7. 推奨される運用

- **手動実行**: 本体の「🔄 同期」ボタンからの呼び出しで使用
- **定期実行**: Windows タスクスケジューラから直接サイドカー exe を起動できる（本体が起動していなくても動く）
- **常駐**: 推奨しない（バッテリー・常駐プロセスの問題）

## 8. 参照実装

- `uninote-sync-todoist`（予定）: Todoist REST API v2
- `uninote-sync-gtasks`（予定）: Google Tasks API v1
- `uninote-sync-notion`（予定）: Notion API v1

各アダプターは別リポジトリ。Rust 推奨だが言語不問（Python / Go / Node でも可、ABI 制約なし）。
