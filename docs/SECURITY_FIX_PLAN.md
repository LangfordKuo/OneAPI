# OneAPI 安全修复计划（M5–M8、M10–M14、L1–L11）

- 状态：已实施（v0.1.18）
- 制定日期：2026-10-31
- 依据：2026-10-31 全仓安全审查报告
- 适用范围：仅修复下列 20 项；其余安全发现保持现状，另行排期

## 0. 范围

### 0.1 本计划修复项

| 编号 | 问题 | 目标 |
|---|---|---|
| M5 | 详情写队列只限制条数、不限制字节，入队前不截断 | 入队前截断；队列增加字节配额 |
| M6 | Responses 默认 `store=true` 且无每 Key 配额，可耗尽磁盘 | 每 Key 与全局存储配额，超额返回 `store:false` |
| M7 | `model` 与 `previous_response_id` 无长度限制，写入带索引的 `usage_logs` | 输入校验 + 持久化截断 |
| M8 | 配额检查与用量入账异步解耦，存在 TOCTOU 与整批丢失 | 准入阶段原子预留；批次失败不丢账；每日计数单调 |
| M10 | `/v1/models` 的 `recent_models` 跨租户泄露模型名 | 仅返回当前 Key 可用模型 |
| M11 | 上游错误与内部拓扑泄露给 API Key 客户端 | 客户端只收稳定分类错误；路由头仅 Playground |
| M12 | 下游慢客户端被误判为上游超时并冷却共享渠道 | 区分客户端背压与上游失败，不再误冷却 |
| M13 | 上游 200 非 SSE 透传；缺失终止事件被当正常完成 | 校验内容类型与协议终止事件，截断即报错 |
| M14 | 协议语义静默丢失与计费算术溢出 | 不可表示字段显式处理；usage 饱和运算 |
| L1 | `responses` 的 `ON CONFLICT(id)` 会改写 `api_key_id` | 冲突时校验归属，禁止跨 Key 改属 |
| L2 | 渠道/模型白名单 JSON 解析失败 fail-open | 解析失败 fail-closed |
| L3 | 备份先按 umask 创建后 chmod；快照无互斥/限流 | 0700 目录 + 0600 文件；失败即失败；加锁限流 |
| L4 | 正文保留期与 usage 耦合；删 Key 不清正文；未开 secure_delete | 独立保留期；删 Key 清正文；启用 secure_delete |
| L5 | 日志详情按客户端 request_id 取最新行，跨 Key 回退 | 按 `usage_logs.id` 查询，删除跨 Key 回退 |
| L6 | 指标 SSE 不随登出失效；无限流阻塞优雅停机 | 会话复检 + 连接上限 + 停机主动结束 |
| L7 | 前端 SSE 401、request_id 拼 URL、设置回退覆盖、编辑回写 enabled、Enter 重复创建 | 前端状态与提交语义修正 |
| L8 | `chart.tsx` 用 `dangerouslySetInnerHTML` 拼 CSS | 改为安全注入并加白名单校验 |
| L9 | 密钥/密码未 zeroize；请求路径与信号处理用 expect | zeroize + 去除不可恢复 panic |
| L10 | API Key 鉴权失败无限速；Key 哈希无 pepper | 鉴权失败限速 + HMAC(pepper) 哈希（兼容旧哈希） |
| L11 | 迁移缺 JSON CHECK、request_details 无外键、0008 索引重名 | 新迁移补触发器/索引；已发布迁移不改 |

### 0.2 明确不修（保持现状）

S1（Compose 默认凭据）、H1（更新器信任链）、H2（登录限速绕过）、H3（登录 Argon2 DoS）、H4（渠道探活密钥外带）、H5（CI 供应链）、M1（CSRF fail-open）、M2（setup 抢注）、M3（Cookie Secure/HSTS）、M4（数据目录权限）、M9（上游 usage 伪造）、M15（每 Key 并发限制）、L12（Docker/Makefile 固定）。

> 上述项目不在本计划的代码改动范围内。若修复过程中发现必须触碰，先更新本文档并单独评审。

## 1. 全局约束

1. **迁移只追加**：不修改 `0001`–`0008`；所有 schema 变更进入 `migrations/0009_security_fixes.sql`。
2. **最小必要改动**：不顺手修复 0.2 中的任何问题，不做无关重构。
3. **兼容性**：
   - L10 保留 v1 哈希列，旧二进制可回滚；
   - L5 只改内部 SPA 使用的接口，前端同步修改；
   - M8 提供环境变量 `ONEAPI_DISABLE_QUOTA_RESERVATION=1` 紧急关闭预留逻辑。
4. **不新增测试专用依赖**：单测使用现有 `tokio`（已启用 `full`）与内存 SQLite；不引入 `tower` dev-dependency。
5. **验证命令**：
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo build --release --locked`（先 `make web`）
   - `cd web && npm run lint && npm run build`
6. **M6 的配额计数依赖单实例串行写**：多实例共用一个 SQLite 文件不在支持范围内；若未来支持多实例，需改为数据库原子条件更新（本计划不覆盖）。
7. **每个阶段结束更新 `docs/PROGRESS.md`**；全部完成后执行一次 AOCI 维护。

## 2. 数据库迁移 0009

新增 `migrations/0009_security_fixes.sql`，内容按下列顺序执行。所有语句必须幂等（`IF NOT EXISTS`），并用 `--` 注释说明归属项。

```sql
-- L5: 记录正文写入时的 Key 归属，Key 删除后仍可精确查正文
ALTER TABLE usage_logs ADD COLUMN detail_api_key_id INTEGER;
UPDATE usage_logs SET detail_api_key_id = api_key_id WHERE detail_api_key_id IS NULL;

-- L10: 带 pepper 的 Key 哈希（惰性回填，旧列保留）
ALTER TABLE api_keys ADD COLUMN key_hash_v2 TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_key_hash_v2
    ON api_keys(key_hash_v2);

-- M8: 准入预留计数（请求进行中占用）
ALTER TABLE api_keys ADD COLUMN reserved_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN reserved_daily_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN reserved_day INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN reserved_at INTEGER;

-- M10: 按 Key 查询最近模型
CREATE INDEX IF NOT EXISTS idx_usage_logs_api_key_created
    ON usage_logs(api_key_id, created_at);

-- M6: Responses 存储计数（每 Key / 全局配额）
CREATE TABLE IF NOT EXISTS response_usage (
    api_key_id INTEGER PRIMARY KEY REFERENCES api_keys(id) ON DELETE CASCADE,
    bytes INTEGER NOT NULL DEFAULT 0,
    rows INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO response_usage (api_key_id, bytes, rows)
SELECT api_key_id,
       SUM(length(COALESCE(request_json, '')) + length(COALESCE(response_json, ''))),
       COUNT(*)
FROM responses
GROUP BY api_key_id;
CREATE INDEX IF NOT EXISTS idx_responses_created_at
    ON responses(created_at);

-- L4: 删除 Key 时同步删除其请求正文
CREATE TRIGGER IF NOT EXISTS trg_api_keys_delete_request_details
AFTER DELETE ON api_keys
BEGIN
    DELETE FROM request_details WHERE api_key_id = OLD.id;
END;

-- L11: JSON 列有效性（新建/更新时阻断非法 JSON）
CREATE TRIGGER IF NOT EXISTS trg_api_keys_json_insert
BEFORE INSERT ON api_keys
WHEN json_valid(NEW.allowed_channel_ids) = 0 OR json_valid(NEW.allowed_models) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid api_key json');
END;

CREATE TRIGGER IF NOT EXISTS trg_api_keys_json_update
BEFORE UPDATE OF allowed_channel_ids, allowed_models ON api_keys
WHEN json_valid(NEW.allowed_channel_ids) = 0 OR json_valid(NEW.allowed_models) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid api_key json');
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_json_insert
BEFORE INSERT ON channels
WHEN json_valid(NEW.models) = 0
  OR json_valid(NEW.model_mapping) = 0
  OR json_valid(NEW.extra_headers) = 0
  OR json_valid(NEW.kinds) = 0
  OR json_valid(NEW.protocol_configs) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid channel json');
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_json_update
BEFORE UPDATE OF models, model_mapping, extra_headers, kinds, protocol_configs ON channels
WHEN json_valid(NEW.models) = 0
  OR json_valid(NEW.model_mapping) = 0
  OR json_valid(NEW.extra_headers) = 0
  OR json_valid(NEW.kinds) = 0
  OR json_valid(NEW.protocol_configs) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid channel json');
END;
```

说明：

- `detail_api_key_id` 对历史 Playground 行为 `NULL`（原 `api_key_id` 为 `NULL`），详情查询将不再回退到其他 Key；历史正文可能不可见，这是有意的 fail-closed。
- 已发布迁移 `0008` 的 `idx_usage_logs_created_at` 与 `0001` 重名是 no-op，**不修改已发布文件**；本计划仅在文档中记录，不新增重复索引。
- 迁移前 `db.rs` 已有 VACUUM 快照逻辑，回滚按 §7 执行。

## 3. 实施阶段

依赖关系：

```
0009 迁移
  ├─ A1 L1 / A2 L2 / A3 L4 / A4 L5 / A5 L10 / A6 L11 / A7 M6
  ├─ B1 L3 / B2 L6 / B3 L9 / B4 M5
  ├─ C1 M10 / C2 M11 / C3 M12 / C4 M13 / C5 M14 / C6 M7
  └─ D1 M8（依赖 0009 + L2 的限额解析）
E1 L7 / E2 L8（可在任意阶段并行，L5 接口变更后必须同步）
F 验证与收尾
```

### 阶段 A：数据层与低风险加固

#### A1（L1）responses 归属保护

文件：`src/models/response.rs`、`src/gateway/responses_state.rs`

1. `response::insert` 返回值改为 `sqlx::Result<bool>`：是否真正写入/更新。
2. upsert 增加归属谓词：

```sql
ON CONFLICT(id) DO UPDATE SET
    model = excluded.model,
    request_json = excluded.request_json,
    response_json = excluded.response_json,
    status = excluded.status,
    created_at = excluded.created_at
WHERE responses.api_key_id = excluded.api_key_id
```

3. `execute_affected()` 为 0 时返回 `Ok(false)` 并记录 `tracing::warn!`（同 id 不同 Key）。
4. `responses_state::store` 把 `false` 当作“未持久化”，沿用现有 `store:false` 回写逻辑。
5. 验收：同一 `id` 用 Key A 写入后，Key B 写入返回 false，Key A 仍可读取原内容。

#### A2（L2）白名单解析 fail-closed

文件：`src/models/api_key.rs`、`src/gateway/auth.rs`、`src/gateway/execute.rs`、`src/gateway/mod.rs`、`src/admin/keys.rs`

1. 新增 `ApiKeyLimits { channel_ids: Vec<i64>, models: Vec<String> }` 与 `ApiKey::parse_limits() -> Result<ApiKeyLimits, AppError>`。
2. 删除/停用 `channel_ids()`、`model_names()` 的 `unwrap_or_default()` 语义；`parse_channel_ids` 的“非法值返回错误”保留。
3. `ApiKeyAuth` 在构造时解析一次并保存 `limits`；解析失败记录 `tracing::error!` 并返回 `AppError::Internal`（fail-closed），不进入路由选择。
4. `allows_model`、`channel_ids` 改为读取已解析结构。
5. 管理端列表解析失败时显示空并记录 warning，但**不影响鉴权失败关闭**。
6. 验收：手工把 `allowed_models` 改为 `{bad`，该 Key 请求返回 500 且日志有明确错误；其他 Key 不受影响。

#### A3（L4）正文保留期解耦、删 Key 清正文、secure_delete

文件：`src/maintenance.rs`、`src/models/setting.rs`、`src/admin/settings.rs`、`src/db.rs`、`web/src/pages/Settings.tsx`、`web/src/api/types.ts`

1. 新增设置 `request_detail_retention_days`（默认 7，范围 0..=3650，0 表示随 Key 删除才清理）。
2. `SettingsOutput/Input`、`from_rows`、`update` 校验、前端设置页字段同步。
3. `maintenance::cleanup_once` 用新设置独立清理 `request_details`，不再依赖 `usage_log_retention_days`。
4. 依赖 0009 的删除触发器；`request_details` 不再需要跨 Key 兜底。
5. `db.rs` 连接选项增加 `PRAGMA secure_delete = ON`（`SqliteConnectOptions::secure_delete(true)` 或启动后执行 `PRAGMA`）。若基准测试写吞吐下降 >5%，改为环境变量 `ONEAPI_SECURE_DELETE=0` 可关闭，并在 PROGRESS 记录。
6. 验收：usage 保留期设为 0 时，正文仍按 `request_detail_retention_days` 删除；删除 Key 后其正文立即消失。

#### A4（L5）日志详情按 `usage_logs.id` 查询

文件：`src/admin/logs.rs`、`src/admin/mod.rs`、`src/models/usage_log.rs`、`src/gateway/usage.rs`、`web/src/pages/Logs.tsx`

1. `UsageRecord`/`insert_one` 写入 `detail_api_key_id`（等于本次 `api_key_id`；Playground 为 `NULL`）。
2. `models::usage_log` 新增 `find_by_id(pool, id)` 与 `detail_bodies_by_id(pool, id)`：
   - 先按 `usage_logs.id` 取行；
   - 用 `(request_id, COALESCE(detail_api_key_id, 0))` 精确查正文；
   - **删除** `detail_bodies` 中“按 request_id 取最新正文”的回退分支。
3. 管理路由改为 `GET /api/logs/{id}`，`Path<i64>`；旧 `request_id` 路径删除（内部 SPA 同步修改）。
4. 前端 `Logs.tsx` 详情查询使用 `selected.id`，类型无需新增字段（`LogEntry.id` 已存在）。
5. 验收：两个 Key 使用同一个 `x-request-id`，日志列表两条；分别点开详情只看到各自正文；删除 Key 后，另一 Key 的详情不受影响。

#### A5（L10）鉴权失败限速 + peppered Key 哈希

文件：`src/security.rs`、`src/models/api_key.rs`、`src/gateway/auth.rs`、`src/admin/keys.rs`、`src/main.rs`、`Cargo.toml`

1. 新增依赖 `hmac = "0.12"`（`sha2` 已有）。
2. `Crypto` 用 HKDF/HMAC 从主密钥派生 `api_key_pepper`；新增：
   - `Crypto::api_key_hash_v2(key) -> String`（HMAC-SHA256）；
   - `Crypto::api_key_hash_legacy(key) -> String`（现有 SHA-256，保留）。
3. `api_keys` 新 Key 同时写 `key_hash`（v1）与 `key_hash_v2`；列表/展示不变。
4. 鉴权查询改为 `WHERE key_hash_v2 = ?1 OR key_hash = ?1`；若命中 v1，异步/同事务回填 v2（best effort）。
5. 新增 `AuthFailureLimiter`（`state.rs` 持有）：
   - 以 **TCP peer IP**（`ConnectInfo`，不使用 XFF）为键，滑动窗口 20 次/分钟；
   - 全局 500 次/分钟；
   - 超限返回 429，记录 `tracing::warn!`；
   - 成功一次清除该 IP 计数。
6. 任何进程内哈希比较使用 `subtle` 或直接依赖数据库唯一索引；不引入提前返回的字符串比较。
7. 验收：连续错误 Key 触发 429；旧 Key（仅 v1）仍可认证并在首次成功后写入 v2；数据库副本单独泄露时无法直接复现 v2 哈希（无主密钥）。

#### A6（L11）迁移约束

0009 已包含触发器。补充：

1. 不修改 `0001`–`0008`。
2. 在 `docs/PROGRESS.md` 记录：`0008` 的索引名与 `0001` 重复，实际生效的是 `0001` 的索引，无安全影响。
3. 验收：向 `channels.models` 写入非法 JSON 被 SQLite 拒绝；现有合法数据迁移成功。

#### A7（M6）Responses 存储配额

文件：`migrations/0009_security_fixes.sql`、`src/models/response.rs`、`src/gateway/responses_state.rs`、`src/gateway/execute.rs`、`src/models/setting.rs`、`src/admin/settings.rs`、`src/maintenance.rs`、`src/state.rs`、`web/src/pages/Settings.tsx`、`web/src/api/types.ts`

1. 0009 新增 `response_usage` 表并从 `responses` 回填（见 §2）。
2. 新增设置：
   - `response_storage_per_key_mb`：默认 64，范围 0..=1_048_576，0 表示不限；
   - `response_storage_total_mb`：默认 2048，范围 0..=1_048_576，0 表示不限。
   - `SettingsOutput/Input`、`from_rows`、`update` 校验、前端设置页同步。
3. 新增 `models::response::store_with_quota(...) -> sqlx::Result<StoreOutcome>`：
   - `StoreOutcome` 为 `Stored` / `Conflict` / `QuotaExceeded` / `TooLarge`；
   - 在一个事务内：
     1. `SELECT length(request_json) + length(response_json) FROM responses WHERE id = ? AND api_key_id = ?` 取旧尺寸；
     2. 计算 `new_size`；若超过单条 10 MiB 返回 `TooLarge`；
     3. 查 `response_usage` 该 Key 的 `bytes` 与 `(SELECT COALESCE(SUM(bytes),0) FROM response_usage)`；
     4. 若 `bytes - old + new_size > per_key_limit` 或总量超限，返回 `QuotaExceeded`；
     5. 执行带归属谓词的 upsert（A1）；
     6. `INSERT INTO response_usage (api_key_id, bytes, rows) VALUES (...) ON CONFLICT(api_key_id) DO UPDATE SET bytes = bytes - ?old + ?new, rows = rows + ?row_delta`。
4. `AppState` 增加 `response_store_lock: tokio::sync::Mutex<()>`；`store_with_quota` 调用点持有该锁，保证单实例内“检查 + 写入 + 计数”串行。
5. `responses_state::store` 将 `QuotaExceeded`/`Conflict`/`TooLarge` 一律按“未持久化”处理，返回 `false`，沿用现有 `store:false` 回写；增加 `response_store_rejected` 计数。
6. 保留期清理：`maintenance.rs` 删除 `responses` 时同步回写计数。
   - 新增 `models::response::delete_before(pool, cutoff, limit) -> Result<Vec<(i64, i64, i64)>>`（key_id、bytes、rows 的聚合），内部使用 `DELETE ... RETURNING api_key_id, length(COALESCE(request_json,'')) + length(COALESCE(response_json,''))`，同一事务内聚合并更新 `response_usage`。
   - 无 `RETURNING` 支持时（旧 SQLite）退化为先 `SELECT` 再 `DELETE`，仍在同一事务内。
7. 不改变 `store` 默认值（保持 `true`），避免破坏 `previous_response_id` 语义；配额是主要修复手段。超额请求不报错，只返回 `store:false`。
8. 单实例约束：多实例共用一个 SQLite 文件不支持；`response_store_lock` 只保证进程内串行。
9. 验收：
   - 单 Key 写入超过 64 MiB 后，新请求返回 `store:false`，磁盘不再增长；
   - 全局超过 2 GiB 后所有 Key 返回 `store:false`；
   - 保留期删除后计数下降，配额恢复；
   - 同一 `id` 覆盖写入时计数按差值更新，不重复累加。

### 阶段 B：文件、SSE 生命周期与内存

#### B1（L3）备份权限与并发

文件：`src/backup.rs`、`src/admin/backup.rs`、`src/state.rs`、`src/main.rs`

1. `backup::snapshot`：
   - 确保 `backups` 目录以 `0700` 创建/修正（`DirBuilderExt::mode` + 已存在时 `set_permissions`）；
   - 先创建 0700 的私有临时目录，`VACUUM INTO` 到其中；
   - 对结果文件 `set_permissions(0600)`；失败则删除文件并返回错误（不再只 warn）；
   - `prune`/`list` 只处理普通文件、已知前缀（`oneapi-`/`manual-`）、扩展名 `.db`，用 `symlink_metadata` 拒绝符号链接。
2. `AppState` 增加 `backup_lock: tokio::sync::Mutex<()>`；`POST /api/backup/snapshot` 用 `try_lock`，失败返回 409；增加最小间隔 10 秒（记录 `last_snapshot_at`）。
3. Unix 启动时设置 `umask(0o077)`（`libc::umask` 需新增 `libc` 依赖，或使用 `rustix`；若不想加依赖，则跳过此步，仅显式 chmod 目录与文件）。**决定：只做显式 chmod，不新增 libc 依赖。**
4. 验收：备份文件 0600、目录 0700；并发两次快照第二次 409；chmod 失败时接口返回错误且文件被删除。

#### B2（L6）指标 SSE 生命周期

文件：`src/state.rs`、`src/admin/metrics.rs`、`src/main.rs`、`src/update.rs`

1. `AppState` 增加 `shutdown: tokio::sync::watch::Sender<bool>`；`main` 在优雅停机与更新重启时 `send(true)`。
2. `metrics::stream`：
   - `tokio::select!` 监听 `shutdown.subscribe().changed()`，收到后结束流；
   - 每 30 秒用 Cookie token 的 SHA-256 查询 `sessions` 是否仍存在，不存在则结束；
   - 用 `Semaphore` 限制同时连接数（默认 4），超出返回 429。
3. `main` 的优雅停机等待增加 15 秒看门狗：超时后记录错误并退出进程（`std::process::exit(1)`），避免更新永久卡住。
4. 验收：登出/改密后指标流在 ≤30 秒内断开；`/api/update/apply` 在 SPA 打开时仍能完成重启。

#### B3（L9）zeroize 与去 panic

文件：`Cargo.toml`、`src/security.rs`、`src/upstream.rs`、`src/main.rs`、`src/admin/auth.rs`、`src/admin/setup.rs`

1. 新增依赖 `zeroize = { version = "1", features = ["derive"] }`。
2. `Crypto.key` 改为 `Zeroizing<[u8; 32]>` 或实现 `Drop` 调用 `zeroize()`。
3. `Crypto::decrypt` 返回 `Zeroizing<String>`；`upstream::build_request/apply_auth` 在构造 header 后自然释放。
4. 登录、改密、安装、`reset-password` 的密码 `String` 立即包成 `Zeroizing<String>`；`reset-password` 读取的 `line` 同样处理。
5. `dummy_password_hash` 的 `expect` 改为启动时预计算并缓存；失败时使用编译期内置的合法 PHC 常量并记录 error。
6. `main.rs` 的 `ctrl_c().expect(...)` 改为 `if let Err(e) = ... { tracing::error! }`，随后正常退出或继续等待。
7. 验收：`cargo clippy` 无 `expect_used` 新增；进程内存转储中不出现明文主密钥/密码（人工检查）。

#### B4（M5）详情队列字节上限与入队前截断

文件：`src/models/request_detail.rs`、`src/gateway/usage.rs`、`src/gateway/execute.rs`、`src/gateway/stream.rs`、`src/app.rs`

1. `request_detail::truncate` 提升为 `pub(crate) fn truncate_detail`，保持 UTF-8 边界安全。
2. `OwnedDetailRecord::from` 在复制前对 `request_body`/`response_body` 各截断到 `MAX_DETAIL_BYTES`，并记录 `size = request_id.len() + request_body.len() + response_body.len()`。
3. `WriteJob::Detail` 增加 `size: usize`。
4. `UsageWriter` 增加：
   - `queued_detail_bytes: Arc<AtomicUsize>`；
   - `MAX_DETAIL_QUEUE_BYTES: usize = 16 * 1024 * 1024`；
   - `detail_dropped: Arc<AtomicU64>`。
5. `try_enqueue_detail` 返回 `DetailEnqueue::{Queued, ByteLimit, Full}`：
   - `queued_detail_bytes + size > MAX_DETAIL_QUEUE_BYTES` → 放弃该详情、`detail_dropped += 1`、返回 `ByteLimit`（详情是可选的，不做同步写，避免阻塞请求路径）；
   - 通道满但字节未超 → `Full`，沿用现有同步写回退；
   - 成功入队 → `Queued` 并累加字节。
6. `write_loop` 消费/写入/失败后都从 `queued_detail_bytes` 减去该条 `size`（失败也不能泄漏计数）。
7. 捕获侧同样先截断：`execute.rs` 非流式详情与 `stream.rs` 的 `response_capture` 在构造 `DetailRecord` 前调用 `truncate_detail`，避免在调用栈上长时间持有完整 32 MiB/64 MiB 字符串。
8. `/api/system` 与 `/readyz` 暴露 `detail_dropped`、`detail_queue_bytes`（只读计数）。
9. 验收：
   - 发送 2 MiB 请求（开启 `store_request_bodies`）后，队列中该详情的两个字段各 ≤64 KiB；
   - 并发灌入详情到字节上限后，`detail_dropped` 增长、进程 RSS 不随请求线性增长；
   - `usage_logs` 行不受影响，成功提交的批次仍触发 `usage_total`。

### 阶段 C：网关协议与信息暴露

#### C1（M10）最近模型按 Key 隔离

文件：`src/models/usage_log.rs`、`src/gateway/mod.rs`、`src/state.rs`

1. 新增 `recent_models_for_key(pool, since, limit, api_key_id)`：

```sql
SELECT model FROM usage_logs
WHERE created_at >= ? AND api_key_id = ?
GROUP BY model
ORDER BY MAX(created_at) DESC
LIMIT ?
```

2. `/v1/models` 的 `has_wildcard` 分支改用该函数，并在插入前用 `auth.limits.models` 过滤（`allows_model`）。
3. 管理端 `admin/catalog.rs` 继续使用全局 `recent_models`（管理员可见全站）。
4. 移除 `RuntimeCache` 对网关侧最近模型的全局缓存引用（或改为按 Key 短缓存），避免跨 Key 复用。
5. 验收：Key A 使用 `model-a`，Key B 使用 `model-b`；B 调 `/v1/models` 不含 `model-a`；管理员目录仍含两者。

#### C2（M11）客户端错误脱敏与路由头

文件：`src/gateway/error.rs`、`src/gateway/execute.rs`、`src/gateway/stream.rs`、`src/gateway/mod.rs`

1. `gateway/error.rs` 新增：
   - `redact_urls(&str) -> String`：把 `http://`/`https://` 开头的连续非空白片段替换为 `[url]`；
   - `client_safe_upstream_message(error_type, raw, is_provider_json) -> String`：
     - `upstream_unreachable` → `"upstream request failed"`；
     - `upstream_configuration_error` → `"upstream channel is misconfigured"`；
     - `upstream_stream_timeout` → `"upstream stream timed out"`；
     - 非 JSON 上游错误 → `"upstream returned an invalid response"`；
     - 结构化 provider error（4xx）→ `redact_urls(message)` 截断 300 字符。
2. `execute.rs` 的 `finish_failover` 与 `stream.rs` 的 `error_frame` 只使用上述安全文案；完整 `last_message` 仅进入日志、`mark_channel_failure`、`usage_logs.error_message`。
3. 路由头 `x-oneapi-channel` / `x-oneapi-upstream-model` 仅在 `identity.source == playground` 时写入；API Key 请求不返回。
4. `gateway/mod.rs` 的 CORS 从 `CorsLayer::permissive()` 改为显式构造：允许任意 Origin/Method/Header，但**不调用 `expose_headers`**。
5. 验收：API Key 请求遇到连接失败只看到通用文案；Playground 仍能显示渠道与上游模型；浏览器 JS 无法读取 `x-oneapi-*`（除非 Playground）。

#### C3（M12）慢客户端不再误冷却渠道

文件：`src/gateway/stream.rs`

1. `WaitOutcome` 增加 `ClientBlocked`。
2. 心跳发送改为竞争：

```rust
tokio::select! {
    _ = tx.closed() => return WaitOutcome::Disconnected,
    _ = tokio::time::sleep(deadline.saturating_duration_since(Instant::now())) =>
        return WaitOutcome::ClientBlocked,
    result = tx.send(Bytes::from_static(HEARTBEAT_FRAME)) => {
        if result.is_err() { return WaitOutcome::Disconnected; }
    }
}
```

3. `run` 中 `ClientBlocked` 记为 `client_slow`、状态码 499，**不得**调用 `mark_channel_failure`。
4. 只有上游侧错误（读取失败、上游空闲超时、协议解析失败）才标记渠道失败。
5. 验收：模拟客户端暂停读取（`curl` 限速/停止消费），服务端在空闲阈值后以 499 结束，渠道 `consecutive_failures` 不增加。

#### C4（M13）SSE 内容类型与终止事件

文件：`src/gateway/execute.rs`、`src/gateway/stream.rs`、`src/protocol/stream.rs`、`src/protocol/sse.rs`

1. `execute.rs` 流式路径在 `status.is_success()` 后：
   - 读取 `Content-Type`；若存在且不包含 `text/event-stream`，读取有界正文，按 `upstream_invalid_content_type` 处理、标记渠道失败并继续下一个候选；
   - 若头部缺失，继续，但 `frame_error` 在 `data` 为空且 `raw` 非空时返回 `upstream_invalid_frame`。
2. 为三个解码器增加 `saw_terminal: bool` 与 `StreamDecoder::saw_terminal()`：
   - Chat：收到 `data: [DONE]`；
   - Anthropic：收到 `message_stop`；
   - Responses：收到 `response.completed` / `response.incomplete` / `response.failed`。
3. `gateway/stream.rs` 在 `decoder.finish()` 前检查：若未观察到终止事件且无 `stream_error`，设置 `upstream_stream_truncated`，向客户端合成 error 帧，HTTP 502，并标记渠道失败。
4. 同协议透传路径在 `observe_passthrough` 中记录终止标记；未见终止同样报错。
5. `response.failed` 无论是否带 `error` 字段都产出 error 事件，不写入 `final_response`，不持久化。
6. Chat 编码器 error 分支不再输出 `data: [DONE]`。
7. 验收：假上游返回 200 HTML → 502/切换；流中途断开 → 客户端收到 error 且状态 502；正常流不受影响。

#### C5（M14）协议语义与计费算术

文件：`src/protocol/chat.rs`、`src/protocol/anthropic.rs`、`src/protocol/responses.rs`、`src/protocol/ir.rs`、`src/protocol/mod.rs`、`src/gateway/usage.rs`

1. **Chat extras**：`chat::encode_request` 把 `request.extras` 中的已知字段（`logit_bias`、`presence_penalty`、`frequency_penalty`、`seed`、`modalities`、`audio`）写回目标 JSON（若目标未显式设置）。
2. **tool_result.is_error**：Chat 编码时若 `is_error=true`，在文本前加机器可读前缀 `[tool_error]`；在 `docs/IMPLEMENTATION_PLAN.md` §5 记录该有损映射。
3. **未知输入显式 400**：未知 content block、未知 tool 类型、未知 `response_format.type`、非字符串 `stop` 项、类型/范围非法的数值字段（`max_tokens`、`n`、temperature、top_p）返回 `ProtocolError::unsupported`/`invalid_request`。
4. **usage 饱和**：`ir.rs`、`chat.rs`、`anthropic.rs`、`responses.rs` 中所有 token 加法改 `saturating_add`；每个字段上限 `1_000_000_000`，超出按上游错误处理（丢弃该字段或 502）。
5. **计费兜底**：`gateway/usage.rs` 的 `i64::try_from(...).unwrap_or(0)` 改为失败时回退本地估算（不产生 0）。
6. **Responses status**：`status` 非 `completed`/`incomplete`（`failed`、`cancelled`、`queued`、`in_progress`、缺失）→ `ProtocolError::invalid_response`；`incomplete` 映射 `MaxTokens`。
7. **Anthropic**：`type != "message"` → 上游错误；`stop_reason` 缺失 → 上游错误；`pause_turn` 显式映射。
8. **拒绝/未知结束原因**：Chat 解析 `message.refusal` 为内容 + `StopReason::ContentFilter`；新增 `StopReason::Unknown(String)` 保存原始 `finish_reason`，Chat 编码输出原始字符串，Anthropic 映射 `end_turn` 并 warning，Responses 置 `incomplete`。
9. 验收：跨协议转换测试覆盖 extras、is_error、未知块、超大 usage；`cargo test` 对应单测通过。

#### C6（M7）`model` 与 `previous_response_id` 长度限制

文件：`src/gateway/execute.rs`、`src/gateway/responses_state.rs`、`src/gateway/usage.rs`、`src/models/usage_log.rs`

1. 新增常量：
   - `MAX_MODEL_NAME_LEN = 256`；
   - `MAX_RESPONSE_CURSOR_LEN = 128`，允许字符 `[A-Za-z0-9_-]`。
2. `execute::proxy` 提取 `model` 后立即校验：
   - 长度超过 256 或含控制字符 → 按入站协议返回 400 `invalid model name`；
   - 在调用上游、构造详情、写 usage 之前返回，避免超长值落库；该请求不产生 usage 行（文档记录此行为）。
3. `responses_state::expand_previous` 在查询前校验 `previous_response_id`：
   - 长度/字符集不合法 → `ExpandError::Invalid("previous_response_id is invalid")`；
   - 错误消息**不回显**原始 cursor（与 M11 的脱敏原则一致）。
4. 写入侧兜底：`gateway/usage.rs::insert_one` 对 `model`、`upstream_model`、`error_type`、`error_message` 做 UTF-8 安全截断（256/256/128/1024），截断时 `tracing::warn!`。这样即使内部路径（上游模型名、错误文本）异常，也不会把超长值写入带索引的 `usage_logs.model`。
5. `x-request-id` 现有 128 字符限制保持不变。
6. 验收：
   - 1 MiB 的 `model` 返回 400，`usage_logs` 无该行；
   - 1 MiB 的 `previous_response_id` 返回 400，响应中不含原始值；
   - 正常 `gpt-4o`/`resp_xxx` 不受影响。

### 阶段 D：配额预留（M8，最高风险，最后做）

文件：`src/models/api_key.rs`、`src/gateway/auth.rs`、`src/gateway/execute.rs`、`src/gateway/stream.rs`、`src/gateway/usage.rs`、`src/main.rs`、`src/state.rs`

1. **准入预留**：
   - 估算 `estimate = input_token_estimate + max_tokens`（`max_tokens` 取请求值，缺省用 `defaults.default_max_tokens`，saturating）；
   - 在 `execute::proxy`/`proxy_stream` 选择渠道前调用 `api_key::reserve(&db, key_id, estimate, day, now)`；
   - 原子 SQL（`?1`=estimate, `?2`=day, `?3`=now, `?4`=id）：

```sql
UPDATE api_keys
SET reserved_tokens = reserved_tokens + ?1,
    reserved_daily_tokens = CASE WHEN reserved_day = ?2 THEN reserved_daily_tokens + ?1 ELSE ?1 END,
    reserved_day = ?2,
    reserved_at = ?3
WHERE id = ?4
  AND enabled = 1
  AND (token_limit IS NULL OR token_limit <= 0
       OR token_usage + reserved_tokens + ?1 <= token_limit)
  AND (daily_token_limit IS NULL OR daily_token_limit <= 0
       OR (CASE WHEN daily_usage_day = ?2 THEN daily_token_usage ELSE 0 END)
          + (CASE WHEN reserved_day = ?2 THEN reserved_daily_tokens ELSE 0 END)
          + ?1 <= daily_token_limit)
```

   - `rows_affected == 1` 才放行；否则 403（累计额度）或 429（每日额度），与现有错误语义一致。
2. **预留释放**：
   - 新增 `ReservationGuard { state, key_id, estimate, day, armed: bool }`；
   - 请求正常结束、把 `UsageRecord` 交给写队列后 `disarm()`（由批量事务结算）；
   - 任一提前返回/取消/panic，`Drop` 中 `tokio::spawn` 调用 `release_reservation`；
   - 失败路径（未产生 usage 行）立即释放。
3. **结算**：
   - `write_batch` 按 Key 汇总 `reserved_amount` 与 `actual_tokens`；
   - 在同一事务中先 `reserved_tokens = MAX(0, reserved_tokens - reserved)` / 日预留同理，再 `add_usage(actual)`；
   - 批次失败时保留预留，由 janitor 清理。
4. **陈旧预留清理**：
   - 启动时 `UPDATE api_keys SET reserved_tokens=0, reserved_daily_tokens=0`；
   - 每 60 秒清理 `reserved_at < now - 30min` 的预留（覆盖崩溃/取消）；
   - 暴露 `reserved_stale_resets` 计数到 `/api/system` 或日志。
5. **紧急开关**：`ONEAPI_DISABLE_QUOTA_RESERVATION=1` 时跳过预留，仅保留现有事后记账（回滚用）。
6. **注意**：Responses 的输入估算必须在 `expand_previous` **之后**计算，否则历史链不计入预留（当前代码在展开前估算，需调整顺序）。
7. 验收：
   - 剩余额度 1000、并发 20 个 `max_tokens=4096` 请求，仅允许约 0 个或按实际剩余放行，不会全部通过；
   - 取消/断开的长流在 30 分钟内释放预留；
   - 正常请求的 `token_usage` 与实际一致，无重复计数。

### 阶段 E：前端

#### E1（L7）

文件：`web/src/hooks/useLiveMetrics.tsx`、`web/src/App.tsx`、`web/src/api/client.ts`、`web/src/pages/Logs.tsx`、`web/src/pages/Settings.tsx`、`web/src/pages/Keys.tsx`、`web/src/pages/Channels.tsx`

1. SSE 401：调用共享 `onUnauthorized()`（`invalidateQueries(['me'])` + `window.location.assign('/')`）；登录成功后重建 SSE。
2. `client.ts`：`request()` 校验 path 以 `/api/` 开头且不含 `..`/`//`；所有动态段用 `encodeURIComponent`。
3. `Logs.tsx` 详情改用 `selected.id`（配合 L5），不再拼 `request_id`。
4. `Settings.tsx`：`settingsQuery.isError` 时只渲染错误态与重试，不渲染可编辑默认值；保存时只提交被修改字段（或至少禁止在错误态保存）。
5. `Keys.tsx`/`Channels.tsx`：编辑提交不包含未改动的 `enabled`；仅在用户显式切换时发送。
6. `Keys.tsx`：`submit()` 开头 `if (saveMutation.isPending) return;`；创建成功后 `saveMutation.reset()`；pending 时禁用输入框。
7. 验收：手动触发 401 后回到登录页；设置接口失败时无法用默认值覆盖；连续回车只创建一个 Key。

#### E2（L8）

文件：`web/src/components/ui/chart.tsx`

1. 移除 `dangerouslySetInnerHTML`；改为 `useEffect` + `ref` 设置 `<style>` 的 `textContent`。
2. 在生成 CSS 前校验：
   - `chartId` 匹配 `^[A-Za-z0-9_-]+$`；
   - config key 匹配 `^[A-Za-z0-9_-]+$`；
   - color 匹配 `^(#[0-9a-fA-F]{3,8}|rgb\(|rgba\(|hsl\(|hsla\(|oklch\(|var\(--[A-Za-z0-9_-]+\))$`。
   - 不匹配则跳过该变量并 `console.warn`（仅开发环境）。
3. 验收：图表颜色正常；构造非法 config 时不会向 DOM 注入样式。

## 4. 测试计划

### 4.1 单元测试（模块内 `#[cfg(test)]`）

| 项 | 测试 |
|---|---|
| L1 | 两个 Key 写同一 response id：第二个返回 false，第一个内容不变 |
| L2 | 非法 JSON 限额：`parse_limits` 返回 Err；鉴权 fail-closed |
| L5 | 同 request_id 两个 Key：`detail_bodies_by_id` 只返回各自正文 |
| L10 | v1 哈希可认证并回填 v2；v2 查询命中；限速器窗口/封禁 |
| M5 | 2 MiB 详情入队后字段 ≤64 KiB；字节上限触发丢弃计数；写入后计数归零 |
| M6 | 每 Key/全局配额；覆盖写入按差值计数；清理后配额恢复 |
| M7 | 超长 model 400；超长 cursor 400 且不回显；写入侧截断 |
| M8 | 预留原子性（并发 20）、释放、结算、陈旧清理 |
| M10 | `recent_models_for_key` 只返回指定 Key |
| M11 | `redact_urls` 去除 URL；错误映射表 |
| M12 | `wait_for_upstream` 在通道满时返回 `ClientBlocked` 而非 `TimedOut` |
| M13 | 三个解码器的终止事件检测；`response.failed` 无 error 仍产错；Chat error 不发 `[DONE]` |
| M14 | extras 回填；is_error 前缀；usage 饱和；Responses status 失败 |

### 4.2 手工冒烟

1. 起一个假上游：返回 200 HTML → 客户端收到 502/通用错误，渠道失败计数增加一次。
2. 假上游正常流但缺少终止事件 → 客户端收到 error，HTTP 502。
3. 慢客户端：发起流式请求后停止读取 → 499，渠道失败计数不增加。
4. 两个 Key 相同 `x-request-id` → 日志两条，详情按 `id` 各自正确。
5. 备份并发两次 → 第二次 409；文件 0600、目录 0700。
6. 删除 Key → `request_details` 立即无该 Key 行。
7. 配额：剩余额度 1000，20 并发长请求 → 不出现全部放行。
8. 设置页 GET 失败 → 只显示错误态，不能保存默认值。
9. 开启 `store_request_bodies`，发送 2 MiB 请求 → 队列字节计数受控，进程 RSS 不随请求线性增长。
10. Responses 单 Key 写满配额 → 返回 `store:false`；清理保留期后恢复写入。
11. `model`/`previous_response_id` 传 1 MiB → 400，响应不含原始值。

### 4.3 回归验证

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo build --release --locked`
- `cd web && npm run lint && npm run build`
- 手工跑一遍 `/v1/chat/completions`、`/v1/responses`、`/v1/messages` 的非流式/流式，确认协议转换无回归。

## 5. API 变更清单

| 变更 | 影响 |
|---|---|
| `GET /api/logs/{request_id}` → `GET /api/logs/{id}` | 仅内部 SPA；同步修改前端 |
| `GET /api/settings` 新增 `request_detail_retention_days` | 前端设置页同步 |
| `GET /api/settings` 新增 `response_storage_per_key_mb`、`response_storage_total_mb` | 前端设置页同步 |
| `/api/system`、`/readyz` 新增 `detail_dropped`、`detail_queue_bytes`、`response_store_rejected` | 只读计数 |
| Responses 超出存储配额时返回 `store:false` | 不报错，客户端按未持久化处理 |
| `/v1/models` 结果按 Key 过滤 | 行为收紧 |
| `/v1/*` 不再返回 `x-oneapi-channel`/`x-oneapi-upstream-model`（Playground 除外） | 前端 Playground 不受影响 |
| `responses` 冲突写入返回 `store:false` | 客户端行为正确 |

## 6. 风险与回滚

| 风险 | 缓解 |
|---|---|
| M8 预留过严导致合法请求被拒 | 预留额按 `input + max_tokens`；提供 `ONEAPI_DISABLE_QUOTA_RESERVATION=1`；上线前用真实流量回放 |
| M8 结算遗漏导致预留泄漏 | 启动清零 + 60 秒 janitor；`reserved_stale_resets` 告警 |
| L10 哈希迁移导致旧 Key 失效 | 保留 v1 查询与写入；命中 v1 惰性回填；回滚旧二进制仍可认证 |
| M13 严格终止事件导致部分兼容上游被拒 | 仅在“无终止事件”时报错；Content-Type 缺失时仍按首帧判定；发布说明标注 |
| L5 接口变更影响外部脚本 | 仅内部 SPA 使用；文档标注 breaking |
| L4 `secure_delete` 性能下降 | 压测；超过 5% 可 `ONEAPI_SECURE_DELETE=0` 关闭 |
| M5 字节上限导致详情被丢弃 | 详情非计费数据；`detail_dropped` 告警；usage 行不受影响 |
| M6 配额计数与实际行不一致 | 同一事务维护 `response_usage`；保留期删除用 `RETURNING` 回写；单测对账 |
| M6 多实例并发写同一 SQLite | 明确不支持多实例共用文件；`response_store_lock` 仅进程内 |
| 0009 迁移失败 | 迁移前 `db.rs` 自动 VACUUM 快照；失败停止启动，按备份恢复 |

回滚步骤：

1. 停止服务。
2. 用迁移前快照覆盖 `oneapi.db`（`data/backups/`）。
3. 回退二进制到上一版本（保留 v1 Key 哈希可继续工作）。
4. 若已产生新数据，先导出 `api_keys`、`usage_logs` 再恢复。

## 7. 工作量与顺序

| 阶段 | 内容 | 预估（人日） |
|---|---|---|
| 0 | 0009 迁移与本地验证 | 1 |
| A | L1/L2/L4/L5/L10/L11 + M6 | 7 |
| B | L3/L6/L9 + M5 | 3 |
| C | M10–M14 + M7 | 6 |
| D | M8 配额预留 | 4 |
| E | L7/L8 前端 | 2 |
| F | 测试、回归、文档 | 2 |
| 合计 | | **25 人日** |

建议提交拆分（每项一个 PR/提交，便于回滚）：

1. `chore(db): 0009 安全修复迁移`
2. `fix(keys): 白名单解析 fail-closed`（L2）
3. `fix(responses): 冲突写入校验归属`（L1）
4. `fix(logs): 详情按 usage_logs.id 查询`（L5）
5. `fix(backup): 权限与并发`（L3）
6. `fix(metrics): SSE 生命周期`（L6）
7. `fix(security): zeroize 与去 expect`（L9）
8. `feat(security): API Key HMAC(prepper) 哈希与鉴权限速`（L10）
9. `fix(retention): 正文保留期解耦与 secure_delete`（L4）
10. `fix(gateway): 错误脱敏与路由头`（M11）
11. `fix(models): 最近模型按 Key 过滤`（M10）
12. `fix(stream): 慢客户端与终止事件`（M12/M13）
13. `fix(protocol): 语义与计费算术`（M14）
14. `feat(quota): 准入预留与结算`（M8）
15. `fix(web): 认证状态、请求路径、设置与表单`（L7）
16. `fix(web): chart 样式安全注入`（L8）
17. `feat(responses): 存储配额与计数`（M6，阶段 A）
18. `fix(usage): 详情队列截断与字节上限`（M5，阶段 B）
19. `fix(gateway): model 与 previous_response_id 长度校验`（M7，阶段 C）

## 8. 完成定义

- [ ] `migrations/0009_security_fixes.sql` 在全新库与升级库上均成功执行
- [ ] 20 项均有对应代码改动与验收记录
- [ ] 新增单测全部通过；`cargo fmt`/`clippy`/`build --locked`/前端 lint+build 通过
- [ ] 手工冒烟 8 项全部通过
- [ ] `docs/PROGRESS.md` 记录本次修复批次与遗留风险
- [ ] `docs/IMPLEMENTATION_PLAN.md` §5 记录 `[tool_error]` 有损映射
- [ ] AOCI 维护完成，`aoci.code.txt` 与本文件一致
- [ ] 未触碰 §0.2 中的任何项（diff 审查确认）
