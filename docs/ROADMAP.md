# Aegis Control Plane — Roadmap

> **North Star** — Aegis 的正確性來自對行為的控制與對決策路徑的驗證，而不是對輸出內容的檢查。

---

## 0. 現況：四步地基（已完成）

控制平面的觀察性與控制性基礎已經到位。從這裡開始，每加一層判斷都有可量測基準。

| 步驟 | Commit | 帶來的能力 |
| :--- | :--- | :--- |
| 1 | [`e977f23`](https://github.com/wei9072/aegis/commit/e977f23) | **DecisionTrace** — 每個 gate 的決定變成 first-class data |
| 2 | [`42d7356`](https://github.com/wei9072/aegis/commit/42d7356) | **Side-effect 單一真實來源** — LLM 沒有直接寫檔通道，必須走 Executor |
| 3 | [`ec2ebb5`](https://github.com/wei9072/aegis/commit/ec2ebb5) | **Per-request tool surface** — Tool list 從 session 屬性變成 request 函式 |
| 4 | [`d1e058d`](https://github.com/wei9072/aegis/commit/d1e058d) | **Eval Harness** — 10 scenarios 對 trace 事件序列做斷言，`aegis eval` 失敗 exit 1 |

執行驗證：

```bash
pytest                    # 122 tests passed
aegis eval                # 10/10 scenarios passed
```

---

## 1. 三個能力軸

四步地基是讓 Aegis 開始能「看見決定」。下一階段是讓 Aegis 開始**做出決定**。所有後續層級都對應到三個正交能力軸：

| 軸 | 待補層級 | 已知缺口（GAP scenario） |
| :--- | :--- | :--- |
| **State validation**（看見現實） | ToolCallValidator | scenario 10：hallucinated side-effect |
| **Intent reasoning**（理解語意） | Intent classification + bypass detection | scenario 9：syntax-bypass via string literal |
| **Delivery control**（讓判斷被感知） | Delivery layer + Policy engine | scenarios 4, 5：fan-out / chain depth 觀察到但無人理 |

層級不是線性堆疊，而是 dataflow：ToolCallValidator 的 trust score 回頭影響 policy；policy 調節 intent 的權重；delivery 形式回過來影響下一輪 LLM。階層只是教學上的方便。

---

## 2. 後續層級（按建置順序）

### Layer 5 — ToolCallValidator（state validation 補完）

**問題**：scenario 10 顯示 LLM 可以聲稱「我創建了 fibonacci.py」而 Aegis 完全看不到那是真是假。
**動作**：每次有可能改變外部狀態的 turn 結束後，比對「LLM 宣稱發生了什麼」與「Executor 實際做了什麼」。

**Tier 拆分**（本層的關鍵設計）：

| Tier | 性質 | 範圍 | 成本 |
| :--- | :--- | :--- | :--- |
| **Tier-1**（先做） | Deterministic | Executor 是否寫了？檔案是否存在？ | 0 token |
| **Tier-2**（後做） | Semantic | LLM 描述的內容是否與實際寫入一致？ | 一次 LLM 推理 |

> Tier-2 與 [Layer 9 intent-bypass detection](#layer-9--intent-bypass-detection) 本質上是同一個語意比對問題。**設計時必須讓 Tier-2 與 Layer 9 共用同一個比對引擎**，否則會出現兩套行為各異的 semantic checker。

**API 草圖**：

```python
class ToolCallValidator:
    def validate(self, claim: str, executor_result: ExecutionResult,
                 trace: DecisionTrace) -> None:
        # Tier-1
        if claim_implies_write(claim) and not executor_result.touched_paths:
            trace.emit("toolcall", "block", reason="hallucinated_claim_no_write")
            return
        # Tier-2 (later)
        if not self.semantic_aligns(claim, executor_result):
            trace.emit("toolcall", "warn", reason="claim_state_mismatch")
```

**GAP 收口**：scenario 10 的 `expected_events` 加上 `_E("toolcall", "block", "hallucinated_claim_no_write")`。

---

### Layer 6 — Delivery Layer

**問題**：scenarios 4 / 5 顯示 fan-out=15、chain depth=5 都被 Ring 0.5 看到，但 signal 跟程式碼擠在同一個輸出通道，下一輪 LLM 把它當 context 吞掉、使用者 scroll 過去就消失。

**動作**：把 critique / warning 從程式碼通道**抽離**：

- 結構化 banner 在回應**最前**輸出
- Code block 跟 critique block 必須能在程式上分離（例如不同 markdown 區段、不同 stream channel）
- 序列化版本（給下一輪 LLM 看）跟人類版本（給使用者看）明確區分

**設計約束**：delivery layer 不做判斷，只負責呈現。判斷在 policy。

**為什麼放在 ToolCallValidator 之後**：等 ToolCallValidator 也會 emit `block / warn` 之後，delivery layer 一次處理「所有 layer 的 warning 怎麼呈現」，而不是只處理 Ring 0.5 一種。

---

### Layer 7 — Policy Engine

**問題**：到目前為止，所有「該不該擋」的邏輯都散在各層 hardcoded（Ring 0 寫死語法錯誤要擋、Ring 0.5 寫死只觀察）。新增規則就要改舊層，違反 OCP。

**動作**：把 signals + state validation results 餵進 policy，policy 輸出四種結果：

```python
class Decision(Enum):
    ALLOW = "allow"
    WARN = "warn"               # 走 delivery 顯眼通道，但放行
    CONTEXTUAL_ALLOW = "contextual_allow"  # 放行但強制標示「教學/示範」
    BLOCK = "block"
```

**規則表初版**（YAML）：

```yaml
policy:
  - when: { layer: ring0_5, signal: fan_out, value_gte: 15 }
    decision: warn
    reason: "high_fan_out_advisory"
  - when: { layer: ring0_5, signal: max_chain_depth, value_gte: 4 }
    decision: warn
    reason: "demeter_violation_advisory"
  - when: { layer: toolcall, decision: block }
    decision: block  # 上層攔截直接 propagate
```

**GAP 收口**：scenarios 4, 5 加上 `_E("policy", "warn", ...)`。

**為什麼放在 delivery 之後**：policy 產出 decision，delivery 才能呈現。順序顛倒會讓 policy 無感。

---

### Layer 8 — Intent Classification（前置 gate）

**問題**：使用者請求性質會影響合理的 tool surface。「教學示範」應該禁止任何寫入，「正常開發」才放開 read-only tools。但 PR 3 的 dynamic tool control 只提供了 mechanism，沒提供 policy。

**動作**：在 prompt 進入 LLM 之前先分類，分類結果驅動 ToolPolicy：

```python
class Intent(Enum):
    NORMAL_DEV = "normal_dev"
    TEACHING = "teaching"
    SUSPICIOUS = "suspicious"

class IntentClassifier:
    def classify(self, prompt: str) -> Intent: ...

class ToolPolicy:
    def tools_for(self, intent: Intent) -> tuple:
        return {
            Intent.NORMAL_DEV: LLM_TOOLS_READ_ONLY,
            Intent.TEACHING: (),
            Intent.SUSPICIOUS: (),
        }[intent]
```

**關鍵不變式**：

> **Intent 只能影響呈現方式，不能影響實際行為。**

即使 intent = TEACHING，policy 仍然應禁止把示範程式碼寫到 `.py` 檔；最多允許輸出 markdown code block。否則 intent 會被當成 escape hatch（jailbreak via 「以教學為目的展示...」）。執行面跟意圖標籤**強制脫鉤**。

---

### Layer 9 — Intent-Bypass Detection（後置 gate）

**問題**：scenario 9 顯示 LLM 在規則上合法（語法正確），語意上仍然完成了「展示錯誤程式碼」這個本應被拒絕的請求。

**動作**：把原始請求跟最終產物送進 semantic comparator，問「這個輸出是否完成了原本應該被擋住的意圖？」

```python
class IntentBypassDetector:
    def detect(self, prompt: str, response: str,
               trace: DecisionTrace) -> None:
        if self.semantic_overlap(prompt_intent, response_meaning) > THRESHOLD:
            trace.emit("intent_bypass", "block",
                       reason="semantic_intent_satisfied_via_loophole")
```

**為什麼放最後**：這層需要至少一次額外的 LLM 推理（semantic comparison），是整個系統最貴的判斷。應在所有便宜的 deterministic gate 都到位後才接上，否則它會掩蓋本來該由 cheaper gate 攔截的 case。

**GAP 收口**：scenario 9 的 `expected_events` 加上 `_E("intent_bypass", "block", ...)`。

---

## 3. 跨層議題

### 3.1 Decision Flow Control

當第 N 層判定有問題時，下一動作是什麼？目前 LLMGateway 只有「Ring 0 失敗 → 重試」一條路徑。當 5–9 層都存在時，每一層的 decision verb（`block / warn / contextual_allow / pass`）都需要明確的處置規則：

| Decision | 處置 |
| :--- | :--- |
| `block` | 立即終止這個 turn，不傳給下游 |
| `warn` | 繼續，但走 delivery layer 顯眼通道 |
| `contextual_allow` | 繼續，但強制 metadata 標記（例如 `not_executable=True`） |
| `pass` | 繼續到下一層 |

**這個處置表本身應該是 policy 的一部分**，不是 hardcode 在 LLMGateway 裡。

### 3.2 成本模型

各層的 LLM 呼叫次數跟總延遲：

| Layer | LLM calls | 何時觸發 |
| :--- | :--- | :--- |
| Ring 0 | 0 | 每次 turn |
| Ring 0.5 | 0 | 每次 turn |
| ToolCallValidator Tier-1 | 0 | 每次 turn 後 |
| Policy | 0 | 每個 decision event |
| Delivery | 0 | 每次回應前 |
| Intent classification | 1（前置） | 每次 turn 開頭 |
| ToolCallValidator Tier-2 | 1（事後） | 有寫入 claim 時 |
| Intent-bypass | 1（事後） | 高風險 prompt 才開 |

最壞情況一次 turn = 4 次 LLM 呼叫（含主回應）。需要做 token caching 跟 conditional triggering（不是每個 turn 都跑全部）。

### 3.3 Trace 演進

DecisionTrace 必須**向後相容**。新層加 emit 不能破壞舊 scenario：

- 加新 layer 名稱 → OK（subsequence 比對自動容許）
- 加新 decision verb → 需要 `aegis/runtime/trace.py` 同步擴充常量
- 改現有 reason code → **禁止**（會讓 scenario `expected_events` 突然失效）

每個 PR 都要跑 `aegis eval`，**這是回歸防線**。

### 3.4 Evaluation Harness 的演化

每加一層，至少要：

1. 在 `aegis/eval/scenarios.py` 增加新情境覆蓋這層的特定行為（pass / block / warn / contextual_allow 各一個）
2. 把對應 GAP scenario 的 `expected_events` 補上新的 expected
3. 確認 `pytest` + `aegis eval` 雙綠

> **Definition of Done for any future layer**：emit code 寫了 + 至少一個 GAP scenario 從 TODO 變成 regression assertion + eval green。

---

## 4. 順序總表

```
Foundation       1. DecisionTrace                ✓ done
                 2. side-effect collapse         ✓ done
                 3. dynamic tool control         ✓ done
                 4. eval harness                 ✓ done

State            5. ToolCallValidator Tier-1     ← next
axis             6. ToolCallValidator Tier-2     (after intent layer ready)

Delivery axis    7. Delivery layer
                 8. Policy engine

Intent axis      9. Intent classification (前置)
                10. Intent-bypass detection (後置, semantic)
```

不嚴格線性——8 / 9 可以並行，6 跟 10 共用 semantic engine 應該一起做。但 5 必須在 6 之前、7 必須在 8 之前、3 必須在 9 之前（已完成）。

---

## 5. 何時 Aegis 達成設計目標

當以下三件事都成立：

1. 任何 prompt 進來，trace 都能完整紀錄它走過的 decision path（已達成）
2. 上面 10 個 GAP scenarios 全部從 TODO 變成 regression assertion（layer 5–10 完成後）
3. 加新規則只需動 policy YAML 跟新增情境，不需要動 gate 程式碼

到那時，Aegis 的正確性指標已經完全從「程式碼能否執行」轉移到「decision quality」，eval harness 是唯一可靠的校正機制。
