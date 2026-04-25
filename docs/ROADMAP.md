# Aegis Control Plane — Roadmap

> Aegis 不是 coding assistant。
> Aegis 是 LLM 系統的 **deterministic control layer**。
>
> 正確性的定義是 **decision correctness**，不是 output correctness。

---

## 0. 系統定義

Aegis 的職責是確保：

1. System invariants 永遠不被破壞
2. Side effects 與 real-world state 一致
3. 每個 decision 都是 observable 且 verifiable

### 核心控制迴路（系統不變式）

所有 feature 都必須符合這條 dataflow：

```
signal → policy → decision → action → trace
```

一個 feature **不算完成**，除非：

1. 它產生 signal
2. signal 影響 decision
3. decision 改變行為
4. decision 被寫入 DecisionTrace

這條迴路是 Aegis 的設計憲法，下面所有層級都是它的展開。

---

## 1. 現況：四步地基（已完成）

控制平面的「觀察」與「結構性控制」已經到位。每加一層判斷都有可量測基準。

| 步驟 | Commit | 帶來的能力 |
| :--- | :--- | :--- |
| 1 | [`e977f23`](https://github.com/wei9072/aegis/commit/e977f23) | **DecisionTrace** — 每個 gate 的決定變成 first-class data |
| 2 | [`42d7356`](https://github.com/wei9072/aegis/commit/42d7356) | **Side-effect 單一真實來源** — LLM 沒有直接寫檔通道，必須走 Executor |
| 3 | [`ec2ebb5`](https://github.com/wei9072/aegis/commit/ec2ebb5) | **Per-request tool surface** — Tool list 從 session 屬性變成 request 函式 |
| 4 | [`d1e058d`](https://github.com/wei9072/aegis/commit/d1e058d) | **Eval Harness** — 10 scenarios 對 trace 事件序列做斷言，`aegis eval` 失敗 exit 1 |

執行驗證：

```bash
pytest          # 122 tests passed
aegis eval      # 10/10 scenarios passed
```

### 關鍵缺口：signals 沒有驅動 decisions

當前各能力的真實狀態：

| 能力 | 狀態 |
| :--- | :--- |
| Ring 0（語法、循環依賴） | 會擋（hard enforcement） |
| Ring 0.5（fan_out, chain_depth, ...） | **只觀察**，不影響行為 |
| ToolCall validation | 不存在 |
| Side-effect verification | 不存在 |

具體例子：`fan_out = 15` 被偵測到，沒有 warning、沒有 block、下一輪 LLM 不知道（scenario 04）。`max_chain_depth = 6` 被觀察到，使用者跟 LLM 都看不到任何後續行為（scenario 05）。

換句話說：

- **目前狀態**：observability
- **目標狀態**：control

> Phase 1 的整個目標就是把這個 gap 收掉。

---

## 2. Phase 1 — 把 Aegis 從觀察者變成決策者（MVP）

**目標**：讓至少一個 signal 真的能改變系統行為，並走完整條 control loop。

### 2.1 Policy Engine（deterministic, no LLM）

**輸入**：signals（fan_out, chain_depth, ...）+ 各 layer 的 decision events
**輸出**：`allow / warn / block` 三選一

**初版規則表（YAML）**：

```yaml
policy:
  - when: { layer: ring0_5, signal: fan_out, value_gte: 10 }
    decision: warn
    reason: high_fan_out_advisory
  - when: { layer: ring0_5, signal: fan_out, value_gte: 20 }
    decision: block
    reason: high_fan_out_block
  - when: { layer: ring0_5, signal: max_chain_depth, value_gte: 5 }
    decision: warn
    reason: demeter_violation_advisory
  - when: { layer: toolcall, decision: block }
    decision: block            # 上層攔截直接 propagate
    reason: propagated_block
```

**設計約束**：

- 這層**不打 LLM**，所有判斷都是 deterministic rule eval
- Policy 是**唯一**決定 verb 的地方——其他層只能 emit observation/event
- 規則表是資料，不是程式碼，新增規則不應動 gate code

### 2.2 Decision → Action Mapping

policy 的輸出必須**真的改變行為**，不能是 silent decision：

| Decision | 行為 |
| :--- | :--- |
| `allow` | 回應原樣放行 |
| `warn` | 走 delivery 顯眼通道，回應仍放行 |
| `block` | 立即終止這個 turn，不傳給下游 |

這個對應表本身是 policy 的一部分，**不能 hardcode 在 LLMGateway**。

### 2.3 Delivery Layer（MVP）

**問題**：scenarios 4 / 5 顯示 fan-out=15、chain depth=5 都被 Ring 0.5 看到，但 signal 跟程式碼擠在同一個輸出通道，下一輪 LLM 把它當 context 吞掉、使用者 scroll 過去就消失。

**動作**：

- Warning 必須在 code **之前**出現
- Warning block 與 code block 必須**可在程式上分離**（不同 markdown section / 不同 stream channel）
- 給人類看的版本與序列化給下一輪 LLM 看的版本必須明確區分

**範例輸出格式**：

```
⚠️  Warning: High fan-out detected (15)

  This may indicate:
  - poor modular design
  - unnecessary dependencies

---
（接著才是 code）
```

**設計約束**：delivery 不做判斷，只負責呈現。判斷在 policy。

### 2.4 DecisionTrace 整合

每個 policy decision 與 delivery action 都必須寫入 trace：

```python
trace.emit(
    "policy", "warn",
    reason="high_fan_out_advisory",
    metadata={"signal": "fan_out", "value": 15, "threshold": 10},
)
trace.emit(
    "delivery", "observe",
    reason="warning_surfaced",
    metadata={"channel": "banner", "before_code": True},
)
```

DecisionTrace 是 **evaluation 的 source of truth**——eval harness 只看 trace 序列，不看輸出文字。

### 2.5 Phase 1 範例（完成後長這樣）

User：「Create a service with 15 imports」

Aegis 回應：

```
⚠️  Warning: High fan-out detected (15)

[code output]
```

對應 DecisionTrace 序列：

- `ring0: pass`
- `ring0_5: detect`（fan_out=15）
- `policy: warn`（high_fan_out_advisory）
- `delivery: observe`（warning_surfaced, before_code=True）

### 2.6 Phase 1 Exit Criteria

Phase 1 完成的定義（缺一不可）：

1. 至少一個 signal（推薦 `fan_out`）真的改變系統行為
2. Warning 出現在 code 之前、且在獨立通道
3. `policy:warn` event 被寫入 DecisionTrace
4. Scenarios 04 / 05 的 `expected_events` **加上**：
   - `_E("policy", "warn", "high_fan_out_advisory")`
   - `_E("delivery", "observe", "warning_surfaced")`
   - 對應 chain depth：`_E("policy", "warn", "demeter_violation_advisory")`
5. `pytest` + `aegis eval` 雙綠

> **這就是「signal → policy → decision → action → trace」第一次完整跑通。**
> Aegis 從這刻起才真的是一個 control system。

---

## 3. Phase 2 — State Validation 與 Context Awareness

Phase 1 解決「signal 不驅動行為」。Phase 2 解決「Aegis 看不見現實 + 不分情境」。

### 3.1 ToolCallValidator — Tier-1（deterministic）

**問題**：scenario 10 顯示 LLM 可以聲稱「我創建了 fibonacci.py」而 Aegis 完全看不到那是真是假。

**Tier-1 動作**（deterministic，0 token）：

- LLM 宣稱寫了檔 → 檢查 Executor 是否真的寫了
- LLM 宣稱檔案存在 → 檢查 path
- 防 path escape（security）

```python
class ToolCallValidator:
    def validate(self, claim: str, executor_result: ExecutionResult,
                 trace: DecisionTrace) -> None:
        if claim_implies_write(claim) and not executor_result.touched_paths:
            trace.emit("toolcall", "block",
                       reason="hallucinated_claim_no_write")
```

**為什麼 Tier-1 必須在 Tier-2 前**：deterministic 檢查 0 成本，能擋住大部分的明顯幻覺；Tier-2 是貴的 semantic check，應該只在 Tier-1 沒攔下時才開。

**GAP 收口**：scenario 10 的 `expected_events` 加上 `_E("toolcall", "block", "hallucinated_claim_no_write")`。

> Tier-2（semantic comparison）留到 Phase 3，與 intent-bypass 共用同一個比對引擎。

### 3.2 Intent Classification（前置 gate）

**目的**：分類使用者意圖，**僅影響呈現方式，不影響執行**。

```python
class Intent(Enum):
    NORMAL_DEV = "normal_dev"
    TEACHING = "teaching"
    ADVERSARIAL = "adversarial"

class IntentClassifier:
    def classify(self, prompt: str) -> Intent: ...
```

**MVP 行為**：

| Intent | 影響 |
| :--- | :--- |
| `normal_dev` | 嚴格執行 invariants |
| `teaching` | 允許輸出 invalid example，但**禁止寫入 `.py` 檔**（只能 markdown code block） |
| `adversarial` | log + 套用最嚴格 policy |

**關鍵不變式**：

> **Intent 不能放鬆 invariants。**
> 即使 `intent = teaching`，policy 仍然應禁止把示範程式碼寫到 `.py` 檔；
> 否則 intent 就會被當成 escape hatch（jailbreak via「以教學為目的展示 SQL injection...」）。

執行面與 intent 標籤**強制脫鉤**——這是非協商的設計。

### 3.3 Dynamic Tool Surface 串聯（PR 3 已交付，這裡只做接線）

PR 3 已交付：tool list 是 per-request 函式，Provider 在執行前驗證 mutating tools 不會被暴露給 LLM。

Phase 2 結束時，IntentClassifier 會驅動這個 mechanism：

```python
class ToolPolicy:
    def tools_for(self, intent: Intent) -> tuple:
        return {
            Intent.NORMAL_DEV: LLM_TOOLS_READ_ONLY,
            Intent.TEACHING: (),       # 教學情境不給任何 tool
            Intent.ADVERSARIAL: (),
        }[intent]
```

---

## 4. Phase 3 — Semantic 與 Advanced Control

### 4.1 ToolCallValidator — Tier-2（semantic）

LLM 描述的內容是否與實際寫入一致？這需要一次 LLM 推理。

> Tier-2 與 Intent-Bypass Detection 在本質上是**同一個語意比對問題**。設計時必須共用同一個 comparator engine，否則會出現兩套行為各異的 semantic checker。

### 4.2 Intent-Bypass Detection（後置 gate）

**問題**：scenario 9 顯示 LLM 在規則上合法（語法正確），語意上仍完成了「展示錯誤程式碼」這個本應被拒絕的請求（把 `def bad(` 藏在字串字面量裡）。

```python
class IntentBypassDetector:
    def detect(self, prompt: str, response: str,
               trace: DecisionTrace) -> None:
        if self.semantic_overlap(prompt_intent, response_meaning) > THRESHOLD:
            trace.emit("intent_bypass", "block",
                       reason="semantic_intent_satisfied_via_loophole")
```

**為什麼放最後**：這層需要至少一次額外的 LLM 推理，是整個系統最貴的判斷。應在所有便宜的 deterministic gate 都到位後才接上，否則它會掩蓋本應由 cheaper gate 攔截的 case。

**GAP 收口**：scenario 9 的 `expected_events` 加上 `_E("intent_bypass", "block", ...)`。

### 4.3 Adaptive Policy / Cross-Layer Reasoning

當 ToolCallValidator 開始 emit `block`，Policy 應該把該 session 的 trust score 調低；IntentClassifier 也應該更傾向 `adversarial`。這層讓系統從「規則表」升級為「狀態機」。

**設計準則**：adaptive 行為仍**不能放鬆 invariants**——只能讓 enforcement **更嚴**，不能更鬆。

---

## 5. 系統約束（Non-Negotiable）

無論 Phase 幾，下面七條都是硬約束，所有層級實作不得違反：

1. **LLM 不能直接改變 state** — 所有 side effect 必須走 Executor
2. **Intent 不能弱化 invariants** — 只能影響 presentation
3. **每個 decision 都必須可追蹤** — 寫入 DecisionTrace 是 mandatory
4. **不允許 silent failure / silent pass** — 沒有 emit 的決定 = bug
5. **Policy 是唯一的 decision verb 來源** — 其他層只能 observe + emit raw event
6. **Decision phase 不讀 live state** — 所有 validator 與 policy 操作同一份 immutable snapshot；snapshot 在執行前取得，executor 在 validation 之後才 mutate state（避免 TOCTOU）
7. **Delivery 雙通道強制隔離** — 給人類看的輸出（warning、banner）與餵給下一輪 LLM 的輸出（clean code）必須分離；warning 不可進入 LLM context，否則 signal 會被 recursive pollution 稀釋

這七條是 Aegis 的設計憲法，PR review 的第一道篩網。

---

## 6. 跨層議題

### 6.1 Decision Flow Control

當第 N 層判定有問題時，下一動作是什麼？目前 LLMGateway 只有「Ring 0 失敗 → 重試」一條路徑。當 Phase 1–3 都到位後，每一層的 decision verb 都需要明確處置規則：

| Decision | 處置 |
| :--- | :--- |
| `block` | 立即終止這個 turn，不傳給下游 |
| `warn` | 繼續，但走 delivery layer 顯眼通道 |
| `allow` | 繼續到下一層 |

**這個處置表本身是 policy 的一部分**，不是 hardcode 在 LLMGateway 裡。

**多層 decision 衝突時的 priority**：

```
block > warn > allow
```

- 任一層 emit `block` → 立即終止這個 turn
- `warn` 不會覆蓋 `block`
- `allow` 只作為 default fallback
- 一個 turn 最終只執行一個 decision

### 6.2 成本模型

各層的 LLM 呼叫次數與觸發條件：

| Layer | LLM calls | 何時觸發 |
| :--- | :--- | :--- |
| Ring 0 | 0 | 每次 turn |
| Ring 0.5 | 0 | 每次 turn |
| Policy | 0 | 每個 decision event |
| Delivery | 0 | 每次回應前 |
| ToolCallValidator Tier-1 | 0 | 每次 turn 後 |
| Intent classification | 1（前置） | 每次 turn 開頭 |
| ToolCallValidator Tier-2 | 1（事後） | 有寫入 claim 時 |
| Intent-bypass | 1（事後） | 高風險 prompt 才開 |

最壞情況一次 turn = 4 次 LLM 呼叫（含主回應）。需要做 token caching 與 conditional triggering（不是每個 turn 都跑全部）。

### 6.3 Trace 演進

DecisionTrace 必須**向後相容**。新層加 emit 不能破壞舊 scenario：

- 加新 layer 名稱 → OK（subsequence 比對自動容許）
- 加新 decision verb → 需要 `aegis/runtime/trace.py` 同步擴充常量
- **改現有 reason code → 禁止**（會讓 scenario `expected_events` 突然失效）

每個 PR 都必須跑 `aegis eval`——**這是回歸防線**。

### 6.4 Evaluation Harness 演化規則

每加一層至少要：

1. 在 `aegis/eval/scenarios.py` 新增覆蓋這層的情境（pass / block / warn 各一個）
2. 把對應 GAP scenario 的 `expected_events` 補上新的 expected
3. 必要時加上 **negative assertion**（斷言某事件**不**發生）防止意外 regression — 例如「allow 路徑不應 emit `policy:block`」
4. 確認 `pytest` + `aegis eval` 雙綠

> **Definition of Done for any layer**：emit code 寫了 + 至少一個 GAP scenario 從 TODO 變成 regression assertion + eval green。

任何 PR 不滿足這四條就**不算完成**。

---

## 7. 順序總表

```
Foundation       1. DecisionTrace                ✓
                 2. Side-effect collapse         ✓
                 3. Dynamic tool control         ✓
                 4. Eval harness                 ✓

Phase 1          5. Policy engine (MVP)          ← next
(MVP control)    6. Delivery layer (MVP)
                 7. Decision→action mapping
                 → scenarios 04, 05 GAP close

Phase 2          8. ToolCallValidator Tier-1
(state + intent) 9. Intent classification (前置)
                 → scenario 10 GAP closes

Phase 3         10. ToolCallValidator Tier-2
(semantic)      11. Intent-bypass detection (後置)
                12. Adaptive policy / cross-layer
                 → scenario 09 GAP closes
```

不嚴格線性——Tier-2 (10) 與 intent-bypass (11) 共用 semantic comparator，應該一起做。但 Phase 1 必須在 Phase 2 之前；Tier-1 必須在 Tier-2 之前；intent classification 必須在 intent-bypass detection 之前。

---

## 8. 何時 Aegis 達成設計目標

當以下三件事都成立：

1. 任何 prompt 進來，trace 都能完整紀錄它走過的 decision path（**已達成**）
2. 10 個 GAP scenarios 全部從 TODO 變成 regression assertion（Phase 1–3 完成後）
3. **加新規則只需動 policy YAML 與新增情境，不需要動 gate 程式碼**

到那時：

- Aegis 的正確性指標完全從「程式碼能否執行」轉移到「decision quality」
- Eval harness 是唯一可靠的校正機制
- 系統定義成立：**deterministic governance layer for non-deterministic systems**

---

## 9. 指導原則

> **Without decision: signal = logging.**
> **With decision: signal = control.**

Aegis 的所有層級設計都是這句話的展開。