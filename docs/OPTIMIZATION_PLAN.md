# OneAPI 优化计划（v0.1.8 审查后）

- 状态：批次 1–5 与 P1-8、P2-6、P2-8、P3-8 已完成（v0.1.10）；P1-6 决定不处理
- 审查基线：`v0.1.8` + 2026-10-28 已完成的审查修复
- 关联文档：`docs/IMPLEMENTATION_PLAN.md`、`docs/PROGRESS.md`、`docs/REVIEW_PLAN.md`
- 说明：`docs/REVIEW_PLAN.md` 是 v0.1.1 的历史计划（阶段 A–D 与 E3 已完成）。本文件只记录**当前仍未执行**的优化，完成后勾选并把结果回填 `docs/PROGRESS.md`。

## 交接约定

1. 行号会随改动漂移，证据优先按“文件 + 函数/符号”定位，行号仅作参考。
2. 每个批次结束必须通过：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`、`web/` 下 `npm run build`。
3. 新增数据库字段/索引只能追加迁移，禁止修改已发布迁移。
4. 涉及网关行为的改动必须补集成测试（沿用 `src/gateway/mod.rs` 与 `src/admin/tests.rs` 的 wiremock/Router 模式）。
5. 受 AOCI 管理对象变化后，在最终稳定状态调用一次 `aoci_maintain`，按签发批次提交完整 Entry，然后执行 Verify/Check/Guide。
6. 完成一项后：勾选本文件、在 `docs/PROGRESS.md` 记录验证结果、同步 `docs/IMPLEMENTATION_PLAN.md` 中受影响的决策/变更记录。

---

## 0. 本轮已完成（不要重复实现）

| 项 | 结论 | 关键位置 |
|---|---|---|
| 流式/缺失 usage 的 Token 计费 | 用本地估算补齐 `usage_logs` 与 `token_usage` | `src/gateway/usage.rs`、`src/protocol/ir.rs`、`src/gateway/execute.rs`、`src/gateway/stream.rs` |
| `x-request-id` 校验 | 只接受 ≤128 字符的 `[A-Za-z0-9._:-]`，否则生成 UUID | `src/gateway/execute.rs::resolve_request_id` |
| `request_details` 隔离 | 主键 `(request_id, api_key_id)`，0 表示 Playground/历史行 | `migrations/0007_request_details_key.sql`、`src/models/request_detail.rs` |
| Client 缓存泄漏 | 缓存键去掉 `updated_at` | `src/upstream.rs::ClientCache` |
| 上游响应体上限 | 64MB，Content-Length + 分块累计双重校验 | `src/upstream.rs::read_body_limited` |
| 错误信息截断 | 类型 128 / 消息 1024 字符，渠道 `last_error` 同步截断 | `src/gateway/error.rs::truncate_message`、`src/models/channel.rs` |
| 保留期清理 | 每批 5000 行、独立事务；`request_details(created_at)` 索引 | `src/maintenance.rs::delete_before`、迁移 0007 |
| 客户端 IP 请求头 | 留空默认 `X-Forwarded-For`（仅回环/内网对端）；显式配置后无条件信任 | `src/client_ip.rs::resolve_client_ip`、`src/admin/auth.rs::ClientAddr` |
| 渠道缓存预解析 | `RuntimeCache` 返回共享 `Arc<Vec<PreparedChannel>>` | `src/state.rs`、`src/models/channel.rs`、`src/gateway/select.rs` |
| SSE 解析 | 线性扫描、每 chunk 只 drain 一次、单帧缓冲 8MB | `src/protocol/sse.rs`、`src/gateway/stream.rs` |
| 热路径 JSON 解析 | Chat 帧先做廉价子串检查；Anthropic/Responses 不再二次解析 | `src/gateway/stream.rs` |
| 请求体按需克隆 | 仅仍有后续候选时克隆，最后一个候选 move | `src/gateway/execute.rs` |
| Responses `store` | 非流式未落库时返回 `store:false` | `src/gateway/execute.rs`、`src/gateway/responses_state.rs` |
| 用量队列退出 flush | `UsageWriter::shutdown` 关闭 sender 并等待排空 | `src/gateway/usage.rs`、`src/main.rs` |

## 1. 已确认的行为决策（改动前先读）

- **客户端 IP**：设置页留空时默认读取 `X-Forwarded-For`，仅当 TCP 对端是回环/私有/链路本地/CGNAT 地址时生效；显式填写任意请求头后视为管理员已确认可信，直接采用、不再看对端。`ONEAPI_TRUST_PROXY` 现在只影响 CSRF 的 `X-Forwarded-Host`。显式配置时源站端口必须只对 CDN/反向代理开放。
- **Token 计费**：`Usage::fill_missing` 只填缺失字段；`fill_partial_estimate` 会用估算替换提前出现的 0 output；都不覆盖上游已报告的总量。
- **请求详情**：`api_key_id = 0` 表示 Playground 与迁移前历史行；日志详情先按 `(request_id, api_key_id)` 查，未命中再回退到该 request_id 最新一条。
- **渠道 Client 缓存**：`(channel_id, timeout_ms, stream)`；配置变更通过 `ClientCache::clear()` 失效。
- **保留期清理**：0 表示永久保留；每批 5000 行、独立事务。

## 2. 优先级

| 级别 | 含义 |
|---|---|
| P1 | 安全、正确性、可用性；建议优先处理 |
| P2 | 性能与资源效率 |
| P3 | 兼容性、前端与工程卫生 |
| P4 | 后续能力路线图（本期不做） |

估算：S ≈ 半天以内，M ≈ 1–2 天，L ≈ 3 天以上（含测试）。

---

## 3. P1：安全、正确性与可用性

### [x] P1-1 登录/改密限速补齐（S）

- 现状：用户不存在时直接返回，不执行 Argon2；存在但密码错误会执行一次哈希，响应时间可区分。`change_password` 没有失败限速。
- 证据：`src/admin/auth.rs::login`、`src/admin/auth.rs::change_password`、`src/admin/auth.rs::LoginLimiter`。
- 方案：
  1. 增加 `dummy_password_hash()`（`OnceLock<String>`，启动时或首次使用时生成），用户不存在时也执行一次 `verify_password`，抹平时序差。
  2. `change_password` 注入 `ClientAddr`，复用 `LoginLimiter`，key 可用 `password|{ip}|{admin_id}`；连续失败同样返回 429。
- 验收：不存在的用户名与错误密码的响应时间无显著差异；连续错误改密触发 429；现有登录限速测试通过。

### [x] P1-2 设置更新事务化（S）

- 现状：`admin::settings::update` 逐项校验并 `upsert`，后面的字段校验失败时前面的修改已经落库，接口却返回 400。
- 证据：`src/admin/settings.rs::update`、`src/models/setting.rs::upsert`。
- 方案：先把所有字段解析/校验进一个结构体，再在一个事务里批量写入；`setting::upsert` 改为接受 `Executor` 或新增 `upsert_many`。
- 验收：提交 `site_name` 合法 + `default_max_tokens=0` 时返回 400，且 `site_name` 未变化；新增回归测试。

### [x] P1-3 CSRF 的 `X-Forwarded-Host` 取值（S）

- 现状：`trust_proxy` 开启时取 `X-Forwarded-Host` 的第一个值。若代理以“客户端值,真实值”追加，第一个值可被客户端影响。
- 证据：`src/admin/csrf.rs::guard`。
- 方案：取最后一个非空值（与多数代理的追加语义一致），并 trim；保持 `site_domain` 兜底。
- 验收：模拟 `X-Forwarded-Host: evil.example, api.example` 时使用 `api.example`；跨源请求仍被拒绝。

### [x] P1-4 上游 Client 的重定向策略（S/M）

- 现状：reqwest 默认最多跟随 10 次重定向。半可信上游可以把请求重定向到内网地址（SSRF 面），也会让“只允许透传到配置地址”的预期失效。
- 证据：`src/upstream.rs::build_client`、`src/upstream.rs::build_stream_client`。
- 方案：设置 `redirect::Policy::custom`，限制为同 host 且最多 3 次；或直接 `Policy::none()`（需确认主流上游不会用 3xx）。在渠道文档/注释中说明。
- 验收：上游返回指向其他 host 的 302 时不跟随；同 host 的 1 次重定向按策略处理；现有 mock 上游测试通过。

### [x] P1-5 故障转移总时限与首帧绝对超时（M）

- 现状：非流式每个候选各自最长 600s，N 个候选串行等待；流式首帧等待是逐 chunk 超时，慢速上游可用不结束的帧长期占住提交前的连接。
- 证据：`src/gateway/execute.rs::proxy_with_identity` 的候选循环、`src/gateway/stream.rs::run` 的首帧循环、`src/upstream.rs::stream_idle_timeout`。
- 方案：
  1. 请求入口计算 `deadline = Instant::now() + total_budget`（例如非流式 10 分钟、流式提交前 60 秒，可用设置扩展）。
  2. 每次尝试前检查剩余预算；不足时停止并用最后一次错误返回 504/502。
  3. 流式首帧改为绝对 deadline，不再每 chunk 重置。
- 验收：构造多个永久挂起的 mock 上游时，请求在预算内结束；首帧慢速 trickle 不会无限等待；现有故障转移测试通过。

### [ ] P1-6 `ONEAPI_SECRET` 密钥派生（M）

- 现状：直接 `SHA-256(secret)` 作为 AES-256-GCM 主密钥；低熵 secret 可被离线爆破（拿到数据库即可尝试解密校验）。
- 证据：`src/security.rs::Crypto::load_or_create`、`src/security.rs::decrypt`。
- 方案：派生改为 HKDF-SHA256（高熵 secret）或 Argon2id（低熵 secret），引入 `v2` 密文格式；解密时先按 `v2`，失败再按旧 SHA-256 兜底，启动后台/首次读取时逐步重加密。
- 验收：旧数据目录仍可解密；新写入使用 `v2:`；错误 secret 仍解密失败；补单元测试。
- 备注：涉及数据迁移与兼容，排期前先确认是否接受“旧格式长期双读”。
- 状态：2026-10-28 决定不处理（用户确认保持现状；`ONEAPI_SECRET` 应使用高熵值）。

### [x] P1-7 短上游 Key 的掩码（S）

- 现状：`Crypto::mask` 对短于 4 字符的 Key 会返回完整明文（`••••` + 全部字符）。
- 证据：`src/security.rs::Crypto::mask`。
- 方案：字符数 ≤4 时只返回 `••••`；更长时保留末 4 位。
- 验收：`"abc"` → `••••`；`"sk-123456"` → `••••3456`。

### [x] P1-8 流式 Responses 的 `store` 一致性（M，需设计）

- 现状：非流式已在落库被跳过时返回 `store:false`；流式在最终帧发出后才落库，超过 10MB 时客户端已看到 `store:true`，后续 GET 为 404。
- 证据：`src/gateway/stream.rs::run` 的 Responses 落库块、`src/protocol/stream.rs::StreamEncoder::final_response`。
- 方案（候选）：
  1. 在发送 `response.completed` 前先构造/测量最终响应，超限则把该帧的 `store` 改为 `false`（需要把落库判断前移到 Done 事件处理）。
  2. 或对超大响应明确返回流式错误事件，而不是静默跳过。
- 验收：构造 >10MB 的流式响应时，客户端看到的 `store` 与实际可查询性一致；正常大小行为不变。

---

## 4. P2：性能与资源

### [x] P2-1 加权选路 O(n²)（S）

- 现状：每个优先级组内每轮重新求和并 `Vec::remove`，同优先级渠道多时放大。
- 证据：`src/gateway/select.rs::candidates`。
- 方案：一次加权采样/洗牌（如按权重展开或 Efraimidis–Spirakis 采样），避免逐轮 remove。
- 验收：1000 个同优先级渠道的候选生成时间明显下降；分布测试通过（权重比例误差在容差内）。

### [x] P2-2 实时指标锁竞争（S）

- 现状：每个流式 chunk 都抢全局 `Mutex`（`LiveMetrics::add_tokens` → `Inner` → `Window`）。
- 证据：`src/metrics.rs`、`src/gateway/stream.rs::account_stream_tokens`。
- 方案：按秒原子计数 + 定期快照，或按渠道分片加锁；保持“上一完整秒”语义与现有 API。
- 验收：高并发流式压测下无锁热点；现有指标测试通过。

### [x] P2-3 日志分页上限/游标（M）

- 现状：`offset` 只做 `max(0)`，深分页扫描变慢；每页额外执行一次 `COUNT(*)`。
- 证据：`src/admin/logs.rs::list`、`src/models/usage_log.rs::list`。
- 方案：限制最大 offset（或改 `(created_at, id)` 游标分页，保留 offset 兼容）；前端“共 N 条”改为可选查询。
- 验收：百万级日志下翻页延迟可控；筛选语义与前端分页行为不变。

### [x] P2-4 `/api/site` 缓存（S）

- 现状：公开接口每次请求查一次 `settings` 取 `site_name`。
- 证据：`src/app.rs::site_info`。
- 方案：并入 `RuntimeCache`（与 `site_domain` 同 TTL），设置更新后失效。
- 验收：连续请求不再产生数据库查询；修改站点名称后 30 秒内或立即生效。

### [x] P2-5 通配渠道的最近模型缓存（S）

- 现状：存在 `*` 渠道时，`/v1/models` 与 `/api/models` 每次都执行 `recent_models` 聚合查询。
- 证据：`src/gateway/mod.rs::available_models`、`src/admin/catalog.rs::list`、`src/models/usage_log.rs::recent_models`。
- 方案：按 30 秒缓存该聚合结果，或复用 `RuntimeCache`。
- 验收：模型列表请求的数据库读次数下降；新增/变更模型在 TTL 内可见。

### [x] P2-6 渠道失败时的缓存抖动（M）

- 现状：每次失败都 `invalidate_channels()`，下一请求重新 `SELECT` 并预解析全部渠道；故障期间是额外开销。
- 证据：`src/state.rs::mark_channel_failure`、`src/state.rs::RuntimeCache`。
- 方案：运行时只更新单个渠道的健康字段（进程内），或对失效做短合并窗口（如 1 秒）；TTL 仍作为兜底。
- 验收：持续失败场景下不再每请求全量重载；冷却/探测行为不变。

### [x] P2-7 SSE 逐行分配（S）

- 现状：`SseParser::push` 每行 `String::from_utf8_lossy(...).into_owned()`，高吞吐下仍有拷贝。
- 证据：`src/protocol/sse.rs::SseParser::push`。
- 方案：复用行缓冲或直接按字节处理 `event`/`data` 前缀，只有需要 `data` 时才构造 String。
- 验收：基准测试吞吐提升；所有解析测试通过。

### [x] P2-8 大响应体的多次缓冲（S/M）

- 现状：非流式响应经历 `bytes` → `String` → `serde_json::Value`，64MB 上限下峰值约 2–3 倍。
- 证据：`src/upstream.rs::read_body_limited`、`src/gateway/execute.rs` 的成功路径。
- 方案：在保留 64MB 上限的前提下，直接从字节流反序列化，或对同协议透传减少一次 String 构造。
- 验收：大响应内存峰值下降；行为不变。

### [x] P2-9 DB 连接池与队列可观测性（S）

- 现状：连接池固定 5；`readyz` 不暴露用量队列深度；队列满时同步写只打日志。
- 证据：`src/db.rs::connect`、`src/gateway/usage.rs`、`src/app.rs::readyz`。
- 方案：连接池可配置（下限/上限）；`readyz` 或管理接口暴露队列深度/最近失败数。
- 验收：压测下无 `database is locked` 长尾；可观测字段有文档。

---

## 5. P3：兼容性、前端与工程卫生

### [x] P3-1 Axum JSON 解析失败的错误形状（S）

- 现状：`/v1/*` 的 `Json` extractor 拒绝时返回 `text/plain`，不符合三种协议的 JSON 错误形状，部分 SDK 解析失败。
- 证据：`src/gateway/mod.rs` 的 `chat`/`responses`/`messages`/`count_tokens` handler。
- 方案：自定义 `ApiJson<T>` extractor，把 `JsonRejection` 映射为 `error::client_error(protocol_for_path(...))`。
- 验收：非法 JSON 返回 400 + 协议对应的 `error.message/type`；正常请求不受影响。

### [x] P3-2 强制 `stream_options.include_usage` 的兼容性（S）

- 现状：Chat 流式请求统一注入 `stream_options.include_usage=true`，少数 OpenAI 兼容上游不认该字段会直接 400。
- 证据：`src/gateway/execute.rs::proxy_stream` 的请求体改写。
- 方案：首次因该字段失败时去掉字段重试一次（只在 400 且错误信息指向该字段时），或按渠道配置开关。
- 验收：不兼容上游可正常流式返回；兼容上游仍能拿到 usage。

### [x] P3-3 前端 SSE 读取器（S）

- 现状：`readSse` 不冲刷末尾帧（连接在没有结尾空行时关闭会丢最后一帧）；`buffer.replace(/\r\n/g,'\n')` 每 chunk 全量替换，长帧跨多 chunk 时是 O(n²)。
- 证据：`web/src/api/sse.ts`。
- 方案：结束时对剩余 buffer 做一次 `finish` 解析；改为增量处理换行，只对新增片段做归一化。
- 验收：无结尾空行的最后一帧可见；长帧流式场景无卡顿。

### [x] P3-4 Playground 流取消（S）

- 现状：流式请求没有 `AbortController`，离开页面或重新发起请求后旧流仍在消费并 setState。
- 证据：`web/src/pages/Playground.tsx`。
- 方案：为每次请求创建 controller，组件卸载/再次发送时 abort；catch 中忽略 `AbortError`。
- 验收：切换页面或重复发送后不再有旧流更新；无控制台错误。

### [x] P3-5 Keys 的 Token 上限整数校验（S）

- 现状：`Number.isFinite` 允许 `1.5`，后端 i64 反序列化失败并返回非 JSON 的 400。
- 证据：`web/src/pages/Keys.tsx::submit`。
- 方案：改用 `Number.isInteger(parsed)`，并给出明确提示。
- 验收：输入小数时就地提示，不发请求；后端保持不变。

### [x] P3-6 基础安全响应头（S）

- 现状：静态资源与 SPA 响应没有 `X-Content-Type-Options: nosniff`、CSP 等。
- 证据：`src/web.rs::embedded_response`、`src/app.rs::router`。
- 方案：至少加 `nosniff`、`Referrer-Policy`；CSP 需按前端实际资源评估后启用。
- 验收：响应头存在且不影响前端加载；用 Playwright/浏览器验证控制台无 CSP 错误。

### [x] P3-7 `setup.rs` 的 admin_id 硬编码（S）

- 现状：安装完成后 `issue_session(&state, jar, 1)` 硬编码 1；当前有 `CHECK(id=1)` 保护，但属于隐含耦合。
- 证据：`src/admin/setup.rs::setup`。
- 方案：创建后查询 admin 或让 `create` 返回 id。
- 验收：安装流程测试通过；无行为变化。

### [x] P3-8 在线更新的路径与信任边界（S/M）

- 现状：用远端 `asset_name` 拼路径；`ONEAPI_UPDATE_API_BASE` 可覆盖元数据地址；只校验同源 SHA256SUMS，无签名。
- 证据：`src/update.rs::apply_inner`、`src/update.rs::api_url`、`src/update.rs::select_assets`。
- 方案：`asset_name` 追加 `Path::file_name()` 校验；文档明确“信任 GitHub 账号 + HTTPS”的威胁模型；签名校验作为可选增强。
- 验收：异常资产名被拒绝；README 有信任模型说明。

---

## 6. P4：后续能力路线图（本期不做）

1. 按 Key/渠道的速率限制与日/月配额（令牌桶 + 429/402）。
2. Prometheus `/metrics`：请求数、错误率、延迟直方图、渠道健康、队列深度。
3. 日志导出（CSV/JSON）与请求重放（含脱敏）。
4. SQLite 在线备份/恢复（`VACUUM INTO` 或 backup API）与配置导出导入。
5. Playground 增强：工具定义编辑、图片输入、历史会话。
6. 多管理员与 RBAC、审计日志。
7. PostgreSQL 支持（当前 SQLite 单写者限制）。
8. Embeddings / 图片生成 / Responses background/files（当前明确不做）。

---

## 7. 建议的执行批次

| 批次 | 内容 | 理由 |
|---|---|---|
| 批次 1 | P1-1、P1-2、P1-3、P1-7 | 都是 S，改动小、安全收益直接 |
| 批次 2 | P1-4、P1-5、P3-1、P3-2 | 可用性与协议兼容性，需要集成测试 |
| 批次 3 | P2-3、P2-4、P2-5、P2-9 | 数据库/查询压力，独立性强 |
| 批次 4 | P2-1、P2-2、P2-7 | 纯性能优化，需要基准/压测 |
| 批次 5 | P3-3、P3-4、P3-5、P3-6 | 前端与响应头，风险低 |
| 单独评估 | P1-6、P1-8、P2-6、P2-8、P3-8 | 涉及格式迁移或设计取舍，先出方案再排期 |

## 8. 已知限制（接受现状）

- 显式配置客户端 IP 请求头时完全信任来源值；源站必须限制为 CDN/代理访问，否则可被伪造绕过登录限速。
- 流式 Responses 超过 10MB 时客户端可能先看到 `store:true`（见 P1-8）。
- 客户端断开时若上游已报告部分 usage，计费以上游值为准；仅缺失或为 0 时使用估算。
- SQLite 单写者模型：高并发写入仍受数据库锁限制，队列只缓解突发。
- 在线更新无签名校验，信任模型为 GitHub 账号 + HTTPS。
