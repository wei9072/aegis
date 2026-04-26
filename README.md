# Aegis — Architecture-Aware Control Plane for LLM Code Generation

> **Aegis does not try to make AI produce better code.**
> **It ensures worse outcomes are not accepted.**
>
> Aegis 不教模型怎麼寫，只確保變糟的結果不會被保留。
>
> 機制核心是 cost-aware regression rollback + 結構不變式 enforcement at module
> boundaries：每一個 LLM 回合都跑過完整的 dataflow
> `signal → policy → decision → action → trace`。結構性 gate（Ring 0 / 0.5）擷取訊號，
> policy 把訊號翻譯成 `allow / warn / block`，delivery 用獨立通道把 warning 顯示給人類
> （不汙染下一輪 LLM context），toolcall validator 比對 LLM 自述 vs Executor 實際寫入。
> 多輪重構場景下，pipeline iteration 透過 cost-aware regression detection 評估自己的工作，
> 必要時 rollback。**每一個 gate 都是否決閘，沒有任何一個是目標閘。**
>
> 三層 decision language：
>
> - **Layer A** — per-gate trace event（`PASS / BLOCK / WARN / OBSERVE`）
> - **Layer B** — per-iteration `DecisionPattern`（七種命名形狀）
> - **Layer C** — per-task `TaskVerdict`（V2 Gap 2 引入）
>
> Layer A+B 完成 = **code-state safety harness**。Layer C 引入 = **task outcome 層**。
>
> 完整 framing 與 V1 evidence 見 [`docs/v1_validation.md`](docs/v1_validation.md)。
> 任何新設計都必須通過這個檢核：**這個設計是在拒絕變糟，還是在導向變好？**
> 如果是後者——方向錯了。
>
> Licensed under [MIT](LICENSE). v0.x — interface is stable, package
> structure (`pyproject.toml` / PyPI wheels) is not yet.

---

## 30-second quickstart

```bash
git clone <this-repo> aegis && cd aegis

# Aegis ships a Rust extension for fast structural-signal extraction.
# `pip install -e .` builds it via maturin (prebuilt wheels coming
# soon — until then this step needs Rust toolchain installed).
pip install -e .

export GEMINI_API_KEY=...   # or GOOGLE_API_KEY / OPENROUTER_API_KEY / GROQ_API_KEY

PYTHONPATH=. python examples/02_gateway_single_call.py
```

Expected output: a Python function plus a 7-line decision trace
showing every gate that fired (Ring 0 syntax, IntentClassifier,
PolicyEngine, etc.). That trace IS the product — Aegis surfaces every
decision its gates make as machine-readable data, not as opaque
LLM behavior.

---

## Programmatic usage (this is the product surface)

The `aegis` CLI is a thin wrapper. The real interface is the Python
library — you embed Aegis as a control plane around your own LLM
agent or workflow. Three core patterns:

**Multi-turn refactor with task-level verification:**

```python
from aegis.runtime import pipeline
from aegis.agents.gemini import GeminiProvider
from aegis.runtime.task_verifier import VerifierResult

class MyVerifier:
    def verify(self, workspace, trace):
        # User-defined: what does "task done" mean here?
        return VerifierResult(passed=..., rationale="...")

result = pipeline.run(
    task="reduce fan_out in service.py to under 5",
    root="./project",
    provider=GeminiProvider(),
    verifier=MyVerifier(),
    on_iteration=lambda ev: print(ev.iteration, ev.decision_pattern.value),
)

print(result.task_verdict.pattern.value)   # SOLVED / INCOMPLETE / ABANDONED
```

**Single-call gateway (wrap any one LLM completion):**

```python
from aegis.agents.llm_adapter import LLMGateway

gateway = LLMGateway(llm_provider=GeminiProvider())
safe_code = gateway.generate_and_validate(prompt="...")
# Returns text only if every gate passed — else retries up to max_retries.
for ev in gateway.last_trace.events:
    print(ev.layer, ev.decision, ev.reason)
```

**No-LLM structural lint:**

```python
from aegis.enforcement.validator import Ring0Enforcer
from aegis.analysis.signals import SignalLayer

violations = list(Ring0Enforcer().check_file("src/foo.py"))
signals = SignalLayer().extract("src/foo.py")  # fan_out, max_chain_depth, ...
```

Runnable copies of all three patterns + a custom-verifier walkthrough:
[`examples/`](examples/).

---

## Core philosophy

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
| **V1** | Multi-turn decision system — DecisionPattern + cost-aware rollback + scenario CLI | syntax_fix / fanout_reduce / lod_refactor / regression_rollback |

ROADMAP §7 中尚未動的只有 §4.3 Adaptive Policy（trust-score / cross-layer reasoning），刻意保留——它需要更多 multi-turn 真實流量資料才有 motivation。V1 階段先把 4 個 decision pattern 的 evidence 蒐集起來，Adaptive 才有資料可學。

驗證：`pytest`（215 tests）+ `aegis eval`（15/15 scenarios）雙綠。

---

## V1 — Decision System

Phase 1–3b 是 **control plane components**：每個 gate 各司其職、emit 各自的 trace event。但「Aegis 是一個 decision system」這個 claim 直到 multi-turn evidence 落地才真正成立——一個 gate 可以 emit decisions，loop 才會做決策。V1 補的就是這條 loop。

### 完整 narrative

| 步驟 | Commit | 揭露 / 補上的能力 |
| :--- | :--- | :--- |
| 1. observe signals | `ec6507d` | 建 multi-turn scenario harness — 第一個 scenario `syntax_fix` 揭露 Planner ↔ matcher byte-concat 契約 bug |
| 2. fix contract | `1849ef6` | edit-engine 加 line-aware fallback joiner — multi-turn 不再被 anchor mismatch 卡住 |
| 3. instrument iteration | `b7915e7` | `IterationEvent` 加 value-aware signal tracking（instance count vs cost sum）— `fanout_reduce` 揭露這 gap |
| 4. forward decision | `ee30088` | `lod_refactor` 第一次看到 ctx.previous_errors 流回 planner prompt → 下一輪 plan_id + patch shape 改變 → 收斂。注意：可觀察到的是「上一輪結果改變了下一輪輸出」，而非「LLM 真的在 reasoning」——後者不在 V1 claim 內 |
| 5. render trace | `37a6c66` | `aegis scenario run` 印出 Plan / Strategy / Validation / Apply / Signals / Decision 六段 narrative — system 從「能做決策」變成「決策可被人讀」 |
| 6. backward decision | `9a3521b` | `regression_rollback` 第一次看到 system **否決自己的工作** — applied → rolled back，下一輪 strategy 引用 previous_regressed |
| 7. name pattern | `9dca962` | `DecisionPattern` enum + `derive_pattern` — 七種決策形狀升成 first-class data。scenario 可以斷言 pattern path |
| 8. correct boundary | `e38ce0b` | `_regressed` 從 instance count 換成 cost sum；`regression_detail` 餵進 planner prompt — rollback 從 heuristic 升成 decision，LLM 從 trial-and-error 變 error-informed |

### Decision vocabulary（七個 pattern）

| Pattern | 觸發條件 | Scenario evidence |
| :--- | :--- | :--- |
| `APPLIED_DONE` | applied + plan_done + not rolled_back | syntax_fix / fanout_reduce / lod_refactor iter 1 |
| `APPLIED_CONTINUING` | applied + not plan_done | (multi-iter forward, 尚無 single-scenario evidence) |
| `REGRESSION_ROLLBACK` | applied + rolled_back + regressed (cost grew) | regression_rollback iter 0/1/2 |
| `EXECUTOR_FAILURE` | rolled_back + not regressed | (executor mid-failure, 尚無 evidence) |
| `SILENT_DONE_VETO` | plan_done + patches present + validator vetoed | lod_refactor iter 0 |
| `VALIDATION_VETO` | not validation_passed (no silent_done) | (純 anchor-fail re-plan, 尚無 evidence) |
| `NOOP_DONE` | plan_done + 0 patches | (pipeline short-circuit) |

四個 scenario 覆蓋三個觀測過的 pattern；其餘三個是 derivation 推得到、暫無 real-traffic evidence 的 corner case。Scenario 的 `expected_patterns` 用於 machine-checkable 斷言：`regression_rollback` 必須出現至少一次 `REGRESSION_ROLLBACK`，否則表示 rollback 路徑沒有被觸發、scenario 設計失效。

### Cost-aware regression — 從 heuristic 到 decision

`_regressed(before, after)` 的設計：

```python
def _total_cost(signals): return sum(sig.value for ... in signals)
def _regressed(before, after): return _total_cost(after) > _total_cost(before)
```

不再 count Signal instances（拆 file 必然增加 instance），而是 sum value（拆 file 不會無端讓 fan_out / chain depth 升）。當 cost 真的升，`_regression_detail` 列出每個 kind 的 delta，這個 dict 流進 `PlanContext.previous_regression_detail`，Planner prompt 引用具體訊號：

```
Previous plan APPLIED but was reverted because the post-apply
total cost rose (regression). Specifically:
  - max_chain_depth value increased by +2
Try a different approach that keeps these costs non-increasing.
Note: adding a new file with all-zero signals does NOT count as
regression — only growth in actual signal values does.
```

這是「mechanism → decision」的 qualitative jump：rollback 不再是「signal 變多就回」，是「成本真的變差就回，並告訴 LLM 哪個成本變差了」。

### Scenario CLI — 讓 decision 可被人讀

```bash
aegis scenario list                        # enumerate available scenarios
aegis scenario run lod_refactor            # drive end-to-end with gemma-4-31b-it
aegis scenario run lod_refactor --model gemini-2.5-flash
```

每一輪輸出六段 narrative + 收斂後輸出 decision path：

```
▶ Iteration 0
  Plan          3e4e1a58  (2 patches)
  Strategy      Introduce delegation methods on User and Profile
  Validation    failed (4 errors)
                · simulate_not_found: edit 0 not_found
  Apply         skipped (validation failed)
  Decision      validator vetoed plan_done=true (patches present
                but anchors did not match) — pipeline replans

▶ Iteration 1
  Plan          233e00c1  (3 patches)
  ...
  Signals       max_chain_depth 4 → 2  ⬇ -2
  Decision      applied and planner declared done — task complete

✓ Converged after 2 iterations, 265.1s total
  Decision path: silent_done_veto → applied_done
  Expected patterns met: silent_done_veto, applied_done
```

`tests/scenarios/<name>/runs/<timestamp>__<model>.json` 是 machine-readable 副本，含每個 IterationEvent 的完整 dump（regression_detail、plan_id、signal value totals 等），供 run-to-run 比較。

### 一句話收斂

> **Aegis 不只是自動修改程式碼，它會觀察結果、判斷是否變差、回滾錯誤決策、並調整下一步行動。**

四個 scenario 各自貢獻這句話的一個動詞：
- 觀察結果 → `fanout_reduce`（gradient improvement 可被量測）
- 判斷是否變差 → `regression_rollback`（cost-aware regression 真的會 fire）
- 回滾錯誤決策 → `regression_rollback`（applied → rolled back 真實發生）
- 調整下一步行動 → `lod_refactor`（previous_errors 進入 planner prompt → 下一輪 plan 結構改變；只 claim 資料管道，不 claim LLM reasoning 因果）

### V1 邊界（明確不在 V1 內）

V1 證明的是 system 存在，不是 system 完美。已知 follow-up：

- **Finding C** — pipeline 沒偵測「連續 N 輪不同 plan 但都 regression rollback」的 stalemate，目前撞 max_iters 才停。下版補。
- **Finding D** — `regression_rollback` 的 split-file 重構在這個 codebase 上會讓 `max_chain_depth +2`，每輪都一樣；可能是 Rust extractor 對 import structure 的計算特性，也可能是真實 capability gap。下版調查。
- **Adaptive Policy（ROADMAP §4.3）** — trust score / cross-layer reasoning。要等 V1 累積更多 decision path 資料才有訓練 / 設計依據。

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
├── scripts/                   # Standalone runners（不掛 pytest，會打真 LLM）
│   ├── dogfood.py             # 5-probe single-turn dogfood
│   └── run_scenario.py        # multi-turn scenario runner（也可用 aegis scenario run）
│
├── tests/                     # Pytest 套件（trace / isolation / tool control / eval harness / pattern）
│   └── scenarios/             # ★ V1 multi-turn scenarios（real LLM, 不 auto-collect）
│       ├── _runner.py         #   - MultiTurnScenario / run_scenario / print_trajectory / dump_run
│       ├── syntax_fix/        #   - APPLIED_DONE baseline
│       ├── fanout_reduce/     #   - APPLIED_DONE baseline (gradient)
│       ├── lod_refactor/      #   - SILENT_DONE_VETO → APPLIED_DONE (forward decision)
│       └── regression_rollback/  #   - REGRESSION_ROLLBACK (backward decision)
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

### `aegis scenario` — multi-turn decision narrative

```bash
python -m aegis.cli scenario list                # list available scenarios
python -m aegis.cli scenario run lod_refactor    # drive against real LLM
python -m aegis.cli scenario run regression_rollback --model gemini-2.5-flash
python -m aegis.cli scenario run syntax_fix --no-save  # skip JSON snapshot
```

`aegis scenario run` 用真 LLM 跑 multi-iter Refactor Pipeline，印出每一輪的 Plan / Strategy / Validation / Apply / Signals / Decision 六段 narrative，並把結構化結果寫到 `tests/scenarios/<name>/runs/<timestamp>__<model>.json`。**這個命令會花 LLM token，CI 不該掛這個**——deterministic baseline 用 `aegis eval`，這個是 product-grade observability。

預設 model 是 `gemma-4-31b-it`（dogfood finding：Gemma 系列比 Gemini 更容易產生需要 rollback 的真實 case，更能壓測 control plane）。詳見 [V1 — Decision System](#v1--decision-system)。

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
pytest                                    # 全部（215 passed）
pytest tests/test_decision_trace.py -v
pytest tests/test_eval_harness.py -v
pytest tests/test_side_effect_isolation.py -v
pytest tests/test_dynamic_tool_control.py -v
pytest tests/test_decision_pattern.py -v       # V1: pattern derivation
pytest tests/test_regression_detection.py -v   # V1: cost-based regression
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
| `test_decision_pattern.py` | V1：DecisionPattern 七種 derivation + boundary（silent_done vs noop_done） |
| `test_regression_detection.py` | V1：cost-aware `_regressed` 跟 `_regression_detail` 邊界 |

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
