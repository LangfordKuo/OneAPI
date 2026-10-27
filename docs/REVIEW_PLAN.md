# OneAPI 审查与优化计划

- 状态：草稿（本地文档，未提交、未推送）
- 审查基线：`v0.1.1`（commit `e746cd9`）
- 审查日期：2026-10-27
- 验证基线：`cargo test --locked` 通过，72 个测试；源码无 TODO/FIXME
- 关联文档：`docs/IMPLEMENTATION_PLAN.md`、`docs/PROGRESS.md`

本文档把 2026-10-27 的全仓审查结论整理为可执行任务。每项包含现状、证据位置、方案与验收标准，完成后勾选并记录到 `docs/PROGRESS.md`。

## 0. 优先级与估算

| 级别 | 含义 | 处理阶段 |
|---|---|---|
| P0 | 功能不一致、用户可见缺陷、与实施计划偏离 | 阶段 A（v0.1.2） |
| P1 | 安全、稳定性、部署风险 | 阶段 B（v0.2） |
| P2 | 性能与资源效率 | 阶段 C（v0.2） |
| P3 | 工程卫生、文档同步 | 阶段 D（随版本顺手清理） |

估算：S ≈ 半天以内，M ≈ 1–2 天，L ≈ 3 天以上（含测试与验证）。

---

## 阶段 A：v0.1.2 修复与一致性（P0）

### A1. 接线三个“死配置”设置项（M）

现状：`default_max_tokens`、`nonstream_timeout_seconds`、`stream_idle_timeout_seconds` 在设置页可编辑并入库，但网关从不读取。

证据：
- `src/admin/settings.rs:99-124` 只做校验与 upsert；
- `src/protocol/anthropic.rs:357` 缺 `max_tokens` 时硬编码 `4096`；
- `src/upstream.rs:8-9` 超时使用常量 600s/120s，仅渠道 `timeout_ms` 可覆盖。

方案：
1. 在 `AppState` 增加设置快照缓存（或复用 C2 的缓存），网关请求入口读取一次。
2. 跨协议编码到 Anthropic 且 `ir.max_tokens.is_none()` 时，用 `default_max_tokens` 填充；同协议透传保持客户端原样。
3. `build_client` / `build_stream_client` / `stream_idle_timeout` 接收默认超时参数：渠道值优先，其次设置值，最后才用常量兜底。
4. 删除 `anthropic.rs` 中硬编码的 4096 兜底，或保留为设置读取失败时的最后防线。

验收：
- 修改设置后无需重启即生效（或明确要求重启并在 UI 标注）。
- 新增测试：未指定 `max_tokens` 的 Chat→Anthropic 请求使用设置值；渠道未配 `timeout_ms` 时使用设置超时。

### A2. 让 `site_name` 真正显示（S）

现状：`site_name` 只存库，`web/src/App.tsx:140` 与 `web/src/pages/Login.tsx:21` 硬编码 “OneAPI”。

方案：
1. 新增公开只读接口 `GET /api/site`（返回 `site_name` 与 `version`），允许未登录访问，供登录页与安装页使用。
2. 前端在 App 启动时查询该接口，用于侧边栏标题、登录页标题、安装页默认值与 `document.title`。
3. 设置保存后使该查询失效。

验收：修改站点名称后，登录页、侧边栏与浏览器标题同步更新；不泄露其他配置。

### A3. 补 `POST /v1/messages/count_tokens`（M）

现状：`docs/IMPLEMENTATION_PLAN.md:172` 已列入计划，但 `src/gateway/mod.rs:25-38` 未注册，Anthropic SDK 类客户端会 404。

方案：
1. 注册路由，复用 `ApiKeyAuth` 与现有网关鉴权/错误形状。
2. 若选中渠道为 Anthropic，原样转发到 `{base}/messages/count_tokens`（同协议透传，仅替换 `model`）。
3. 其他上游按字符数估算 `{"input_tokens": n}`，并在响应头或字段中标注为估算（保持 Anthropic 响应形状）。
4. 该端点不写 `usage_logs`（不计费、无实际生成），错误仍按 Anthropic 形状返回。

验收：Anthropic 上游转发成功；非 Anthropic 上游返回结构合法的估算值；错误码与 `/v1/messages` 一致。

### A4. 修复通配符渠道导致模型列表为空（S）

现状：`src/gateway/mod.rs:170-176` 与 `src/admin/catalog.rs:27-31` 显式排除 `*`。仅配置 `*` 的渠道会让 `/v1/models`、`/api/models` 和 Playground 下拉为空，但实际可以路由。

方案（二选一，建议 1+2）：
1. 模型列表 = 启用渠道的显式模型 ∪ 最近 N 天 `usage_logs.model` 去重结果（仅用于展示）。
2. 若结果仍为空但存在启用的通配符渠道，返回一个明确标记（例如列表包含 `*` 或额外字段 `wildcard: true`），前端提示“该渠道接受任意模型，请手动输入”。

验收：仅有通配符渠道时，Playground 至少能手动输入模型并成功调用；已有显式模型时列表不重复、不丢失。

### A5. 渠道测试/拉取模型支持未保存配置（M）

现状：`src/admin/mod.rs:41-45` 仅有 `POST /api/channels/{id}/test` 与 `{id}/fetch-models`；前端也标注“使用已保存的配置”（`web/src/pages/Channels.tsx:312-314`）。实施计划第 8 节要求支持草稿配置。

方案：
1. 新增 `POST /api/channels/test` 与 `POST /api/channels/fetch-models`，接收 `ChannelInput`（创建场景 `api_key` 必填；编辑场景可带 `id` 表示“留空沿用已保存密钥”）。
2. 复用现有 `validate`、`send_test`、`send_models`，不写库。
3. 前端抽屉里的“测试连通性/拉取模型”改为提交当前表单值；保留列表行的“测试”按钮走已保存配置。

验收：新建渠道时无需先保存即可测试与拉取模型；编辑时留空 Key 仍能测试；敏感信息不出现在响应中。

### A6. 转发客户端协议相关请求头（S）

现状：`src/upstream.rs:84-113` 只应用渠道 `extra_headers` 与默认 `anthropic-version`，客户端传入的 `anthropic-beta`、`OpenAI-Beta` 等被丢弃，影响长上下文、prompt caching 等能力。

方案：
1. 建立白名单（如 `anthropic-beta`、`anthropic-version`、`openai-beta`、`openai-organization`、`openai-project`），由客户端头覆盖渠道默认值。
2. 绝不转发 `Authorization`、`x-api-key`、`Cookie`、`Host`。
3. 白名单可通过渠道 `extra_headers` 继续扩展。

验收：`anthropic-beta` 能到达 Anthropic 上游；鉴权头仍只使用渠道密钥；新增单元测试覆盖覆盖/拒绝逻辑。

### A7. 持久化 `cache_creation_tokens`（S）

现状：IR 已解析该字段（`src/protocol/ir.rs:146`），但 `usage_logs` 无对应列，`src/gateway/usage.rs` 未绑定，缓存写入成本无法单独分析。

方案：
1. 新增迁移 `migrations/0002_usage_cache_creation.sql`（已发布迁移不可修改），增加 `cache_creation_tokens INTEGER`。
2. `UsageRecord` / `usage::insert` / 日志查询模型 / `LogEntry` / 前端类型与详情页同步展示。
3. 仪表盘汇总可选增加 `SUM(cache_creation_tokens)`。

验收：Anthropic 上游返回的 `cache_creation_input_tokens` 出现在日志详情；旧数据为 NULL 不影响查询。

### A8. 修正日志页空态文案（S）

现状：`web/src/pages/Logs.tsx:461-467` 提示“当前版本未保存请求与响应正文”，与已实现的功能矛盾。

方案：改为“未记录请求/响应体”，并提示“可在设置 → 记录请求/响应体中开启（单条最多 64KB）”。

验收：开关关闭时提示准确；开启后正常展示正文。

### A9. 后端校验渠道权重范围（S）

现状：`src/admin/channels.rs:263` 只做 `max(1)`，无上限；`src/gateway/select.rs:60-61` 对权重求和，极大值在 debug 构建下可能溢出 panic。

方案：后端将 `weight` 限制在 `1..=1000`（与前端一致），越界返回 400；顺带校验 `priority` 在合理区间（如 `-10000..=10000`）。

验收：提交 `weight = i64::MAX` 返回 400；现有渠道与测试不受影响。

---

## 阶段 B：安全与稳定性（P1）

### B1. 收紧 `/api/system`（S）

现状：`src/app.rs:16,48-58` 公开返回 `data_dir` 绝对路径。

方案：该接口改为需要管理员会话；或至少移除 `data_dir`，仅保留名称、版本、运行时长、数据库状态。前端 Dashboard 已登录，改动无感。

验收：未登录访问返回 401；Dashboard 正常显示。

### B2. 管理员登录限速与失败退避（M）

现状：`src/admin/auth.rs:73` 无任何限制；Argon2 校验成本高，公网暴露时存在暴力破解与 DoS 风险。

方案：
1. 进程内按 `IP` 与 `username` 维护失败计数，指数退避（例如 5 次失败后 30s、5 分钟、15 分钟）。
2. 超限返回 429 并带 `Retry-After`。
3. 成功登录清零；计数器带过期清理，避免内存无限增长。
4. 反向代理场景下正确获取客户端 IP（与 B3 的信任代理配置共用）。

验收：连续失败触发 429；正常用户不受影响；新增单元测试。

### B3. 修正反向代理下的 CSRF 同源判断（M）

现状：`src/admin/csrf.rs:23-40` 直接比较 `Origin` 与 `Host`。反代改写 `Host` 时会误拒所有写请求。

方案：
1. 新增 `ONEAPI_TRUST_PROXY` 开关（默认关闭）。
2. 开启时优先使用 `X-Forwarded-Host` / `Forwarded` 进行同源比较；未开启时保持现状。
3. 文档说明该开关只应在受信反代后启用。

验收：模拟反代头时同源请求通过；跨源请求仍被拒绝；默认配置行为不变。

### B4. 限制 Responses 历史展开规模（M）

现状：`src/gateway/responses_state.rs:44-59` 无上限。单条响应限 10MB，但多轮续接会让展开后的请求持续增长，并每轮完整重存历史。

方案：
1. 设置最大展开条目数与最大 JSON 字节数（例如 2000 条 / 16MB）；超限返回 400 并说明。
2. 长期方案：按轮存增量，仅在读取时拼接；保留对旧数据的兼容。
3. 补充测试：超限拒绝、正常多轮不受影响。

验收：构造超长历史时返回明确 400；数据库不会因单请求无限增长。

### B5. 过期会话纳入周期清理（S）

现状：`src/admin/auth.rs:153` 仅在登录时 `delete_expired`。

方案：在 `maintenance::cleanup_once` 中增加 `DELETE FROM sessions WHERE expires_at < now`，沿用 6 小时周期。

验收：构造过期会话后清理任务能删除；登录流程不变。

### B6. 明确在线更新的信任模型（S/M）

现状：`src/update.rs:116-124` 只校验同 Release 的 `SHA256SUMS`，无签名。

方案：
1. 文档明确“信任 GitHub Release 账号 + HTTPS”的威胁模型。
2. 可选：支持 minisign/GPG 签名校验（新增公钥配置项），默认关闭。
3. 更新完成后清理 `data_dir/updates` 中的临时文件。

验收：文档有明确说明；临时文件不残留。

### B7. 指标 SSE 在未登录时停止重连（S）

现状：`web/src/hooks/useLiveMetrics.ts` 使用 EventSource，401 后会持续重试。

方案：EventSource 无法读取状态码，改为先探测 `/api/auth/me`，或改用 fetch + ReadableStream（与 Playground 的 `readSse` 复用），遇到 401 关闭并不再重连。

验收：登出后不再产生持续请求；重新登录后恢复。

### B8. 为长连接增加心跳（可选，S）

现状：`src/admin/metrics.rs` 每秒有数据帧，本身即心跳；网关 SSE 无下游心跳。

方案：仅在网关流式响应空闲超过阈值（如 15s）时发送 SSE 注释行 `: keep-alive`，避免中间代理断开。注意不要影响 usage 统计与正文捕获。

验收：慢速上游下代理不再因空闲断开；现有流式测试通过。

---

## 阶段 C：性能与资源效率（P2）

### C1. 缓存每个渠道的 reqwest Client（L，收益最高）

现状：`src/gateway/execute.rs:325`、`:791` 每次尝试都新建 Client，连接池与 TLS 会话无法复用。

方案：
1. 在 `AppState` 增加客户端缓存（`RwLock<HashMap<ChannelCacheKey, Client>>`），键包含渠道 ID 与 `updated_at`，或包含 base_url/proxy/timeout 的配置指纹。
2. 渠道创建/更新/删除后失效对应条目；设置默认超时变更时整体失效。
3. 非流式与流式分别缓存，保持现有超时语义。

验收：压测下 QPS 与 p95 延迟改善；渠道更新后立即使用新配置；内存占用可控（渠道数量级）。

### C2. 缓存渠道列表与设置快照（M）

现状：`execute.rs:208` 每请求 `channel::list` 全表 + 逐条 JSON 解析；`execute.rs:80` 每请求读取 `store_request_bodies`。

方案：
1. 进程内缓存渠道列表（含解析后的 models / mapping / extra_headers）与设置快照。
2. 渠道与设置的写接口成功后主动失效；可选 TTL 兜底。
3. 缓存需考虑多实例部署：文档注明当前为单实例内存缓存，多实例下以 TTL 兜底。

验收：单请求的数据库读次数下降；渠道修改在 TTL 或立即失效后生效。

### C3. 削减写放大并引入日志写入队列（L）

现状：
- 每次成功请求都执行 `mark_success`（`execute.rs:397`、`stream.rs:169`），即使渠道本就健康；
- 每次请求直接插入 `usage_logs`，未实现计划中的单写队列（`IMPLEMENTATION_PLAN.md:80,224`）。

方案：
1. `mark_success` 增加条件：仅当 `consecutive_failures <> 0 OR last_error IS NOT NULL OR disabled_until IS NOT NULL` 时更新。
2. 实现 mpsc 单写任务：usage 记录入队，批量事务写入（例如每 100 条或 200ms），队列满时降级为同步写并告警。
3. 进程退出时 flush 队列；`readyz` 可选暴露队列深度。

验收：高并发下写锁冲突与延迟下降；退出前不丢日志；现有日志测试通过。

### C4. 启用 HTTP 压缩（S）

现状：`Cargo.toml:35` 启用了 `compression-gzip`，但 `app.rs` 未挂 `CompressionLayer`。

方案：对 `/api` 与静态资源启用压缩，显式排除 `text/event-stream` 与已压缩资源；确认与 `Cache-Control` 组合正确。

验收：JSON 与 JS 响应带 `Content-Encoding: gzip`；SSE 不被压缩或缓冲。

### C5. 优化日志查询（M）

现状：`src/models/usage_log.rs:58-67` 的 `model`/`request_id` 使用 `%LIKE%`，无法走索引；每次列表额外 `COUNT(*)`（`:128-130`）；深分页 OFFSET 变慢。

方案：
1. 模型与请求 ID 支持精确匹配（默认）与模糊匹配（显式 `like=1`），或改为前缀匹配。
2. 为高频组合增加复合索引（如 `(status_code, created_at)`、`(source, created_at)`）。
3. 评估游标分页（`created_at, id`），保留 offset 兼容。
4. 前端“共 N 条”可改为可选查询，减少一次 COUNT。

验收：百万级日志下列表响应时间明显下降；筛选语义与文档一致。

### C6. 减少大请求体的重复克隆（S）

现状：`execute.rs:272` 对每个候选渠道 `body.clone()`，32MB 请求体可能被多次复制。

方案：首次尝试直接序列化，失败切换时才按需克隆/重新构造；或使用 `Arc<Value>` 共享只读部分。

验收：多渠道路由时内存峰值下降；行为不变。

### C7. 降低实时指标锁竞争（S）

现状：`src/metrics.rs:96` 每个流式 chunk 抢一次 `Mutex`。

方案：改为按秒原子计数 + 定期快照，或按渠道分片加锁；保持现有 API 与窗口语义。

验收：高并发流式下无锁热点；指标测试通过。

### C8. 前端体积优化（M）

现状：主包约 790KB，总产物约 1.47MB。

方案：检查 Ant Design 的按需引入与 dayjs locale、图表库是否进入主包；必要时拆分 Playground 的大体积依赖；开启构建分析。

验收：首屏 JS 明显下降；路由懒加载与图表懒加载保持不变。

---

## 阶段 D：工程卫生与文档同步（P3）

### D1. 清理未使用依赖（S）

- `eventsource-stream`（`Cargo.toml:18`）全仓库无引用，删除。
- `tower`（`Cargo.toml:34`）仅测试使用，移至 `[dev-dependencies]`。
- `tower-http` 的 `compression-gzip` 在 C4 落地后保留；`timeout`、`limit` 特性未使用，按需删除。

验收：`cargo build` / `cargo test` / `cargo clippy -D warnings` 通过。

### D2. 移除过期属性（S）

- `src/config.rs:16` 的 `#[allow(dead_code)]`（`cookie_secure` 已被使用）。
- `src/error.rs:7` 的 `#[allow(dead_code)]`。

验收：clippy 无新增警告。

### D3. 文档同步（M）

- `README.md`：
  - 平台表的 Runner 名改为与 `.github/workflows/release.yml` 一致（`ubuntu-24.04`、`macos-15` 等）；
  - 删除已完成的“计划中的 M7 剩余项”；
  - 补充 `ONEAPI_ALLOW_SELF_UPDATE`、`ONEAPI_UPDATE_API_BASE`、`ONEAPI_TRUST_PROXY`（B3 落地后）。
- `docs/IMPLEMENTATION_PLAN.md`：
  - 状态改为“已实现（v0.1.1）”；
  - 第 8 节接口与实现对齐，或把未实现项移入本计划。
- `docs/PROGRESS.md`：更新“下一步”，移除“推送 v0.1.0 标签”等过期内容。

验收：文档描述与代码行为一致；新会话按文档可正确恢复上下文。

### D4. 版本号同步（S）

`web/package.json:4` 为 `0.1.0`，crate 为 `0.1.1`。发布前统一版本号策略（前端可跟随 crate，或在发布脚本中同步）。

### D5. 引入前端 ESLint（M）

实施计划第 11 节提到 ESLint，目前 CI 只跑 `tsc --noEmit` 与 `vite build`。建议加入 ESLint + React Hooks 规则，并在 CI 中执行。

### D6. 补充环境变量模板（S）

`.env.example` 增加 `ONEAPI_LOG_JSON`（已存在）、`ONEAPI_ALLOW_SELF_UPDATE`、`ONEAPI_UPDATE_API_BASE`、`ONEAPI_TRUST_PROXY`（按落地情况）。

---

## 阶段 E：后续能力路线（v0.3+，需单独立项）

以下项超出 v1 的既定范围，但对自托管网关价值较高，建议逐项评估：

1. **按 Key / 渠道的速率限制与额度**：令牌桶 + 日/月配额，超限返回 429/402；需要明确计费单位与 Redis/内存后端。
2. **Prometheus `/metrics`**：请求数、错误率、延迟直方图、渠道健康、队列深度。
3. ~~**渠道健康与退避策略**：从固定 60s 冷却改为指数退避 + 半开探测。~~ ✅ 已在阶段 E3 落地。
4. **日志导出与请求重放**：CSV/JSON 导出、从日志一键重放（含脱敏）。
5. **备份与恢复**：SQLite 在线备份（`VACUUM INTO` 或 backup API）、导出/导入配置。
6. **Playground 增强**：工具定义编辑、图片输入、保存历史会话。
7. **多协议扩展**：Embeddings、图片生成、Responses 的 background/files（当前明确不做）。
8. **多管理员与审计日志**（当前明确不做）。

---

## 执行约定

1. 每个阶段结束必须通过：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`、`npm run build`（或 `make build`）。
2. 每个阶段结束更新 `docs/PROGRESS.md`（里程碑/验证记录）并同步 `docs/IMPLEMENTATION_PLAN.md` 的决策变化。
3. 新增数据库字段必须通过新的迁移文件，禁止修改已发布迁移。
4. 涉及网关行为的改动必须补集成测试（沿用 `src/gateway/mod.rs` 的 wiremock 模式）。
5. 受 AOCI 管理的内容变化后，在最终稳定状态调用一次 `aoci_maintain` 并按签发批次更新条目。
6. 本计划为本地草稿，未提交、未推送；是否纳入版本控制由维护者决定。

## 变更记录

- 2026-10-27：基于 v0.1.1 全仓审查创建本计划。
