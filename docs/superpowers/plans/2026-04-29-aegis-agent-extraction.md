# aegis-agent 抽離計畫

> **AI 代理執行者注意：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 逐任務執行。步驟使用複選框（`- [ ]`）追蹤進度。本計畫不是 TDD 形態（沒有紅綠循環），是 repo 外科手術；步驟驗證以「指令成功 + 預期輸出」為憑。

**目標：** 把 `crates/aegis-agent`（18,422 src LOC + 295 測試）從 aegis monorepo 抽離到獨立 repo，把 aegis 還原成「純裁判」形態，移除 `aegis chat` 子命令；保留 V3 git 歷史與 claw-code 出處鏈。

**架構：** Aegis 仍是負空間裁判 harness；新 repo 是 `aegis-mcp` 的眾多消費者之一（並列於 Cursor / Claude Code / Aider）。跨 repo 耦合 = git tag 依賴 `aegis-core` + `aegis-decision`，無 path、無 submodule、無 crates.io 發布。

**技術棧：** `git filter-repo`（必須先安裝）、cargo workspaces、GitHub `gh` CLI（用於建 repo + 推送）。

---

## 已內建的載重決策（執行前若想改，現在說）

| # | 決策點 | 已內建選擇 | 理由 |
|---|---|---|---|
| **D1** | 新 repo 名 | `<NEW_REPO_NAME>`（**待用戶填**） | 用戶選擇；推薦：`aegis-claw`（顯示 claw-code 血脈）或保留 `aegis-agent` |
| **D2** | aegis-cli `chat` 子命令命運 | **DELETE** | aegis CLI 還原成 `check / pipeline / languages / mcp`；想要 chat 的人去裝新 repo 的 binary。最對齊負空間 framing |
| **D3** | 跨 repo 依賴策略 | **git tag dep** | 在 aegis 切 `v0.1.1-pre-extract` tag，新 repo `Cargo.toml` 用 `{ git = "...", tag = "v0.1.1-pre-extract" }`。不強迫 V2 packaging 提前 |
| **D4** | git 歷史保留 | **`git filter-repo` 保留 V3 歷史** | claw-code MIT 出處鏈可追溯；fresh init 會切斷 attribution chain |
| **D5** | 抽離後本 repo 內 crate 處置 | **硬刪**（無 stub） | git log 自有紀錄；stub 會腐爛 |
| **D6** | `tests/mcp_with_aegis_mcp.rs` 跨 repo 策略 | **`#[ignore]` + 文件化指引** | 本地或 CI 想跑時 clone aegis tag 並 build aegis-mcp。等 V2 packaging 後可改 download release binary |

非載重（沿用合理預設）：
- 文件遷移：`docs/v3_agent_design.md`、`docs/v3_dogfood.md` 整檔搬到新 repo `docs/`
- aegis repo 文件刪修：README.md / ROADMAP.md / framework.md 各自 surgical edit（見 Phase D）
- memory 更新：抽離完成後一次性清整（見 Phase E）

---

## 文件變更總覽（鎖定切分）

**新 repo（`<NEW_REPO_NAME>`）會有：**
- `Cargo.toml` — 改寫成 standalone，path dep → git tag dep
- `src/`（複製整個 `crates/aegis-agent/src/`）
- `tests/`（295 測試）
- `examples/`（chat_demo.rs）
- `README.md`（新寫）
- `LICENSE`（從 aegis 複製，MIT）
- `NOTICE`（新寫，集中標明 claw-code MIT 出處）
- `docs/v3_agent_design.md`（從 aegis 搬遷，內部連結重寫）
- `docs/v3_dogfood.md`（從 aegis 搬遷）
- `.github/workflows/ci.yml`（新 repo CI；跑 `cargo test`，整合測試 `--ignored` 跳過）

**aegis repo（本 repo）會：**
- 刪：`crates/aegis-agent/`（整個目錄）
- 刪：`docs/v3_agent_design.md`、`docs/v3_dogfood.md`
- 改：`Cargo.toml`（移除 workspace member）
- 改：`crates/aegis-cli/Cargo.toml`（移除 aegis-agent dep）
- 改：`crates/aegis-cli/src/main.rs`（刪 chat 子命令 + 所有 `aegis_agent::` import 區塊）
- 改：`README.md`（刪 V3 chat 段落、修 aegis-agent 提及）
- 改：`docs/ROADMAP.md`（V3 章節改成 historical pointer 到新 repo）
- 改：`docs/framework.md`（reference implementation 指向新 repo URL）
- 改：`AGENTS.md`（若有 V3 / chat 提及）
- 改：`docs/post_launch_discipline.md`（V3 提及更新）
- 加：`docs/CHANGELOG.md`（若無）或 ROADMAP.md 加一條 V3.9 — agent split-out

**Cross-repo（兩邊都動）：**
- aegis 切 tag `v0.1.1-pre-extract`
- 新 repo 在 `Cargo.toml` 釘該 tag

---

## Phase 0 — 預檢與保險（在 aegis repo）

### 任務 0：確認乾淨狀態並切 tag

**檔案：** 無修改（讀取 + tag）。

- [ ] **步驟 1：確認 working tree 乾淨**

  運行：`git status`
  預期：`nothing to commit, working tree clean`（4 張未追蹤 PNG 截圖無關，可忽略）

- [ ] **步驟 2：確認全 workspace 編譯與測試通過（基準線）**

  運行：`cargo test --workspace 2>&1 | tail -20`
  預期：`test result: ok` + 約 433 tests passed

- [ ] **步驟 3：確認 git filter-repo 已安裝**

  運行：`git filter-repo --version`
  預期：版本字串。若 `command not found`，先 `pipx install git-filter-repo` 或 `pip install --user git-filter-repo`

- [ ] **步驟 4：切跨 repo pin tag**

  運行：
  ```
  git tag -a v0.1.1-pre-extract -m "Pin point before aegis-agent extraction"
  git push origin v0.1.1-pre-extract
  ```
  預期：`origin` 上看得到該 tag（`git ls-remote --tags origin | grep v0.1.1-pre-extract`）

- [ ] **步驟 5：建立工作分支保護當前狀態**

  運行：`git checkout -b extract/aegis-agent`
  預期：在新分支上；`main` 維持不動直到所有 phase 結束。

---

## Phase A — 抽離到新 repo（在 `/tmp/extract-aegis-agent` 暫存目錄）

### 任務 A1：fresh clone + filter-repo 鎖定 aegis-agent 子樹

**檔案：** 新建 working tree（不影響 aegis repo）。

- [ ] **步驟 1：到暫存目錄並 fresh clone**

  運行：
  ```
  cd /tmp && rm -rf extract-aegis-agent
  git clone git@github.com:wei9072/aegis.git extract-aegis-agent
  cd extract-aegis-agent
  ```
  預期：clone 成功；位於 `/tmp/extract-aegis-agent`。

- [ ] **步驟 2：filter-repo 只保留 aegis-agent 路徑並提升至根**

  運行：
  ```
  git filter-repo --path crates/aegis-agent/ --path-rename crates/aegis-agent/:
  ```
  預期：歷史改寫；只剩觸及 `crates/aegis-agent/` 的 commit；現存檔案路徑提升到 repo 根。

- [ ] **步驟 3：檢視倖存檔案樹**

  運行：`ls -la && find . -name '*.rs' -not -path './.git/*' | wc -l`
  預期：根目錄有 `Cargo.toml`、`src/`、`tests/`、`examples/`；Rust 檔約 50–60 個。

- [ ] **步驟 4：檢視倖存 git log**

  運行：`git log --oneline | head -20`
  預期：V3.0–V3.8 commit 訊息看得到；無無關於 aegis-agent 的 commit。

- [ ] **步驟 5：檢視 claw-code 出處標註倖存**

  運行：`grep -rn "Adapted from claw-code" src/ | wc -l`
  預期：≥10（先前驗證為 11 處）。

### 任務 A2：改寫 Cargo.toml 為 standalone

**檔案：**
- 修改：`Cargo.toml`

- [ ] **步驟 1：讀現有 Cargo.toml 記錄基準**

  運行：`cat Cargo.toml`
  記下：`version`、`edition`、`license`、`repository` 是 workspace 繼承（需具體填值）。

- [ ] **步驟 2：完整覆寫為 standalone 版本**

  寫入 `Cargo.toml`：
  ```toml
  [package]
  name = "aegis-agent"
  version = "0.1.0"
  edition = "2021"
  license = "MIT"
  repository = "https://github.com/wei9072/<NEW_REPO_NAME>"
  description = "Coding agent built on the Aegis negative-space rejection harness."

  [dependencies]
  aegis-core = { git = "https://github.com/wei9072/aegis.git", tag = "v0.1.1-pre-extract" }
  aegis-decision = { git = "https://github.com/wei9072/aegis.git", tag = "v0.1.1-pre-extract" }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  ureq = { version = "2.10", features = ["json"] }
  glob = "0.3"

  [dev-dependencies]
  tempfile = "3"
  syn = { version = "2", features = ["full", "visit"] }
  ```

- [ ] **步驟 3：嘗試 build**

  運行：`cargo build 2>&1 | tail -30`
  預期：成功（git deps fetch 一次比較慢）。若 `aegis-core` / `aegis-decision` 公開 API 不一致，**這一步應該不會失敗**因為我們釘的是同一 commit；若失敗請停下來看 error 不要硬改。

- [ ] **步驟 4：跑非整合測試**

  運行：`cargo test 2>&1 | tail -30`
  預期：絕大多數通過；`tests/mcp_with_aegis_mcp.rs` 內各 test 可能失敗（找不到 `aegis-mcp` binary）— 步驟 5 處理。

- [ ] **步驟 5：對 mcp_with_aegis_mcp.rs 加 `#[ignore]`**

  讀：`tests/mcp_with_aegis_mcp.rs`
  對檔內每個 `#[test]` 函式上方加：
  ```rust
  #[ignore = "Cross-repo integration: requires aegis-mcp binary built from sister repo. See README.md > Cross-repo integration tests."]
  ```

- [ ] **步驟 6：再跑測試確認**

  運行：`cargo test 2>&1 | tail -10`
  預期：`test result: ok` + ≥1 ignored；剩餘測試全綠。

- [ ] **步驟 7：commit**

  運行：
  ```
  git add Cargo.toml tests/mcp_with_aegis_mcp.rs
  git commit -m "build: standalone repo — replace path deps with git tag deps; ignore cross-repo integration tests"
  ```

### 任務 A3：新增 README / NOTICE / LICENSE

**檔案：**
- 創建：`README.md`、`NOTICE`
- 確認：`LICENSE`（filter-repo 不會帶 root LICENSE，需從 aegis 複製）

- [ ] **步驟 1：複製 LICENSE 從 aegis repo**

  運行：`cp /home/a108222024/harness/aegis/LICENSE ./LICENSE`
  預期：MIT LICENSE 在新 repo 根目錄。

- [ ] **步驟 2：寫 README.md**

  內容（精確）：
  ```markdown
  # <NEW_REPO_NAME>

  A coding agent built on the [Aegis](https://github.com/wei9072/aegis) negative-space rejection harness.

  ## What this is

  An LLM-driven coding agent that wraps Aegis as its judgement gate.
  Aegis decides whether each proposed file write is allowed to stay;
  this agent generates the writes.

  Four differentiation points vs. a generic chat-loop agent:

  1. **PreToolUse aegis-predict** — every file-write tool call is
     pre-checked against `aegis-mcp validate_change`; BLOCK skips
     execution and the LLM sees structured reasons.
  2. **Cross-turn cost tracking** — `CostTracker` accumulates
     structural cost across iterations; `CostBudgetExceeded` ends
     the session.
  3. **Verifier-driven done** — `AgentTaskVerifier` (Shell / Test /
     Build / Composite) overrules the LLM's "no more tool_use" claim.
  4. **Stalemate detection** — three successive identical cost
     totals → `StoppedReason::StalemateDetected`.

  ## What this is NOT

  Aegis itself. Aegis is a separate project with a different thesis
  (*"don't try to make AI better; ensure worse outcomes don't
  stick"*). This repo deliberately sits downstream of Aegis as one
  example of a code-generating agent that defers to it.

  ## Lineage

  This crate originated as `crates/aegis-agent` inside the Aegis
  monorepo and was spun out at aegis tag `v0.1.1-pre-extract`. V3
  phase commit history (V3.0–V3.8) is preserved.

  Conversation / tool / session / hook / API scaffolding (~11k LOC)
  is adapted from [claw-code](https://github.com/ultraworkers/claw-code)
  under MIT, with per-file `// Adapted from claw-code (MIT) — <upstream path>`
  attribution at the top of borrowed modules. See `NOTICE`.

  ## Build

  ```bash
  cargo build --release
  cargo install --path .
  ```

  Provides one binary: `aegis-agent` chat / one-shot driver (TBD if
  you want a different binary name — for now this crate is library +
  example).

  ## Run

  See [`docs/v3_dogfood.md`](docs/v3_dogfood.md) for the three usage
  modes (chat REPL / chat --tools / MCP server).

  ## Relationship to Aegis

  Cross-repo dependency: `aegis-core` and `aegis-decision` are
  pulled via git tag in `Cargo.toml`. To upgrade the Aegis pin,
  bump the tag and re-test.

  ## Cross-repo integration tests

  `tests/mcp_with_aegis_mcp.rs` exercises this agent talking to the
  real `aegis-mcp` binary over JSON-RPC stdio. It's `#[ignore]`d by
  default. To run it locally:

  ```bash
  git clone git@github.com:wei9072/aegis.git /tmp/aegis-mcp-src
  (cd /tmp/aegis-mcp-src && cargo build --release -p aegis-mcp)
  PATH=/tmp/aegis-mcp-src/target/release:$PATH cargo test --test mcp_with_aegis_mcp -- --ignored
  ```

  ## License

  MIT — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
  ```

- [ ] **步驟 3：寫 NOTICE**

  內容：
  ```
  <NEW_REPO_NAME>
  Copyright (c) 2026 wei9072

  This product includes software developed by the claw-code project
  (https://github.com/ultraworkers/claw-code), licensed under MIT.

  Files in this repository carrying a top-of-file comment of the form
      // Adapted from claw-code (MIT) — <upstream path>
  contain code copied and modified from claw-code's `rust/crates/runtime/`
  and `crates/api/` modules. Specifically:

      runtime/conversation.rs   → src/conversation.rs, src/api.rs, src/tool.rs
      runtime/file_ops.rs       → (file_ops integration in src/tool.rs)
      runtime/bash.rs           → src/bash.rs
      runtime/session.rs        → src/message.rs (Session)
      runtime/compact.rs        → src/compact.rs
      runtime/permissions.rs    → src/permission.rs (modified — see comment)
      runtime/hooks.rs          → src/hooks.rs
      runtime/mcp_*.rs          → src/mcp/*.rs
      api/                      → src/providers/*.rs

  See `docs/v3_agent_design.md` § "What we borrow from claw-code" for
  the LOC table and rationale.

  This product also depends on the Aegis project
  (https://github.com/wei9072/aegis) for `aegis-core` and `aegis-decision`,
  which are licensed under MIT.
  ```

- [ ] **步驟 4：commit**

  運行：
  ```
  git add LICENSE README.md NOTICE
  git commit -m "docs: add README, NOTICE (claw-code attribution table), copy LICENSE"
  ```

### 任務 A4：搬遷 V3 文件並修連結

**檔案：**
- 創建：`docs/v3_agent_design.md`、`docs/v3_dogfood.md`
- 在 aegis repo 那邊的刪除動作放 Phase D

- [ ] **步驟 1：複製 v3 文件**

  運行：
  ```
  mkdir -p docs
  cp /home/a108222024/harness/aegis/docs/v3_agent_design.md docs/v3_agent_design.md
  cp /home/a108222024/harness/aegis/docs/v3_dogfood.md docs/v3_dogfood.md
  ```

- [ ] **步驟 2：把 v3_agent_design.md 內部相對連結改寫**

  讀：`docs/v3_agent_design.md`
  把所有指向 aegis repo 內檔（例如 `crates/aegis-agent/...`、`docs/framework.md`、`docs/ROADMAP.md`）的相對 link 改寫：
  - `crates/aegis-agent/...` → 對應新 repo 路徑（例如 `src/...`、`tests/...`）
  - `docs/framework.md` → `https://github.com/wei9072/aegis/blob/main/docs/framework.md`（絕對 URL，因 framework.md 留在 aegis）
  - `docs/ROADMAP.md` → `https://github.com/wei9072/aegis/blob/main/docs/ROADMAP.md`

  使用 `grep -n "](.*\.md)\|](crates/" docs/v3_agent_design.md` 列出所有候選，逐一決定。

- [ ] **步驟 3：對 v3_dogfood.md 做同樣修整**

- [ ] **步驟 4：commit**

  運行：
  ```
  git add docs/
  git commit -m "docs: bring v3_agent_design + v3_dogfood with rewritten links"
  ```

### 任務 A5：新 repo CI 設定

**檔案：**
- 創建：`.github/workflows/ci.yml`

- [ ] **步驟 1：寫最小 CI**

  內容：
  ```yaml
  name: CI

  on:
    push:
      branches: [main]
    pull_request:

  jobs:
    test:
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - uses: Swatinem/rust-cache@v2
        - name: Build
          run: cargo build --workspace --all-targets
        - name: Test (excluding cross-repo ignored)
          run: cargo test --workspace
        - name: Clippy
          run: cargo clippy --workspace --all-targets -- -D warnings
  ```

  （cross-repo `mcp_with_aegis_mcp` 測試被 `#[ignore]`，CI 不會跑；本地 `--ignored` 才跑。需要時可後續加一個獨立 job clone aegis 並 build aegis-mcp。）

- [ ] **步驟 2：commit**

  運行：
  ```
  git add .github/workflows/ci.yml
  git commit -m "ci: minimal GitHub Actions — build + test + clippy"
  ```

### 任務 A6：在 GitHub 建 repo 並推送

- [ ] **步驟 1：用 gh 建 repo**（需要使用者登入或本地 `gh auth status` 已通）

  運行：
  ```
  gh repo create wei9072/<NEW_REPO_NAME> --public \
    --description "Coding agent built on the Aegis negative-space rejection harness."
  ```
  預期：repo 建立。**若 `gh` 未認證，請先 `gh auth login`，或手動到 GitHub 介面建 repo 後跳到步驟 2。**

- [ ] **步驟 2：origin 改指向新 repo 並推送**

  運行：
  ```
  git remote remove origin
  git remote add origin git@github.com:wei9072/<NEW_REPO_NAME>.git
  git push -u origin main
  ```
  預期：所有 V3 commit 推上去。

- [ ] **步驟 3：去 GitHub 確認**

  瀏覽：`https://github.com/wei9072/<NEW_REPO_NAME>`
  確認：commit 歷史可見、README 顯示、LICENSE/NOTICE 存在。

---

## Phase B — 驗證新 repo 自包含

### 任務 B1：clean clone 從新 repo build + test

**目的：** 證明新 repo 不依賴本機 aegis path。

- [ ] **步驟 1：clean clone 到第三個目錄**

  運行：
  ```
  cd /tmp && rm -rf verify-extract && git clone git@github.com:wei9072/<NEW_REPO_NAME>.git verify-extract && cd verify-extract
  ```

- [ ] **步驟 2：build**

  運行：`cargo build 2>&1 | tail -15`
  預期：成功；git deps（aegis-core、aegis-decision）從 aegis tag 抓下來。

- [ ] **步驟 3：test**

  運行：`cargo test 2>&1 | tail -10`
  預期：全綠 + ≥1 ignored。

- [ ] **步驟 4：手動跑跨 repo 整合測試（可選但建議）**

  運行：
  ```
  cd /tmp && rm -rf aegis-mcp-src && git clone git@github.com:wei9072/aegis.git aegis-mcp-src && (cd aegis-mcp-src && cargo build --release -p aegis-mcp)
  cd /tmp/verify-extract && PATH=/tmp/aegis-mcp-src/target/release:$PATH cargo test --test mcp_with_aegis_mcp -- --ignored 2>&1 | tail -15
  ```
  預期：integration test 通過。**若失敗：很可能是新 repo 對 aegis-mcp 的版本假設與當前 aegis HEAD 不一致 — 但因 tag pin 在 `v0.1.1-pre-extract`，理論上一致。** 失敗請記錄具體錯誤回報。

---

## Phase C — 在 aegis repo 移除 aegis-agent 與 chat 子命令

**重點：所有 phase C 動作在 aegis repo 的 `extract/aegis-agent` 分支上做（Phase 0 步驟 5 建的）。**

### 任務 C1：刪 chat 子命令的源碼依賴

**檔案：**
- 修改：`crates/aegis-cli/Cargo.toml`
- 修改：`crates/aegis-cli/src/main.rs`

- [ ] **步驟 1：讀 main.rs 找 chat 相關區塊**

  運行：`grep -n "chat\|aegis_agent" crates/aegis-cli/src/main.rs | head -40`
  記下：chat 子命令 clap 結構、各 `aegis_agent::` import 區塊起訖、相關 helper fn 範圍（已知至少 506–860 行有大量引用）。

- [ ] **步驟 2：移除 main.rs 內的 chat 子命令**

  - 刪 clap `Command::Chat` enum variant 與所有相關 struct
  - 刪 `match` 分支中對 `Command::Chat(...)` 的處理
  - 刪所有以 `aegis_agent::` 為首的 import
  - 刪所有只被 chat 使用的 helper fn（`fn run_chat`、SSE rendering helpers 等）

  做法：使用多個 `Edit` 操作；每次刪一塊後 `cargo check -p aegis-cli` 驗證 import 是否還有殘留 / 未使用。

- [ ] **步驟 3：移除 Cargo.toml dep**

  編輯 `crates/aegis-cli/Cargo.toml`，刪除：`aegis-agent = { path = "../aegis-agent" }`

- [ ] **步驟 4：驗證 cli 還能編譯與通過原 cli 測試**

  運行：`cargo check -p aegis-cli && cargo test -p aegis-cli 2>&1 | tail -15`
  預期：編譯成功；非 chat 相關測試全綠。若 chat 相關測試還在，刪掉。

- [ ] **步驟 5：commit**

  運行：
  ```
  git add crates/aegis-cli/
  git commit -m "feat(cli)!: remove chat subcommand — agent surface moves to <NEW_REPO_NAME>

  BREAKING CHANGE: aegis CLI no longer ships the chat REPL. Install
  the standalone agent from https://github.com/wei9072/<NEW_REPO_NAME>
  if you want chat / one-shot agent mode. aegis CLI returns to its
  pure-judge surface: check / pipeline / languages / mcp."
  ```

### 任務 C2：刪 aegis-agent crate 與 workspace 成員

**檔案：**
- 修改：`Cargo.toml`（root）
- 刪除：`crates/aegis-agent/`

- [ ] **步驟 1：從 root Cargo.toml workspace.members 移除 aegis-agent**

  讀 `Cargo.toml`，編輯第 19 行附近，刪除 `"crates/aegis-agent",`。

- [ ] **步驟 2：刪 crate 目錄**

  運行：`git rm -r crates/aegis-agent`
  預期：~50 個檔案 staged 為刪除。

- [ ] **步驟 3：驗證 workspace 整體**

  運行：`cargo build --workspace && cargo test --workspace 2>&1 | tail -20`
  預期：編譯成功；測試數從 433 → 約 138（433 − 295 = 138；±20 容差）。

- [ ] **步驟 4：commit**

  運行：
  ```
  git add Cargo.toml crates/aegis-agent
  git commit -m "feat!: extract aegis-agent to standalone repo

  Removes crates/aegis-agent from this workspace; the V3 agent now
  lives at https://github.com/wei9072/<NEW_REPO_NAME> with its V3
  phase commit history preserved via filter-repo. claw-code MIT
  attribution chain travels with the new repo.

  Aegis returns to its single thesis: a negative-space rejection
  harness. Crates remaining: aegis-core, aegis-decision, aegis-ir,
  aegis-runtime, aegis-providers, aegis-trace, aegis-cli, aegis-mcp.

  See docs/post_launch_discipline.md for why."
  ```

### 任務 C3：刪 V3 專屬文件

**檔案：**
- 刪除：`docs/v3_agent_design.md`、`docs/v3_dogfood.md`

- [ ] **步驟 1：刪檔**

  運行：`git rm docs/v3_agent_design.md docs/v3_dogfood.md`

- [ ] **步驟 2：commit**

  運行：
  ```
  git commit -m "docs: remove V3-specific docs (moved to <NEW_REPO_NAME>)"
  ```

---

## Phase D — 修整 aegis framing 文件

### 任務 D1：README.md

**檔案：** `README.md`

- [ ] **步驟 1：定位需修段落**

  運行：`grep -n "aegis-agent\|aegis chat\|V3" README.md`
  預期 hits：第 24 行、216 行、256 行。

- [ ] **步驟 2：修第 24 行附近**

  原文：`docs/framework.md`. `aegis-agent` is one implementation case: Aegis applied to agent-driven code changes.`
  改：`docs/framework.md`. The reference implementation `aegis-agent` lives at https://github.com/wei9072/<NEW_REPO_NAME> (Aegis applied to agent-driven code changes).`

- [ ] **步驟 3：刪除 V3 chat 章節（第 216 行起）**

  讀 `README.md` 從第 200 行起到第 280 行，識別 `### aegis chat — interactive coding agent (V3)` 整段直到下一個同層 heading；刪除。在原位插入：
  ```markdown
  ### Want a chat agent on top of Aegis?

  See [`<NEW_REPO_NAME>`](https://github.com/wei9072/<NEW_REPO_NAME>) —
  the V3 reference agent that wraps `aegis-mcp` as its judgement gate.
  Aegis itself stops at the gate; agents that *use* Aegis live in
  separate repos.
  ```

- [ ] **步驟 4：驗證 markdown 渲染合理**

  運行：`grep -n "aegis-agent\|aegis chat\|V3" README.md`
  預期：保留的提及都只是「指向新 repo」形式。

- [ ] **步驟 5：commit**

  運行：
  ```
  git add README.md
  git commit -m "docs(readme): redirect V3 / chat references to <NEW_REPO_NAME>"
  ```

### 任務 D2：docs/ROADMAP.md

**檔案：** `docs/ROADMAP.md`

- [ ] **步驟 1：把 V3 章節（行 54 起的整個「V3 — substrate + hand」section）替換為 historical pointer**

  原內容：~50 行 V3 phase status 表格。
  替換為：
  ```markdown
  ## V3 — substrate + hand (DONE 2026-04-27, EXTRACTED 2026-04-29)

  V3 (`aegis-agent`) was a coding agent built on aegis primitives.
  All eight phases (V3.0 through V3.8) shipped in one day on
  2026-04-27, adding 295 tests to the workspace.

  On 2026-04-29 the V3 codebase was extracted to a standalone repo
  at https://github.com/wei9072/<NEW_REPO_NAME> with its V3 commit
  history preserved (via `git filter-repo`). This restored aegis to
  a pure-judge framing — the agent surface lives downstream of
  aegis-mcp like any other consumer (Cursor, Claude Code, Aider).

  V3 phase detail and design rationale now live in
  [`<NEW_REPO_NAME>/docs/v3_agent_design.md`](https://github.com/wei9072/<NEW_REPO_NAME>/blob/main/docs/v3_agent_design.md).
  ```

- [ ] **步驟 2：搜其他 V3 / aegis-agent 提及**

  運行：`grep -n "aegis-agent\|V3" docs/ROADMAP.md`
  決定每個 hit：保留（僅作歷史敘述）或重寫（指向新 repo）。

- [ ] **步驟 3：commit**

  運行：
  ```
  git add docs/ROADMAP.md
  git commit -m "docs(roadmap): replace V3 phase table with extraction pointer to <NEW_REPO_NAME>"
  ```

### 任務 D3：docs/framework.md

**檔案：** `docs/framework.md`

- [ ] **步驟 1：定位**

  運行：`grep -n "aegis-agent" docs/framework.md`
  預期 hits：第 7、12、61、120、163 行。

- [ ] **步驟 2：每個 hit 改寫**

  原樣式（多處）：`reference implementation (`aegis-agent`)`
  改成：`reference implementation (`aegis-agent`, https://github.com/wei9072/<NEW_REPO_NAME>)`

  並加一段 callout（第 12 行附近，最早提及處）：
  ```markdown
  > **Note (2026-04-29):** the reference implementation lives in a
  > separate repo so this framework definition stays domain-independent.
  > See https://github.com/wei9072/<NEW_REPO_NAME>.
  ```

- [ ] **步驟 3：commit**

  運行：
  ```
  git add docs/framework.md
  git commit -m "docs(framework): point reference-implementation references at <NEW_REPO_NAME>"
  ```

### 任務 D4：AGENTS.md / docs/post_launch_discipline.md / 其他殘留

- [ ] **步驟 1：殘留掃描**

  運行：`grep -rn "aegis-agent\|aegis_agent\|aegis chat\|V3 chat" *.md docs/ AGENTS.md 2>/dev/null`
  逐一決定 — 大多應該 mass-rewrite 為「在新 repo」。

- [ ] **步驟 2：逐檔 surgical edit + 一次 commit**

  運行：
  ```
  git add -A
  git commit -m "docs: sweep remaining aegis-agent references → <NEW_REPO_NAME>"
  ```

---

## Phase E — 收尾：merge、push、memory

### 任務 E1：合回 main + 推送

- [ ] **步驟 1：最終驗證 workspace**

  運行：`cargo build --workspace && cargo test --workspace 2>&1 | tail -10`
  預期：build 綠；測試 ~138 個全綠。

- [ ] **步驟 2：合分支**

  運行：
  ```
  git checkout main
  git merge --no-ff extract/aegis-agent -m "feat!: extract aegis-agent to standalone repo

  See https://github.com/wei9072/<NEW_REPO_NAME>.

  Aegis is now a pure-judge harness. Workspace shrinks by 18k LOC
  and 295 tests; framing aligns with docs/post_launch_discipline.md."
  ```

- [ ] **步驟 3：推送**

  運行：`git push origin main`

- [ ] **步驟 4：切收尾 tag**

  運行：
  ```
  git tag -a v0.2.0 -m "Post-extraction: aegis returns to pure-judge surface"
  git push origin v0.2.0
  ```

### 任務 E2：更新 auto-memory

**檔案：** `~/.claude/projects/-home-a108222024-harness-aegis/memory/MEMORY.md` + 個別 memory 檔。

需要 touch 的記憶條目（基於現有索引）：
- `aegis_core_framing_negative_space.md` — 加註：2026-04-29 抽離 V3 後 framing 純化
- `archived_v4_v6.md` — 不必動
- `dogfood_*` — 標記成 historical（dogfood 跑步機現在在新 repo）
- 新增：`v3_extraction_2026-04-29.md` — 紀錄抽離決定 + 新 repo URL + tag pin

- [ ] **步驟 1：寫新 memory 檔**

  路徑：`~/.claude/projects/-home-a108222024-harness-aegis/memory/v3_extraction_2026_04_29.md`
  內容：簡述抽離決定、git tag pin、新 repo URL、五項已內建決策。

- [ ] **步驟 2：在 MEMORY.md 加索引行**

- [ ] **步驟 3：對受影響的舊 memory 檔加日期標註**

---

## 自檢

### 1. 規格覆蓋
- ✅ 抽離 → Phase A
- ✅ 跨 repo 依賴 → Cargo.toml 改 git tag dep（A2）
- ✅ git 歷史保留 → filter-repo（A1）
- ✅ chat 子命令刪除 → C1
- ✅ workspace 成員移除 → C2
- ✅ V3 文件搬遷 → A4 + C3
- ✅ framing 文件修整 → D1–D4
- ✅ 驗證 → B1
- ✅ memory → E2

### 2. 佔位符掃描
- `<NEW_REPO_NAME>` 是 D1 待用戶填的具名變數，全篇明確標示 — 非禁止意義的「待定」。
- 沒有 "TODO 加錯誤處理"、"類似任務 N" 之類的紅旗。

### 3. 型別 / 命名一致性
- tag 名稱 `v0.1.1-pre-extract` 全篇一致（Phase 0 切、A2 釘、B1 驗證）。
- 分支名 `extract/aegis-agent` 全篇一致（Phase 0 建、E1 合）。
- `<NEW_REPO_NAME>` 全篇佔位一致。

### 4. 風險回滾路徑

| 卡點 | 回滾 |
|---|---|
| Phase A filter-repo 出錯 | `/tmp/extract-aegis-agent` 刪掉重 clone；不影響 aegis repo |
| Phase B 新 repo build 失敗 | 不要進 Phase C；先在新 repo 修；aegis repo 還沒被改 |
| Phase C 編譯失敗 | `git checkout extract/aegis-agent~N` 退到上個 commit；分支還沒合進 main |
| Phase E 合錯 | 還沒推就 `git reset --hard origin/main`；推了就 revert commit |

**單向絞鎖點：** Phase E1 步驟 3「推送 main」是 push 之前都能本機回退、push 之後成本高的關鍵點。執行前再跑一次完整 `cargo test --workspace`。

---

## 執行交接

**「計畫已保存到 `docs/superpowers/plans/2026-04-29-aegis-agent-extraction.md`。在啟動 Phase 0 之前，請先回答 D1 並確認 D2–D6（不同意就指出來）。執行方式兩選：**

**1. 子代理驅動（推薦）** — 每個 Task（A1、A2、…）派一個子代理執行 + 檢查點審查。隔離乾淨，但對話開銷較大。

**2. 內聯執行** — 在當前會話直接跑 Phase 0 → A → B → C → D → E，每個 Phase 結束我停下來給你看。較快但本對話會變長。

**選哪個？並回答 D1（新 repo 名）。」**
