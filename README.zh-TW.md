# Aegis

[English](README.md) · [繁體中文](README.zh-TW.md)

> Aegis 只陳述事實。判決留給代理人。

給 LLM 編程工作流程用的「無判決觀察層」。

> **如果你是 AI 編碼代理**（Claude Code、Cursor、Aider 等）正在
> 協助使用者安裝 Aegis，請改讀 [`AGENTS.md`](AGENTS.md) ——
> 那裡有命令式的安裝指令。

---

## 這是什麼（V2）

Aegis 是一個 MCP server，對「準備寫進檔案的內容」產出 **findings**。
它不擋、不警告、不放行、也不下任何判決。每一條 finding 是一個事實
——「這個 regex 在這裡命中了」、「這個 signal 升 1」、「這個檔案
import 的東西也 import 自己」—— 並帶足夠的結構化 context，讓消費端
代理（也就是 LLM）自己決定要怎麼處理。

**Aegis 不寫程式、不擋寫入、不評分結果。** 它只描述「變了什麼」。

之前 V1 的架構（Ring 0/0.5/0.7/R2 + BLOCK/WARN/PASS 判決、
multi-turn pipeline、cost-aware regression rollback、stalemate /
thrashing 偵測）在 V2 全部移除。判決應該存在的地方是消費端代理的
推理過程，不是這個工具。

---

## 為什麼存在

LLM 系統有三種失敗模式是現有工具抓不到的：

1. 多輪重構靜默累積退化
2. LLM 描述的動作跟實際工具呼叫不一致
3. 結構規則悄悄被侵蝕，沒人發現

Aegis 存在的目的就是讓這些失敗**可見**。在這個 context 下能不能
接受是代理或人類的問題；Aegis 只負責確保資料攤在桌面上。

---

## 怎麼運作

兩層基礎設施 + 一個 MCP tool。

```
┌─────────────────────────────────────┐
│ MCP Tool: validate_file             │
│   (path, new_content,               │
│    old_content?, workspace_root?)   │
└──────────────┬──────────────────────┘
               │ findings[]
               ▼
┌─────────────────────────────────────┐
│ Findings 產生器                      │
│   Syntax · Signal · Security        │
│   Workspace                         │
└──────────────┬──────────────────────┘
               │
       ┌───────┴────────┐
       ▼                ▼
┌─────────────┐  ┌─────────────────┐
│ Layer 1     │  │ Layer 2         │
│ parse(file) │  │ WorkspaceIndex  │
│  → Tree     │  │ （mtime 快取）  │
└─────────────┘  └─────────────────┘
```

**Layer 1 — parse**：每個檔案 tree-sitter 解析一次，下游所有 finding
產生器共用同一棵樹。不再有「每個 signal 自己 `Parser::new()`」、不再
有「寫 temp 檔再讀回」。語法有錯也照樣回樹。

**Layer 2 — WorkspaceIndex**：跨檔的反向索引（imports、public
symbols、per-file signals），用 mtime cache 確保重複呼叫只重 parse
真的有改動的檔案。

**Findings**：四種 kind —— Syntax、Signal、Security、Workspace。
每條都帶 `file`、可選的 `range` 跟 `snippet`、結構化的 `context`。
**不帶 severity**。

---

## Findings 種類

| `kind` | 意義 | 範例 `rule_id` |
| :--- | :--- | :--- |
| **Syntax** | tree-sitter 找到 ERROR / MISSING 節點。 | `ring0_violation` |
| **Signal** | 結構性計數器（14 個）。傳了 `old_content` 時，`context` 帶 `value_before` / `value_after` / `delta`。 | `fan_out`、`max_chain_depth`、`cyclomatic_complexity`、`nesting_depth`、`empty_handler_count`、`unfinished_marker_count`、`unreachable_stmt_count`、`mutable_default_arg_count`、`shadowed_local_count`、`suspicious_literal_count`、`unresolved_local_import_count`、`member_access_count`、`type_leakage_count`、`cross_module_chain_count`、`import_usage_count`、`test_count_lost` |
| **Security** | 命中具體的反模式（10 條規則）。`context.severity_hint` 是建議不是判決。 | `SEC001`–`SEC010`（eval/exec、寫死的 secret、關 TLS、shell injection、SQL 拼接、CORS 萬用字元+credentials、JWT 不驗證、危險反序列化、弱 hash、弱 RNG）|
| **Workspace** | 跨檔 finding。只有傳 `workspace_root` 才會出現。 | `cycle_introduced`、`public_symbol_removed`、`file_role` |

`aegis-allow: <rule_id>`（或 `aegis-allow: all`）寫在同一行或前一行
時，**不會把 finding 過濾掉**，而是把該 finding 的
`user_acknowledged` 設為 `true`。代理會看見這個標記，可以自己決定
要不要尊重。

---

## 快速開始

V2 只有一個二進位檔：`aegis-mcp`（MCP server）。

### 安裝

```bash
# 前置：git + Rust toolchain（1.74+）
git clone https://github.com/wei9072/aegis && cd aegis
cargo install --path crates/aegis-mcp
```

### 設定你的 MCP client

讓 MCP 認得 `aegis-mcp` 這個 binary（透過 stdio）。每個 client 設定
語法不同；server 本身不吃任何旗標。

### 唯一的工具：`validate_file`

```jsonc
{
  "name": "validate_file",
  "arguments": {
    "path": "src/auth.py",
    "new_content": "...",                  // 必填
    "old_content": "...",                  // 選填 — 開啟 delta
    "workspace_root": "/path/to/project"   // 選填 — 加入 Workspace findings
  }
}
```

回傳：

```json
{
  "schema_version": "v2.0",
  "findings": [
    {
      "kind": "security",
      "rule_id": "SEC009",
      "file": "src/auth.py",
      "range": { "start_line": 47, "start_col": 4, "end_line": 47, "end_col": 52 },
      "context": { "severity_hint": "block", "message": "weak hash …" },
      "user_acknowledged": false
    },
    {
      "kind": "signal",
      "rule_id": "unfinished_marker_count",
      "file": "src/auth.py",
      "context": { "value_before": 0, "value_after": 1, "delta": 1 },
      "user_acknowledged": false
    },
    {
      "kind": "workspace",
      "rule_id": "cycle_introduced",
      "file": "src/auth.py",
      "context": { "cycle": ["src/auth.py", "src/user.py", "src/auth.py"] },
      "user_acknowledged": false
    }
  ]
}
```

**第一次**帶 `workspace_root` 的呼叫會建立 workspace index（把所有
支援的檔案 parse 一次）；之後的呼叫吃 cache，只重 parse mtime 變過
的檔案。**不需要另外的 "scan" 步驟**。

---

## 支援的原始碼語言

依副檔名 runtime 分派。新增一個語言只需要 ── 一個 Cargo dep + 一個
`crates/aegis-core/src/ast/languages/` 下的 adapter 檔 + 一個 `.scm`
import query。

| 語言 | Layer 1 parse | 副檔名 |
| :--- | :---: | :--- |
| Python | ✅ | `.py`、`.pyi` |
| TypeScript | ✅ | `.ts`、`.tsx`、`.mts`、`.cts` |
| JavaScript | ✅ | `.js`、`.mjs`、`.cjs`、`.jsx` |
| Go | ✅ | `.go` |
| Java | ✅ | `.java` |
| C# | ✅ | `.cs` |
| PHP | ✅ | `.php`、`.phtml`、`.php5`、`.php7`、`.phps` |
| Swift | ✅ | `.swift` |
| Kotlin | ✅ | `.kt`、`.kts` |
| Dart | ✅ | `.dart` |
| Rust | ✅ | `.rs` |

---

## 設計原則

- **陳述事實，不下判決。** Findings 沒有 severity 欄位。消費端代理
  自己決定哪些 finding 重要、要怎麼反應。
- **Parse 一次、共享樹。** 每個 finding 產生器吃 `ParsedFile`。沒有
  per-signal 的 `Parser::new()`、沒有寫 temp 檔再讀回。
- **Workspace bootstrap 是隱式的。** 第一次帶 `workspace_root` 時建
  立 index；之後吃 mtime cache。**沒有獨立的 scan tool**、不需手動
  初始化。
- **不自動學習、不目標導向優化。** Aegis 不追蹤跨呼叫的成敗、不調整
  規則、不評分。state 只剩 workspace cache。
- **MCP 一個工具、surface 很窄。** `validate_file`，就這樣。沒有
  `retry` / `hint` / `explain`。代理的推理是代理自己的事。

---

## 狀態

| 層 | 狀態 |
| :--- | :--- |
| Layer 1（parse + 11 個語言 adapter）| ✅ |
| Layer 2（WorkspaceIndex + mtime cache）| ✅ |
| Findings：Syntax + Signal + Security + Workspace | ✅ |
| MCP server（`aegis-mcp`）| ✅ |
| V1 binaries（`aegis`、`aegis pipeline run`、`aegis check`、`aegis attest`、`aegis scan`）| ❌ V2 移除 |

---

## 授權

MIT —— 見 [`LICENSE`](LICENSE)。

V2 —— MCP-only 架構。Pipeline / runtime / providers / IR / decision
等 crate 全部移除；判決交給消費端代理。
