# Aegis

[English](README.md) · [繁體中文](README.zh-TW.md)

> Aegis 確保壞決策不會留下。

一個用來防止 LLM 驅動工作流程中靜默退化（silent regression）的系統。

> **如果你是 AI 編碼代理**（Claude Code、Cursor、Aider 等）正在
> 協助使用者安裝或整合 Aegis，請改讀 [`AGENTS.md`](AGENTS.md)
> ——那裡有命令式的安裝指令、可直接貼上的整合模板，以及你
> 必須遵守的 framing 規則。本 README 其餘內容是給評估是否
> 採用 Aegis 的人類讀者。

---

## 這是什麼

Aegis 是給 LLM 系統用的、以約束為基礎的行為控管框架。
（廣義地說：一個驗證環境，而不是代理驅動器。）

跨領域的框架層定義請見 [`docs/framework.md`](docs/framework.md)。
本 repo 是程式碼變動域上的 reference implementation：把 Aegis 套用到代理驅動的
程式碼變更上。

**Aegis 不寫程式，也不告訴 LLM 該怎麼寫程式。它只裁決 LLM
產出的程式碼能不能留下。** LLM（或任何你包進去的程式碼生成器）
對「寫什麼」保有完整的撰寫角色；Aegis 只在「什麼能通過閘門」
這件事上行使裁決角色。

它執行的是一個區域性的閉環迴圈：
每一個被提案的狀態轉移都會被驗證，
退化會被拒絕。

Aegis 不去優化模型的行為，
而是強制執行明確、可驗證的約束，
確保不合法或退化的狀態不會留下。

---

## 為什麼存在

LLM 系統有三種失敗模式是現有工具抓不到的：

1. 多輪重構靜默累積退化
2. LLM 描述的動作跟實際工具呼叫不一致
3. 結構規則悄悄被侵蝕，沒人發現

Aegis 存在的目的就是讓這些失敗變得可見、可拒絕。

---

## 核心機制

Aegis 控制狀態轉移：

```
Sₙ → Sₙ₊₁ 只有當所有約束都滿足、且沒有退化發生時才被允許。

否則，系統回滾到 Sₙ。
```

成本感知（cost-aware）回滾是唯一跨輪一致的判準。其他檢查
（驗證、結構約束）只擔任區域守衛角色，不是全域方向訊號。

### V1.10 實際檢查的項目（八層）

| 層 | 檢查項目 | 在哪裡執行 |
| :--- | :--- | :--- |
| **Ring 0** — 語法 | tree-sitter 解析；遇到 ERROR / MISSING 節點 → BLOCK | `aegis check`、MCP、pipeline、`aegis attest` |
| **Ring 0.5** — 結構訊號 | 一次 AST 巡訪抽出 14 個訊號，並依嚴重度分為三級 —— **block 級**（`empty_handler_count`、`unreachable_stmt_count`、`mutable_default_arg_count`、`shadowed_local_count`、`suspicious_literal_count`、`unresolved_local_import_count`、`unfinished_marker_count`、`test_count_lost`）、**warn 級**（`fan_out`、`max_chain_depth`、`cyclomatic_complexity`、`nesting_depth`、`cross_module_chain_count`、`import_usage_count`）、**info 級**（`member_access_count`、`type_leakage_count`）。嚴重度決定退化是否升級為 BLOCK。詳見 [`signal_layer_pyapi.rs::severity_for`](crates/aegis-core/src/signal_layer_pyapi.rs)。 | `aegis check`、MCP、pipeline |
| **Ring 0.7** — 安全 | 10 條布林規則（`SEC001`–`SEC010`）：shell 注入、SQL 注入、JWT 關閉驗證、弱雜湊（md5/sha1）、安全脈絡內的不安全亂數、危險反序列化（`pickle`、`yaml.load`、`marshal`、Java `readObject` 等）。每行可用 `// aegis-allow: SEC00X`（或 `# aegis-allow: …`）豁免。 | `aegis check`、MCP、pipeline、`aegis attest` |
| **成本退化** | 逐訊號 delta —— **任一 block 級訊號上升 → BLOCK；任一 warn 級訊號上升 → WARN**（記錄理由，但不單獨 fail）。早期的 sum-based 檢查已淘汰，因為跨訊號相消會掩蓋真正的退化。 | MCP（傳入 `old_content` 時）、pipeline（每輪）|
| **Ring R2** — workspace 結構 | 單檔 `validate_change` 做不到的跨檔檢查：**循環引入偵測**（這次變更會不會新增 module import cycle？）、**公開符號被刪但仍有呼叫端**、檔案角色分類（entry / core / hub / ordinary）、針對專案基線的 fan_in / fan_out / 訊號 z-score。`entry` 角色的檔案會自動抑制 `fan_out` warn（entry 檔本來就該 import 多）。 | `aegis-mcp validate_change_with_workspace`、`aegis attest --workspace`、pipeline |
| **PlanValidator** | 路徑安全 / 範圍 / 危險路徑 / 虛擬檔案系統模擬 | 僅 `aegis pipeline run` |
| **Executor + Snapshot** | 原子套用，失敗時靠備份目錄回滾 | 僅 `aegis pipeline run` |
| **Stalemate / Thrashing 偵測器** | 序列層級；用具名理由中止迴圈 | 僅 `aegis pipeline run` |

`aegis check` 跟 MCP server 暴露前五層（單檔 + workspace 判定）；
多輪 pipeline 額外加上後三層（跨輪迴圈控制）。

每一份 verdict 的 `decision` 會是下列四值之一：

- **PASS** —— 所有 gate 都通過。
- **WARN** —— 至少有一個 warn 級 gate 觸發（例如啟發式訊號退化）；
  變更允許通過，但理由會被記錄出來。
- **BLOCK** —— 至少有一個 block 級 gate 觸發（Ring 0、Ring 0.7、
  block 級 Ring 0.5 退化、Ring R2 cycle 等）。
- **SKIP** —— 副檔名沒有對應的語言 adapter（`.md`、`.toml`、
  `.json` 等）；aegis 明確「沒意見」。在這裡回 BLOCK 只會讓
  上游代理以為它的 markdown 寫錯了。

---

## 設計原則

- 不寫程式碼；只裁決已寫的程式碼
- 不告訴模型該寫什麼；只說什麼不能留下
- 只拒絕可驗證為壞的東西
- 不自動學習
- 不做目標導向的優化

前兩條是 load-bearing 的——它們是 Aegis 為什麼能包住任何
程式碼生成代理（Cursor、Claude Code、Aider、你自己的 pipeline）
而不變成跟對方競爭的代理：Aegis 行使*裁決角色*；被包進去的
代理保有*撰寫角色*。

---

## 保證

- 當約束已定義時，退化會被偵測並回滾
- 不合法的狀態會在驗證層被擋下
- 所有決策都會被記錄到一份機器可讀的 trace 裡

---

## 非目標

Aegis 不是：

- 一個 AI 代理
- 一個優化器
- 一個自我改進系統

這些都是刻意的設計選擇，不是未來工作。
Aegis 不會演化成上述任何一種。
完整的延後清單見
[`docs/post_launch_discipline.md`](docs/post_launch_discipline.md)。

---

## 範圍

Aegis 在單一執行迴圈內強制正確性，
但不跨執行調整行為。

迴圈是區域性的，不是全域性的。

---

## 快速開始

從 V1.10 開始，Aegis 是單一個 Rust workspace，產出兩個
binary——`aegis`（CLI）跟 `aegis-mcp`（MCP stdio server）。
執行階段零 Python。

### 安裝

```bash
# 前置條件：git + Rust toolchain。
# 如果還沒裝 Rust：
#   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
#   source "$HOME/.cargo/env"

git clone https://github.com/wei9072/aegis && cd aegis
cargo build --release --workspace
```

裝到系統路徑（讓 `aegis` / `aegis-mcp` 進到 `$PATH`）：

```bash
cargo install --path crates/aegis-cli
cargo install --path crates/aegis-mcp     # 可選 — MCP server
```

跨平台 release artifacts（Linux x86_64/aarch64、macOS
x86_64/aarch64、Windows x86_64）會經由 GitHub Releases
發布——詳見 [`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md)
中的 V2.0。

### 靜態分析（不需 LLM、不需 API key）

`aegis check` 對任何支援的原始檔執行 Ring 0（語法）+
Ring 0.5（14 個結構訊號）+ Ring 0.7（10 條安全規則）：

```bash
aegis languages                       # 列出支援的語言
aegis check path/to/file.py           # 人類可讀的訊號
aegis check path/to/file.py --json    # 機器可讀
```

整 workspace 掃描 —— 平行掃描 + 持久 mtime+size 快取（穩定
codebase 重掃 < 1 秒），加上跨檔 import-graph cycle 偵測：

```bash
aegis scan --workspace .              # 平行掃描 + cycle 偵測
aegis scan --workspace . --top 20     # 列出成本最高的前 N 個檔案
aegis scan --no-cache --no-cycles     # 跳過快取 / 跳過 cycle pass
```

Post-write 證明 —— 讀檔案的 disk 內容，跑所有**絕對性**檢查
（Ring 0 + Ring 0.7 + 可選的 Ring R2 cycle）。設計給
`PostToolUse` hook / CI 用，讓繞過 pre-write 閘門的寫入仍然
會被裁決。會把 sha256 戳記過的 JSONL row 附加到
`<workspace>/.aegis/attestations.jsonl`：

```bash
aegis attest path/to/file.py --workspace .
aegis attest path/to/file.py --workspace . --json    # 機器可讀
```

設定精靈 —— 互動式寫 `~/.config/aegis/config.toml`（provider、
base URL、模型、API key 環境變數）。不會 export 任何東西到
你的 shell：

```bash
aegis setup
```

意圖：放進 pre-commit hook、CI gate 或 hook chain，讓壞 diff
過不了你在意的那道邊界。可以直接貼的範例見
[`docs/integrations/`](docs/integrations/)。

### LLM 驅動的多輪 pipeline

`aegis pipeline run` 在你的 workspace 上跑 Planner → Validator
→ Executor 迴圈。Provider 設定走環境變數——支援任何 OpenAI
相容端點（OpenAI、OpenRouter、Groq）：

```bash
export AEGIS_PROVIDER=openai          # 或：openrouter | groq
export AEGIS_MODEL=gpt-4o-mini        # 該 provider 暴露的任何模型
export OPENAI_API_KEY=sk-...          # 或 AEGIS_API_KEY（不綁特定 provider）

aegis pipeline run \
  --task "rename the foo helper to bar everywhere" \
  --root . \
  --max-iters 3
```

每一輪會印一行摘要（`iter 0 [abc12345] plan=continuing
patches=2 applied=true rolled_back=false`）；加 `--json` 可以
在結束時拿到機器可讀的摘要。當 planner 宣告完成、發出
stalemate、發出 thrashing、或撞到 `--max-iters` 時迴圈會停。
如果結構訊號在迴圈中變差，成本感知的退化回滾會自動觸發。

### MCP server（Cursor / Claude Code / 你自己的代理）

```bash
aegis-mcp     # stdio JSON-RPC，MCP protocol 2025-06-18
```

依 [`docs/integrations/mcp_design.md`](docs/integrations/mcp_design.md)
設定你的 MCP client；之後代理會在迴圈中呼叫
`validate_change(path, new_content, old_content?)` 拿回一個
`{decision, reasons, signals_after, …}` 的裁決。純觀察——
絕不指導代理（見 [Design principles](#設計原則)）。

---

## 整合

你已經在用 Cursor / Claude Code / Aider / Copilot / 你自己的
代理。Aegis 設計成是個**側通道強制執行層**，不會要你換工具。

| 邊界 | 路徑 | 狀態 |
| :--- | :--- | :--- |
| Commit | [Git pre-commit hook](docs/integrations/git_pre_commit.md) | ✓ ready（5 行 bash）|
| PR / merge | [GitHub Action / CI gate](docs/integrations/github_action.md) | ✓ ready（10 行 YAML）|
| 代理決策 | [MCP server](docs/integrations/mcp_design.md) | ✅ `validate_change` 已就緒（`cargo install --path crates/aegis-mcp && aegis-mcp`）|

挑符合你工作流程的邊界用；可以疊。索引 + 各路徑細節：
[`docs/integrations/`](docs/integrations/)。

---

## 狀態

| 層 | 狀態 | 備註 |
| :--- | :--- | :--- |
| Execution Engine | ✅ | Pipeline + Executor + 成本感知退化回滾。原生 Rust 迴圈在 `aegis-runtime::native_pipeline`。|
| Static analysis | ✅ | Ring 0（語法）+ Ring 0.5（14 個分嚴重度的訊號）+ Ring 0.7（10 條安全規則 `SEC001`–`SEC010`）+ Ring R2（跨檔 cycle / 公開符號刪除 / 檔案角色 + z-score），共用於 `aegis check` + `aegis scan` + `aegis attest` + `aegis pipeline run` + `aegis-mcp validate_change` / `validate_change_with_workspace`。|
| Decision Trace | ✅ | `DecisionTrace` + 10 種 `DecisionPattern` + 5 種 `TaskVerdict`；Python 時期的跨模型證據在 [`docs/v1_validation.md`](docs/v1_validation.md)。Rust 重新驗證受 LLM API 預算所限（V1.8）。|
| MCP server | ✅ | `aegis-mcp` —— 自行實作的 JSON-RPC 2.0 over stdio；只暴露一個工具：`validate_change`，依 [`docs/integrations/mcp_design.md`](docs/integrations/mcp_design.md)。|
| Cross-model sweep harness | 🟡 | `aegis pipeline run` 可逐一場景跑；批次 sweep（`aegis sweep`）是 V1.8 backlog —— 受 API 預算所限。|
| Feedback Layer | ❌ | 設計上即非目標 —— 見 [Non-goals](#非目標) 跟 [Critical Principle](docs/gap3_control_plane.md#critical-principle)。由 `crates/aegis-decision/tests/contract.rs` 結構性強制執行。|

### 支援的原始碼語言（Ring 0 + Ring 0.5 訊號）

Tier 2 的多語言支援在 Rust port 的 V1.4–V1.7 落地（見
[`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md)）。
隨著 V1.10 的 Python 刪除，下表列出的每個語言都同時拿到
**強制執行**（`aegis check` 的 Ring 0 + Ring 0.5）跟**重構**
那一半（`aegis pipeline run` 接 OpenAI 相容 LLM provider）。
執行 `aegis languages` 看當前註冊狀況。

| 語言 | Ring 0 語法 | Ring 0.5 fan-out | Ring 0.5 chain-depth | 副檔名 |
| :--- | :---: | :---: | :---: | :--- |
| Python | ✅ | ✅ | ✅ | `.py`、`.pyi` |
| TypeScript | ✅ | ✅ | ✅ | `.ts`、`.tsx`、`.mts`、`.cts` |
| JavaScript | ✅ | ✅ | ✅ | `.js`、`.mjs`、`.cjs`、`.jsx` |
| Go | ✅ | ✅ | ✅ | `.go` |
| Java | ✅ | ✅ | 🟡 | `.java` |
| C# | ✅ | ✅ | ✅ | `.cs` |
| PHP | ✅ | ✅ | ✅ | `.php`、`.phtml`、`.php5`、`.php7`、`.phps` |
| Swift | ✅ | ✅ | ✅ | `.swift` |
| Kotlin | ✅ | ✅ | ✅ | `.kt`、`.kts` |
| Dart | ✅ | ✅ | 🟡 | `.dart` |
| Rust | ✅ | ✅ | ✅ | `.rs` |

🟡 = 預設的 chain-depth walker 在這個語言的 AST 形狀上會少算；
計畫的修法路徑是逐語言 override（`LanguageAdapter::max_chain_depth`）。

要新增一個語言，只需要：一個 Cargo dep + 一個 adapter 檔案
放在 `crates/aegis-core/src/ast/languages/` + 一個 `.scm`
query —— 逐語言 checklist 在
[`docs/multi_language_plan.md#per-language-work-checklist`](docs/multi_language_plan.md#per-language-work-checklist)。

---

## 理念

> 如果 Aegis 開始自動學習，
> 它已違反自身的設計。

---

## 授權

MIT —— 見 [`LICENSE`](LICENSE)。

V1.10 —— Rust workspace，執行階段零 Python。跨平台 release
artifacts（Homebrew、npm、GitHub Releases）的模板放在
[`packaging/`](packaging/)；啟動是
[`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md)
中 V2.0 的里程碑。
