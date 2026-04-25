# Aegis — Architecture-Aware Control Plane for LLM Code Generation

> 一個由 **Rust 內核** 與 **Python 控制平面** 組成的「行為守門員」。
> 它在 LLM 生成程式碼的每一個回合做兩件事：用硬性規則阻擋語法 / 結構違規（Ring 0），並把每一個決定寫進結構化的 **DecisionTrace**，讓決策路徑變成可驗證的資料。
>
> **North Star** — Aegis 的正確性來自對行為的控制與對決策路徑的驗證，而不是對輸出內容的檢查。

---

## 核心理念

Aegis 把整個系統視為一個帶有回饋迴路的決策系統，沿三個能力軸演進：

| 能力軸 | 已有機制 | 後續層級 |
| :--- | :--- | :--- |
| **State validation**（看見現實） | Side-effect 收斂到 Executor、Ring 0 語法 / 循環依賴 | ToolCallValidator（Tier-1 / Tier-2） |
| **Intent reasoning**（理解語意） | — | Intent classification、Intent-bypass detection |
| **Delivery control**（讓判斷被感知） | Ring 0.5 訊號（同通道附掛） | Delivery layer、Policy engine |

層級不是線性堆疊，而是 dataflow：每一層的 decision 都寫進同一份 trace，eval harness 對 trace 事件序列做斷言。

兩個目前已落地的硬層：

| Ring | 性質 | 動作 | 規則 |
| :--- | :--- | :--- | :--- |
| **Ring 0** | 硬性（Binary） | **阻擋 / SystemExit(1)** | 語法錯誤、模組循環依賴 |
| **Ring 0.5** | 觀察性（Advisory） | emit `observe` 事件 | Fan-out、方法鏈深度（Demeter）、耦合、內聚 |

完整路線圖見 [`docs/ROADMAP.md`](docs/ROADMAP.md)。

---

## 控制平面四步地基

下面四個 PR 已經建立起決策可觀測 + 行為可控的最小骨架：

| 步驟 | Commit | 帶來的能力 |
| :--- | :--- | :--- |
| 1 | [`e977f23`](https://github.com/wei9072/aegis/commit/e977f23) | **DecisionTrace** — 每個 gate 的決定變成 first-class data |
| 2 | [`42d7356`](https://github.com/wei9072/aegis/commit/42d7356) | **Side-effect 單一真實來源** — LLM 沒有直接寫檔通道；寫入只能走 Executor |
| 3 | [`ec2ebb5`](https://github.com/wei9072/aegis/commit/ec2ebb5) | **Per-request tool surface** — Tool list 從 session 屬性變成 request 函式 |
| 4 | [`d1e058d`](https://github.com/wei9072/aegis/commit/d1e058d) | **Eval Harness** — 10 scenarios 對 trace 事件序列做斷言 |

驗證：`pytest`（122 tests）+ `aegis eval`（10/10 scenarios）皆綠。

---

## 架構不變式

下列是程式上強制的（structural test 會擋 PR），不是慣例：

1. **Executor 是檔案系統唯一的寫入者。** LLM 看不到 `write_file`；嘗試把 mutating callable 加進 `LLM_TOOLS_READ_ONLY` 會在 `tests/test_side_effect_isolation.py` 失敗。
2. **每個 request 都產出一份 DecisionTrace。** 由 `LLMGateway.last_trace` 暴露，`aegis eval` 對它做斷言。
3. **Tool list 是 per-request 函式，不是 session 屬性。** GeminiProvider 不快取 chat session，避免未來 intent 層無法切換工具表。
4. **新層加 emit 不能破壞舊 scenario。** Eval harness 用 subsequence 比對，加事件 OK，改 reason code 禁止。
5. **Intent 不能影響 side effects。** 即便未來引入「教學 / 開發」分類，呈現可放鬆，落地一律不放鬆。

---

## 專案結構

```
aegis/
├── aegis/                     # Python 控制平面
│   ├── cli.py                 # CLI 入口（check / generate / refactor / chat / eval）
│   ├── core/                  # Rust 綁定薄封裝（bindings.py）
│   ├── enforcement/           # Ring 0：語法 + 循環依賴；emit pass / block
│   ├── analysis/              # Ring 0.5：訊號層、coupling、demeter、metrics；emit observe
│   ├── graph/                 # 依賴圖服務（build / has_cycle / fan_out）
│   ├── ir/                    # PatchPlan 資料模型、模組名稱正規化
│   ├── runtime/               # Refactor Pipeline、Executor、Validator、攔截器
│   │   └── trace.py           # ★ DecisionEvent + DecisionTrace（pure data）
│   ├── agents/                # LLM Providers + LLMGateway（trace 主織入點）
│   │   ├── llm_adapter.py     #   - LLMProvider Protocol、Gateway、Ring0Validator、SignalContextBuilder
│   │   └── gemini.py          #   - LLM_TOOLS_READ_ONLY、per-request session、runtime mutation guard
│   ├── shared/                # EditEngine（unique-anchor 編輯引擎）
│   ├── tools/                 # 給 LLM 用的 read-only 工具（read_file、list_directory）
│   │                          #   - MUTATING_TOOL_NAMES：結構性 + runtime 雙重黑名單
│   ├── eval/                  # ★ Eval harness
│   │   ├── harness.py         #   - Scenario / ExpectedEvent / run_all
│   │   └── scenarios.py       #   - 10 個內建情境，含 4 個 GAP 標註
│   ├── config/                # policy YAML 載入器與 schema
│   └── daemons/               # 檔案監看、自動修復、增量更新（部分為骨架）
│
├── aegis-core-rs/             # Rust 內核（PyO3 extension：aegis_core_rs）
│   ├── src/
│   │   ├── ast/               # tree-sitter（Python / TypeScript）
│   │   ├── ir/                # 跨語言中繼表示
│   │   ├── graph/             # petgraph 依賴圖與循環偵測
│   │   ├── signals/           # coupling / demeter
│   │   ├── incremental/       # 增量更新器
│   │   └── enforcement.rs     # Ring 0 語法檢查
│   └── queries/               # tree-sitter query（.scm）
│
├── templates/
│   └── default_core_policy.yaml
│
├── docs/
│   └── ROADMAP.md             # ★ 後續層級設計（state / intent / delivery 三軸）
│
├── tests/                     # Pytest 套件（含 trace、isolation、tool control、eval harness）
└── conftest.py
```

---

## 安裝與建置

```bash
# 1. 建立並進入虛擬環境
python -m venv .venv
source .venv/bin/activate

# 2. 安裝 Python 相依套件
pip install click google-genai prompt_toolkit pytest maturin

# 3. 編譯 Rust 內核並安裝到當前虛擬環境
cd aegis-core-rs
maturin develop --release
cd ..

# 4. 確認內核已掛載
python -c "import aegis_core_rs; print(aegis_core_rs.ring0_status())"
# → Ring 0 Rust Core Initialized
```

LLM 功能需要設定 API Key：

```bash
export GEMINI_API_KEY="your-key-here"
```

---

## CLI 使用方式

入口點：`python -m aegis.cli <command>`

### `check` — Ring 0 架構檢查

```bash
python -m aegis.cli check ./my_project
python -m aegis.cli check ./my_project --signals   # 加上 Ring 0.5 訊號
```

失敗時 `exit 1`，可直接掛 CI。

### `generate` — LLM 生成（帶 Ring 0 守門 + DecisionTrace）

```bash
python -m aegis.cli generate "寫一個 LRU cache class" -o lru.py
```

`LLMGateway.generate_and_validate()` 流程：產出 → Ring 0 驗證 → 失敗時帶 violation 重試（最多 3 次）→ 通過後附上 Ring 0.5 觀察。每一步寫進 `gateway.last_trace`。

### `refactor` — 規劃-驗證-執行 重構流水線

```bash
python -m aegis.cli refactor "拆掉 circular dependency" ./my_project \
    --scope aegis/ --max-iters 3
```

流程（`aegis/runtime/pipeline.py`）：

1. **Planner** 收集 signals、依賴圖、檔案內容，產出結構化 `PatchPlan` (JSON)
2. **Validator** 檢查 patch 合法、anchor 唯一、scope 範圍
3. **Executor** 帶備份地原子套用；失敗自動 rollback
4. 重新分析訊號，若**總 signal 數增加** → rollback 整回合
5. 直到 `done=true` 或 `max_iters` 為止

### `chat` — 互動式對話（每輪輸出皆經 Ring 0 驗證）

```bash
python -m aegis.cli chat --model gemini-2.5-flash
```

> ⚠ 自 PR 3 起，每輪是獨立 session（為了配合 per-request tool surface）。需要跨輪記憶請把歷史顯式塞進 prompt。

### `aegis eval` — 對 trace 做行為驗證

```bash
python -m aegis.cli eval            # 簡潔輸出
python -m aegis.cli eval --verbose  # 含事件數、raise 訊息
```

跑 10 個內建情境，斷言 trace 的事件序列是否符合預期。**任一情境失敗 → exit 1**。新增層級時的回歸防線。

---

## 兩個核心流程

### 1. LLMGateway（`aegis/agents/llm_adapter.py`）

```
Prompt ──▶ trace.emit(gateway:request_started)
            │
            ▼
       provider.generate(prompt, tools)        ◀── per-request tool surface
            │
            ▼
       trace.emit(provider:tool_surface)       ◀── 記錄這次的 tool list
            │
            ▼
       Ring0Validator.validate(trace)          ◀── emit ring0:pass / ring0:block
            │
       violations?──Yes──▶ trace.emit(gateway:retry) → 重試
            │
            No
            ▼
       SignalContextBuilder.build_context(trace)  ◀── emit ring0_5:observe 多次
            │
            ▼
       trace.emit(gateway:response_accepted)
            ▼
       回傳安全程式碼，trace 落在 gateway.last_trace
```

### 2. Refactor Pipeline（`aegis/runtime/pipeline.py`）

```
Task ──▶ _build_context ──▶ Planner.plan ──▶ PatchPlan
                                  │
                      Validator.validate (anchors / scope / syntax)
                                  │
                          Executor.apply (backup + atomic)
                                  │
                      重新計算 signals；若 regressed → rollback
                                  │
                          done=true ? → 結束 / 否則進入下一輪
```

> 此流程目前**尚未**完全織入 DecisionTrace，是後續工作項之一（見 [ROADMAP](docs/ROADMAP.md)）。

---

## Rust 內核揭露的 API

透過 `aegis/core/bindings.py` 統一轉出，上層**永遠不直接 import `aegis_core_rs`**：

| 名稱 | 類型 | 用途 |
| :--- | :--- | :--- |
| `check_syntax(path)` | fn | Ring 0 語法檢查 |
| `get_imports(path)` | fn | 取出檔案 import 清單 |
| `extract_signals(path)` | fn | 萃取 Ring 0.5 結構訊號 |
| `build_ir(path)` | fn | 建構中繼表示 |
| `analyze_file(path)` | fn | 綜合 AST metrics |
| `extract_ts_imports(path)` | fn | TypeScript import 抽取 |
| `DependencyGraph` | class | petgraph 驅動的依賴圖 |
| `IncrementalUpdater` | class | 增量更新器 |
| `Signal` / `IrNode` / `AstMetrics` | class | 資料型別 |

---

## DecisionTrace API

每個 gate 都接受可選的 `trace: DecisionTrace`，emit 結構化事件：

```python
from aegis.runtime.trace import DecisionTrace, BLOCK, PASS, OBSERVE

trace = DecisionTrace()
ring0.check_file("main.py", trace=trace)
signal_layer.extract("main.py", trace=trace)

trace.events            # list[DecisionEvent]
trace.by_layer("ring0") # 篩 layer
trace.has_block()       # 任何 layer 是否擋掉
trace.reasons()         # 全部 reason code
trace.to_list()         # JSON-serializable
```

事件 schema：

```python
DecisionEvent(
    layer="ring0" | "ring0_5" | "provider" | "gateway" | ...,
    decision="pass" | "block" | "warn" | "observe",
    reason="syntax_invalid" | "fan_out" | "tool_surface" | ...,
    signals={"fan_out": 15.0},
    metadata={"path": "...", "violations": [...]},
    timestamp=...,
)
```

---

## 測試

```bash
pytest                                    # 全部（122 passed）
pytest tests/test_decision_trace.py -v
pytest tests/test_eval_harness.py -v
pytest tests/test_side_effect_isolation.py -v
pytest tests/test_dynamic_tool_control.py -v
```

關鍵套件：

| 檔案 | 角色 |
| :--- | :--- |
| `test_enforcement.py` | Ring 0 行為（語法、循環依賴） |
| `test_signal_layer.py` | Ring 0.5 訊號 |
| `test_decision_trace.py` | DecisionTrace primitives + 各 gate emit 路徑 |
| `test_side_effect_isolation.py` | 結構性守則：LLM 不可有 mutating tool |
| `test_dynamic_tool_control.py` | Per-request tool surface + runtime guard |
| `test_eval_harness.py` | 10 個內建情境 + harness 自身邏輯 |
| `test_refactor_pipeline.py` | Planner-Validator-Executor 完整流水線 |

---

## 策略檔（`templates/default_core_policy.yaml`）

```yaml
version: "2.0"
enforcement_level: "ring0_only"

ring0:
  syntax_validity:
    enabled: true
    message: "系統內核攔截：程式碼存在致命語法錯誤..."
  anti_circular_dependency:
    enabled: true
    message: "系統內核攔截：偵測到循環依賴 A→B→A..."

ring0_5_signals:
  fan_out_advisory: 15
  max_chain_depth_advisory: 3
```

修改這個檔案即可調整 Aegis 的嚴格度；Ring 0.5 閾值**不會阻擋**，只是當作 LLM 的決策提示。未來 policy engine 上線後（見 [ROADMAP §Layer 7](docs/ROADMAP.md)），閾值將驅動真正的 `policy:warn` 事件而非僅作 advisory。

---

## 擴充方向

- **新語言支援**：在 `aegis-core-rs/src/ast/languages/` 加入對應 tree-sitter grammar，再擴充 `get_imports` / `extract_signals` 分支。
- **新 LLM Provider**：在 `aegis/agents/` 新增 class，實作 `generate(prompt: str, tools: tuple | None = None) -> str`，並暴露 `last_used_tools` 屬性供 gateway 觀察工具使用。
- **自訂 Ring 0 規則**：在 `aegis/enforcement/rules.py` 加新檢查，並在 `Ring0Enforcer.check_project` 串接，記得接 `trace` 參數並 emit 對應事件。
- **新增 eval scenario**：在 `aegis/eval/scenarios.py` 加入 `Scenario(...)`，標 `expected_events`；GAP scenario 必須帶 ≥ 40 字元的 `note` 解釋未來哪一層該介入。

---

## 相關文件

- [`docs/ROADMAP.md`](docs/ROADMAP.md) — 後續層級設計與順序（ToolCallValidator、Delivery、Policy、Intent）
- [`docs/superpowers/`](docs/superpowers/) — 早期內部設計筆記

---

## License

尚未指定。
