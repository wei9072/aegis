# Aegis — Architecture Linter & Code Generator

> 一個由 **Rust 內核** 與 **Python 上層** 組成的「架構守門員」。
> 它在 LLM 生成程式碼的每一個回合進行 **Ring 0（硬性攔截）** 與 **Ring 0.5（結構訊號）** 檢查，把不合格的輸出擋在檔案落地之前。

---

## 核心理念

Aegis 把「程式碼品質規範」劃分成兩個層級，這是整個專案的骨幹：

| Ring | 性質 | 動作 | 規則範例 |
| :--- | :--- | :--- | :--- |
| **Ring 0** | 硬性（Binary） | **阻擋 / SystemExit(1)** | 語法錯誤（Tree-sitter 解析失敗）、模組循環依賴 |
| **Ring 0.5** | 觀察性（Advisory） | 作為上下文回饋給 LLM | Fan-out、方法鏈深度（Law of Demeter）、耦合、內聚 |

> Ring 0 是「不能妥協」，Ring 0.5 是「給 LLM 當決策依據」。只有 Ring 0 會真正阻擋執行。

規則來源：`templates/default_core_policy.yaml`

---

## 專案結構

```
aegis/
├── aegis/                     # Python 上層：流程編排、LLM Agents、CLI
│   ├── cli.py                 # 進入點（Click）
│   ├── core/                  # Rust 綁定薄封裝（bindings.py）
│   ├── enforcement/           # Ring 0 驗證器（語法 + 循環依賴）
│   ├── analysis/              # Ring 0.5 訊號層、耦合、內聚、Demeter、metrics
│   ├── graph/                 # 依賴圖服務（build / has_cycle / fan_out）
│   ├── ir/                    # PatchPlan 資料模型、模組名稱正規化
│   ├── runtime/               # Refactor Pipeline、Executor、Validator、攔截器
│   ├── agents/                # LLM Providers（Gemini / OpenAI / Claude）、Planner、Critic、LLMGateway
│   ├── shared/                # EditEngine（unique-anchor 編輯引擎）
│   ├── tools/                 # 給 LLM function-calling 用的工具（file_system、circuit_breaker、mcp_client）
│   ├── config/                # policy YAML 載入器與 schema
│   └── daemons/               # 檔案監看、自動修復、增量更新（部分為骨架）
│
├── aegis-core-rs/             # Rust 內核（PyO3 extension，模組名 aegis_core_rs）
│   ├── src/
│   │   ├── ast/               # tree-sitter 驅動的 AST parser（Python / TypeScript）
│   │   ├── ir/                # 跨語言中繼表示（IR）
│   │   ├── graph/             # petgraph 依賴圖與循環偵測
│   │   ├── signals/           # coupling / demeter 等 Ring 0.5 訊號
│   │   ├── incremental/       # 增量更新器與 cache
│   │   └── enforcement.rs     # Ring 0 語法檢查
│   └── queries/               # tree-sitter query 檔（.scm）
│
├── templates/
│   └── default_core_policy.yaml   # 預設策略（Ring 0 規則 + Ring 0.5 閾值）
│
├── tests/                     # Pytest 測試套件
└── conftest.py
```

---

## 安裝與建置

專案由兩塊組成——Rust 內核必須先編譯成 Python extension，上層 Python 才能 `import aegis_core_rs`。

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

入口點：`python -m aegis.cli <command>`（或將 `aegis/cli.py` 做成 console script）

### `check` — Ring 0 架構檢查

對指定路徑執行語法驗證 + 循環依賴偵測。

```bash
python -m aegis.cli check ./my_project
python -m aegis.cli check ./my_project --signals   # 額外顯示 Ring 0.5 訊號
```

失敗時以 `exit 1` 結束，可直接接入 CI。

### `generate` — LLM 生成（帶 Ring 0 守門）

```bash
python -m aegis.cli generate "寫一個 LRU cache class" -o lru.py
```

背後走 `LLMGateway.generate_and_validate()`：
產出 → Ring 0 驗證 → 若語法錯誤則帶著 violation 重試（預設最多 3 次）。

### `refactor` — 規劃-驗證-執行 重構流水線

```bash
python -m aegis.cli refactor "拆掉 circular dependency" ./my_project \
    --scope aegis/ --max-iters 3
```

流程（`aegis/runtime/pipeline.py`）：

1. **Planner** 收集 signals、依賴圖、檔案內容，產出結構化 `PatchPlan` (JSON)
2. **Validator** 檢查 patch 是否合法、anchor 是否唯一、是否在 scope 內
3. **Executor** 帶備份地套用 patch；失敗自動 rollback
4. 重新分析訊號，若**總 signal 數增加**（regression）→ 整回合回滾
5. 直到 `done=true` 或 `max_iters` 為止

### `chat` — 互動式對話（每輪輸出皆經 Ring 0 驗證）

```bash
python -m aegis.cli chat --model gemini-2.5-flash
```

---

## 兩個你需要知道的核心流程

### 1. LLMGateway（`aegis/agents/llm_adapter.py`）

```
Prompt ─▶ LLM.generate ─▶ Ring0Validator.validate
                            │
                    violations? ──Yes──▶ 帶 violation 重試
                            │
                            No
                            ▼
                    SignalContextBuilder 附上 Ring 0.5 觀察
                            ▼
                       回傳安全程式碼
```

### 2. Refactor Pipeline

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

---

## Rust 內核揭露的 API

透過 `aegis/core/bindings.py` 統一轉出，上層**永遠不直接 import `aegis_core_rs`**（便於替換後端）：

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

## 測試

```bash
pytest                     # 全部
pytest tests/test_enforcement.py -v
pytest tests/test_refactor_pipeline.py -v
```

現有測試覆蓋：AST parser、Signal Layer、EditEngine、Enforcement、FileSystem 工具、
Refactor Pipeline、Semantic IR、LLM Gateway、CLI。

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

修改這個檔案即可調整 Aegis 的嚴格度；Ring 0.5 閾值**不會阻擋**，只是做為 LLM 的決策提示。

---

## 擴充方向

- **新語言支援**：在 `aegis-core-rs/src/ast/languages/` 加入對應 tree-sitter grammar，再擴充 `get_imports` / `extract_signals` 分支。
- **新 LLM Provider**：在 `aegis/agents/` 新增一個 class，實作 `LLMProvider.generate(prompt) -> str` 即可接入。
- **自訂 Ring 0 規則**：在 `aegis/enforcement/rules.py` 加新檢查，並在 `Ring0Enforcer.check_project` 串接。

---

## License

尚未指定。
