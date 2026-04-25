# Aegis — Architecture-Aware Control Plane for LLM Code Generation

> 一個由 **Rust 內核** 與 **Python 控制平面** 組成的「行為守門員」。
> 每一個 LLM 回合都跑過完整的 dataflow `signal → policy → decision → action → trace`：
> 結構性 gate（Ring 0 / 0.5）擷取訊號，policy 把訊號翻譯成 `allow / warn / block`，
> delivery 用獨立通道把 warning 顯示給人類（不汙染下一輪 LLM context），
> toolcall validator 比對 LLM 自述 vs Executor 實際寫入，
> intent 層分類 prompt 並偵測語意 bypass。
>
> **North Star** — 正確性的定義是 **decision correctness**，不是 output correctness。

---

## 核心理念

Aegis 把整個系統視為一個帶有回饋迴路的決策系統，沿三個能力軸演進：

| 能力軸 | 機制 | 收口的 GAP |
| :--- | :--- | :--- |
| **State validation**（看見現實） | Side-effect 收斂到 Executor、Ring 0、ToolCallValidator Tier-1（path）+ Tier-2（content） | scenario 10 + 13/14 |
| **Intent reasoning**（理解語意） | IntentClassifier（前置 keyword）+ IntentBypassDetector（後置 semantic） | scenario 09 + 11/15 |
| **Delivery control**（讓判斷被感知） | PolicyEngine（rule table）+ DeliveryRenderer（雙通道） | scenarios 04/05 + 12 |

每一層的 decision 都寫進同一份 trace，eval harness 對 trace 事件序列做斷言。

落地的層級：

| Layer | 性質 | 動作 | 規則 / 條件 |
| :--- | :--- | :--- | :--- |
| **Ring 0** | 硬性（Binary） | **阻擋 / SystemExit(1)** | 語法錯誤、模組循環依賴 |
| **Ring 0.5** | 觀察性 | emit `observe` 事件 | Fan-out、方法鏈深度、耦合、內聚 |
| **PolicyEngine** | 規則驅動（無 LLM） | emit `policy:warn / block` | 預設規則：`fan_out ≥ 10 warn / ≥ 20 block`、`max_chain_depth ≥ 5 warn` |
| **DeliveryRenderer** | 純呈現 | emit `delivery:observe warning_surfaced` | 雙通道隔離（human banner / LLM 純 code） |
| **ToolCallValidator Tier-1** | 確定性（無 LLM） | emit `toolcall:block` | 寫檔宣稱詞 + path-like token + executor 是否真寫 |
| **ToolCallValidator Tier-2** | 語意（1 LLM call） | emit `toolcall:pass / block` | LLM 描述 vs `ExecutionResult.path_contents` 語意對齊 |
| **IntentClassifier** | 確定性（無 LLM） | emit `intent:observe normal_dev/teaching/adversarial` | 中英 phrase 列表，adversarial 優先於 teaching |
| **IntentBypassDetector** | 語意（1 LLM call，後置） | emit `intent_bypass:pass / block` | 只在 teaching/adversarial 跑，比對 prompt rejection-target 與 response |

完整路線圖見 [`docs/ROADMAP.md`](docs/ROADMAP.md)。

---

## 已完成 phase

四步地基讓「決策可觀測」就位；Phase 1–3b 把它推到「決策可控制」。

| Phase | 主題 | 收口 scenarios |
| :--- | :--- | :--- |
| Foundation（1–4） | DecisionTrace、Side-effect 收斂、per-request tool surface、Eval harness | — |
| **Phase 1** | PolicyEngine + DeliveryRenderer + decision→action mapping | 04, 05 |
| **Phase 2** | ToolCallValidator Tier-1 + IntentClassifier | 10, 11, 12 |
| **Phase 3a** | IntentBypassDetector + SemanticComparator + GeminiSemanticComparator（LLM-backed） | 09, 15 |
| **Phase 3b** | ToolCallValidator Tier-2（重用同一個 SemanticComparator） | 13, 14 |

ROADMAP §7 中尚未動的只有 §4.3 Adaptive Policy（trust-score / cross-layer reasoning），刻意保留——它需要 multi-turn 真實流量資料才有 motivation，目前 dogfood 都是 single-turn。

驗證：`pytest`（194 tests）+ `aegis eval`（15/15 scenarios）雙綠。

---

## 架構不變式

下列是程式上強制的（structural test 會擋 PR），不是慣例（對應 ROADMAP §5 七條 invariants）：

1. **Executor 是檔案系統唯一的寫入者。** LLM 看不到 `write_file`；嘗試把 mutating callable 加進 `LLM_TOOLS_READ_ONLY` 會在 `tests/test_side_effect_isolation.py` 失敗。
2. **Intent 不能弱化 invariants。** Intent label 只影響呈現，不影響執行——scenario 12 把這條 pin 死（teaching prompt + fan_out=15 仍 emit `policy:warn`）。
3. **每個 decision 都必須可追蹤。** 寫入 `DecisionTrace` 是 mandatory；`aegis eval` 對 trace 序列做斷言。
4. **不允許 silent failure / silent pass。** 沒有 emit 的決定 = bug——IntentBypassDetector 與 ToolCallValidator Tier-2 在 pass 時也 emit。
5. **PolicyEngine 是唯一的 decision verb 來源。** 其他層只能 observe + emit raw event；policy 把 signal 翻譯成 verb。
6. **Decision phase 不讀 live state。** Validator 與 policy 操作 caller 提供的 snapshot（`ExecutionResult.path_contents` 等），避免 TOCTOU。
7. **Delivery 雙通道強制隔離。** Human-visible warning（banner）絕不進入 LLM-bound channel，否則 signal 會被 recursive pollution 稀釋。
8. **新層加 emit 不能破壞舊 scenario。** Eval harness 用 subsequence 比對，加事件 OK，改 reason code 禁止。
9. **Tool list 是 per-request 函式，不是 session 屬性。** GeminiProvider 不快取 chat session，per-turn 重新解析 tool surface。

---

## 專案結構

```
aegis/
├── aegis/                     # Python 控制平面
│   ├── cli.py                 # CLI 入口（check / generate / refactor / chat / eval）
│   ├── core/                  # Rust 綁定薄封裝（bindings.py）
│   ├── enforcement/           # Ring 0：語法 + 循環依賴；emit pass / block
│   ├── analysis/              # Ring 0.5：訊號層、coupling、demeter、metrics；emit observe
│   ├── policy/                # ★ PolicyEngine：deterministic SignalRule 驅動的 decision verb 來源
│   ├── delivery/              # ★ DeliveryRenderer：雙通道呈現（human banner / LLM clean code）
│   ├── toolcall/              # ★ ToolCallValidator：Tier-1 path 比對 + Tier-2 semantic content 比對
│   ├── intent/                # ★ IntentClassifier（前置）+ IntentBypassDetector（後置）
│   ├── semantic/              # ★ SemanticComparator Protocol + Stub + GeminiSemanticComparator
│   ├── graph/                 # 依賴圖服務（build / has_cycle / fan_out）
│   ├── ir/                    # PatchPlan 資料模型、模組名稱正規化
│   ├── runtime/               # Refactor Pipeline、Executor、Validator、攔截器
│   │   ├── trace.py           # ★ DecisionEvent + DecisionTrace（pure data）
│   │   └── executor.py        #   - ExecutionResult.path_contents 餵給 Tier-2
│   ├── agents/                # LLM Providers + LLMGateway（trace 主織入點）
│   │   ├── llm_adapter.py     #   - LLMProvider Protocol、Gateway、Ring0Validator、SignalContextBuilder、ExecutionRecorder Protocol
│   │   └── gemini.py          #   - LLM_TOOLS_READ_ONLY、per-request session、runtime mutation guard
│   ├── shared/                # EditEngine（unique-anchor 編輯引擎）
│   ├── tools/                 # 給 LLM 用的 read-only 工具（read_file、list_directory）
│   │                          #   - MUTATING_TOOL_NAMES：結構性 + runtime 雙重黑名單
│   ├── eval/                  # Eval harness
│   │   ├── harness.py         #   - Scenario / ExpectedEvent / run_all + _StubExecutorRecorder
│   │   └── scenarios.py       #   - 15 個內建情境（4 GAP 已收口 + 4 新增覆蓋 Phase 2/3）
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

跑 15 個內建情境，斷言 trace 的事件序列是否符合預期。**任一情境失敗 → exit 1**。新增層級時的回歸防線。

---

## 兩個核心流程

### 1. LLMGateway（`aegis/agents/llm_adapter.py`）

```
Prompt ──▶ trace.emit(gateway:request_started)
            │
            ▼
       IntentClassifier.classify(prompt)       ◀── emit intent:observe normal_dev/teaching/adversarial
            │
            ▼  （retry loop 開始）
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
       ToolCallValidator.validate(code, executor_result, trace)
            │                                  ◀── Tier-1 deterministic; Tier-2 semantic if comparator wired
       block?──Yes──▶ trace.emit(gateway:block toolcall_block) → raise
            │
            No
            ▼
       SignalContextBuilder.build_context(trace)  ◀── emit ring0_5:observe 多次
            │
            ▼
       PolicyEngine.evaluate(trace)            ◀── emit policy:warn / block
            │
       block?──Yes──▶ trace.emit(gateway:block policy_block) → raise
            │
            No
            ▼
       DeliveryRenderer.render(code, verdict)  ◀── emit delivery:observe warning_surfaced
            │                                      （human view 帶 banner / LLM view 純 code）
            ▼
       IntentBypassDetector.detect(prompt, code, intent)  ◀── 只在 teaching/adversarial 跑
            │                                              emit intent_bypass:pass / block
       block?──Yes──▶ trace.emit(gateway:block intent_bypass_block) → raise
            │
            No
            ▼
       trace.emit(gateway:response_accepted)
            ▼
       回傳 view.human，trace 落在 gateway.last_trace
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
pytest                                    # 全部（194 passed）
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
| `test_policy_engine.py` | PolicyEngine deterministic rule eval、threshold edge、custom rules |
| `test_delivery_renderer.py` | 雙通道隔離、banner-before-code、emit warning_surfaced |
| `test_toolcall_validator.py` | Tier-1 path 比對 + Tier-2 semantic comparator |
| `test_intent_classifier.py` | 中英 phrase 分類、adversarial > teaching priority |
| `test_intent_bypass.py` | 只在 teaching/adversarial 跑、threshold inclusive、emit-on-pass |
| `test_semantic_comparator.py` | Stub fixed/mapping 模式、Protocol 一致性 |
| `test_gemini_comparator.py` | JSON parser 邊界、clamp、lazy provider |
| `test_eval_harness.py` | 15 個內建情境 + harness 自身邏輯 |
| `test_refactor_pipeline.py` | Planner-Validator-Executor 完整流水線 |

---

## Dogfood（真實流量）

`pytest` 跟 `aegis eval` 都是 deterministic — fake provider、canned response、stub comparator。要看每一層在真實 LLM 流量下的行為差異，用 `scripts/dogfood.py`：

```bash
PYTHONPATH=. python scripts/dogfood.py                    # 5 probes against gemma-4-31b-it（預設）
PYTHONPATH=. python scripts/dogfood.py --probe C          # 只跑 fan_out probe
PYTHONPATH=. python scripts/dogfood.py --model gemini-2.5-flash
```

5 個 probe 各壓一層：A normal-dev、B teaching、C fan-out=15、D adversarial、E side-effect 幻覺。輸出每個 probe 的 trace 事件序列 + raise / output 摘要，方便比對跨模型行為。

> 預設 `gemma-4-31b-it` 是有理由的：跨模型實測發現 Gemini 對 read-only tool surface 會自我拒絕，Gemma 則會幻覺出 side-effect 宣稱——後者實際觸發 ToolCallValidator Tier-1，是 Aegis 縱深防禦的真實 motivation 來源。

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

這個 YAML 是早期 advisory 設計留下的；Phase 1 已把實際 policy 規則搬進 `aegis/policy/engine.py` 的 `DEFAULT_RULES`（`SignalRule` 列表）。`fan_out_advisory` 等舊欄位現在是文件值，真正驅動 `policy:warn / block` 的是 `DEFAULT_RULES`。要調 threshold 改後者即可，加新規則只需新增 `SignalRule(...)`，不必動 gate 程式碼（這也是 ROADMAP §8 設計目標 condition 3）。

---

## 擴充方向

- **新語言支援**：在 `aegis-core-rs/src/ast/languages/` 加入對應 tree-sitter grammar，再擴充 `get_imports` / `extract_signals` 分支。
- **新 LLM Provider**：在 `aegis/agents/` 新增 class，實作 `generate(prompt: str, tools: tuple | None = None) -> str`，並暴露 `last_used_tools` 屬性供 gateway 觀察工具使用。
- **自訂 Ring 0 規則**：在 `aegis/enforcement/rules.py` 加新檢查，並在 `Ring0Enforcer.check_project` 串接，記得接 `trace` 參數並 emit 對應事件。
- **新增 policy 規則**：在 `aegis/policy/engine.py` 的 `DEFAULT_RULES` 加 `SignalRule(signal_name, threshold, decision, reason)`；不要動 gate 程式碼。新 reason code 也要在 scenarios 補對應斷言。
- **擴充 IntentClassifier phrase 列表**：在 `aegis/intent/classifier.py` 的 `_TEACHING_PHRASES_*` / `_ADVERSARIAL_PHRASES_*` 加 phrase；priority 是 ADVERSARIAL > TEACHING > NORMAL_DEV。
- **替換 SemanticComparator**：實作 `aegis.semantic.comparator.SemanticComparator` Protocol（`compare(a, b, *, context) -> SemanticResult`），即可同時供 IntentBypassDetector 與 ToolCallValidator Tier-2 共用。
- **新增 eval scenario**：在 `aegis/eval/scenarios.py` 加入 `Scenario(...)`，標 `expected_events`；GAP scenario 必須帶 ≥ 40 字元的 `note` 解釋未來哪一層該介入。Tier-2 / IntentBypass 場景可注入 `toolcall_comparator` / `intent_bypass_comparator` + `stub_execution_result` 來模擬 Executor 行為。

---

## 相關文件

- [`docs/ROADMAP.md`](docs/ROADMAP.md) — 後續層級設計與順序（ToolCallValidator、Delivery、Policy、Intent）
- [`docs/superpowers/`](docs/superpowers/) — 早期內部設計筆記

---

## License

尚未指定。
