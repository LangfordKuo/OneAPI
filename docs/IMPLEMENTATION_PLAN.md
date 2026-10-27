# OneAPI 实施计划

- 状态：M0–M7 已实现；审查计划阶段 A–D 与 E3 已落地并随 v0.1.2 发布
- 最后更新：2026-10-27
- 关联文档：`docs/PROGRESS.md`
- 适用仓库：`/root/OneAPI`

## 0. 目标

做一个自托管的 AI API 网关：后台配置上游渠道，客户端用统一 Key 调用；入站支持 Chat Completions / Responses / Anthropic Messages 三种协议，可跨协议转换到任意一种上游协议，支持 SSE 流式；多渠道按优先级/权重路由并在失败时自动切换。最终产物是内嵌 React 静态资源的单个 Rust 二进制（数据目录外部化：SQLite + 密钥文件）。

## 1. 已确认决策

| 决策点 | 选择 |
|---|---|
| 使用定位 | 单管理员 + 多 API Key，不做多用户/注册 |
| 计费 | 只记录用量日志，不做额度/计费拦截 |
| Responses 范围 | 本地会话状态：落库、支持 `previous_response_id`、`GET/DELETE /v1/responses/:id` |
| 协议矩阵 | Chat / Responses / Anthropic 三种入站 × 三种上游，双向全矩阵，含流式 |
| 高级特性 | Tool Calling、推理/思考内容、图片/多模态输入、Prompt Caching（能转则转） |
| 渠道路由 | 多渠道路由 + 优先级/权重 + 故障转移 |
| 上游网络 | 每渠道 HTTP/SOCKS5 代理 + 自定义请求头；不做跳过 TLS 校验 |
| 后台功能 | 用量仪表盘、渠道测试 + 模型拉取、在线 Playground |

明确不做（v1）：多用户/注册/充值、Embeddings/图片生成、Responses 的 background/files/conversations、TLS 校验跳过、OAuth、限流、审计日志。

## 2. 技术选型

后端（单 crate）：Rust 2021、axum 0.8、tokio 1、tower-http 0.6（trace/cors/compression/request-id/limit）、reqwest 0.12（rustls-tls + stream + socks）、sqlx 0.8（sqlite/migrate/chrono/uuid/json，运行时查询，不用编译期宏）、rust-embed 8、serde/serde_json、thiserror 2、tracing + tracing-subscriber、argon2、aes-gcm、sha2、rand、base64、uuid、chrono、eventsource-stream 0.2、axum-extra（cookie）、mime_guess、clap 4。

前端：Vite 7 + React 18.3 + TypeScript 5 + Ant Design 5 + React Router 7 + TanStack Query 5 + @ant-design/plots（懒加载）+ dayjs。中文界面，内置暗色模式。

环境：Node 24.22.1 已随 vite-plus 安装（`/root/.local/share/vite-plus/bin` 下有 node/npm/pnpm/npx），实施时把该目录加入 PATH 即可；Rust 使用 stable 工具链。

## 3. 目录结构

```
OneAPI/
├── Cargo.toml  build.rs  Makefile  Dockerfile  docker-compose.yml
├── rust-toolchain.toml  .env.example  .gitignore
├── migrations/0001_init.sql
├── docs/IMPLEMENTATION_PLAN.md  docs/PROGRESS.md
├── src/
│   ├── main.rs             # clap: serve(默认) / reset-password / version
│   ├── app.rs              # Router 组装、中间件
│   ├── config.rs           # env 配置
│   ├── state.rs  error.rs  telemetry.rs  security.rs
│   ├── db.rs               # pool、migrations、WAL/busy_timeout
│   ├── models/             # admin, channel, api_key, usage_log, response, setting
│   ├── admin/              # auth, channels, keys, settings, csrf, logs, dashboard, playground, catalog
│   ├── gateway/
│   │   ├── mod.rs  auth.rs  select.rs  execute.rs  error.rs
│   │   ├── stream.rs       # 同协议 tee、跨协议流转换、首 event 切换、499
│   │   ├── responses_state.rs
│   │   └── usage.rs
│   ├── protocol/
│   │   ├── ir.rs  error.rs  sse.rs  stream.rs
│   │   └── chat.rs  responses.rs  anthropic.rs
│   ├── upstream.rs         # 每渠道 reqwest client(proxy/headers/timeout)
│   └── web.rs              # release: rust-embed; debug: ServeDir + SPA fallback
├── tests/  + tests/fixtures/{chat,responses,anthropic}/
└── web/                    # Vite + React
    ├── package.json  vite.config.ts  tsconfig.json
    └── src/{main.tsx,layouts/,pages/,components/,api/,hooks/,styles/}
```

## 4. 数据模型

SQLite，时间统一存 unix 毫秒 `INTEGER`。

- `admin(id=1, username, password_hash, created_at)`
- `sessions(token_hash PK, admin_id, created_at, expires_at)`
- `channels(id, name, kind TEXT CHECK in ('openai_chat','openai_responses','anthropic'), base_url, api_key_enc, auth_header TEXT 'bearer'|'x-api-key', models TEXT JSON, model_mapping TEXT JSON, extra_headers TEXT JSON, proxy_url, priority INT, weight INT, timeout_ms, enabled, last_error, last_error_at, disabled_until, consecutive_failures, created_at, updated_at)`
- `api_keys(id, name, key_hash UNIQUE, key_prefix, enabled, expires_at, last_used_at, created_at)`
- `usage_logs(id, request_id, source 'gateway'|'playground', api_key_id, channel_id, model, upstream_model, inbound_protocol, upstream_protocol, stream, status_code, attempts, error_type, error_message, prompt_tokens, completion_tokens, total_tokens, cached_tokens, reasoning_tokens, latency_ms, first_token_ms, created_at)`
- `responses(id 'resp_...' PK, api_key_id, model, request_json, response_json, status, created_at)`
- `request_details(request_id PK, request_body, response_body, created_at)`（可选，默认关闭，截断 64KB）
- `settings(key PK, value)`：站点名、默认 max_tokens、超时、日志/响应保留天数、是否存请求体

索引：`usage_logs(created_at)`、`(model)`、`(channel_id)`、`(api_key_id)`、`responses(api_key_id)`。WAL + `busy_timeout=5s` + `synchronous=NORMAL`；日志写入走单 writer 任务（mpsc + 批量 flush），避免高并发写锁竞争。

## 5. 协议转换设计

核心是统一 IR + 适配器，避免 9 组点对点转换：

```rust
UnifiedRequest { model, system: Vec<Part>, messages: Vec<Message>,
  tools, tool_choice, parallel_tool_calls, max_tokens, temperature, top_p,
  stop, stream, reasoning: Option<ReasoningCfg>, response_format,
  previous_response_id, store, passthrough: Value }
Message { role: System|Developer|User|Assistant|Tool, content: Vec<Part> }
Part    { Text | Image | ToolUse{id,name,input} | ToolResult{id,content,is_error} | Reasoning{text,signature} }
StreamEvent { Start | BlockStart{index,kind} | TextDelta | ReasoningDelta |
              ToolInputDelta | BlockStop | UsageDelta | Stop{reason} | Done | Error | Ping }
```

**同协议快速通道（关键设计）**：入站协议 == 上游渠道协议时，不做 IR 往返。把原始 JSON 解析成 `Value`，只替换 `model`（应用 `model_mapping`）后原样转发；响应字节原样透传，同时用轻量观察者解析 usage/id/输出，供日志与 Responses 存储。这样上游新增字段、beta 特性、Anthropic 的 `cache_control`/`context_management` 等都能零成本兼容。跨协议才走 IR。

**流式**：3 个解码器（SSE → `StreamEvent`）+ 3 个编码器（`StreamEvent` → SSE），共 6 条跨协议流路径；同协议走 tee 透传。编码器需维护块索引与工具调用 id 映射：

- Chat：`data: {chat.completion.chunk, choices[0].delta}`，末尾 `data: [DONE]`；同协议流若客户端未要 usage，可向上游注入 `stream_options.include_usage` 并过滤掉仅含 usage 的尾块。
- Anthropic：`message_start → content_block_start → *_delta → content_block_stop → message_delta → message_stop`，支持 `ping`/`error`。
- Responses：`response.created → output_item.added → content_part.added → output_text.delta / function_call_arguments.delta / reasoning_summary_text.delta → output_item.done → response.completed`。

**字段映射**：

| 语义 | Chat | Responses | Anthropic |
|---|---|---|---|
| system | role system/developer | instructions / developer | system |
| 图片 | image_url | input_image | image block |
| 工具定义 | tools[].function | tools[] 扁平 | tools[].input_schema |
| 工具调用 | assistant.tool_calls | function_call item | tool_use block |
| 工具结果 | role:tool | function_call_output | tool_result |
| 推理参数 | reasoning_effort | reasoning.effort | thinking.budget_tokens |
| 推理内容 | reasoning_content | reasoning item | thinking block |
| 结束原因 | finish_reason | status/incomplete_details | stop_reason |
| 用量 | prompt/completion/total | input/output/total | input/output + cache_* |
| 缓存 | 自动（drop cache_control） | prompt_cache_key | cache_control（仅 Anthropic 上游保留） |

- 推理预算映射：effort→budget 用可配置表（low 2048 / medium 8192 / high 16384），并强制 `budget < max_tokens`；反向按区间推断 effort。
- Anthropic 要求 `max_tokens`：缺失时用渠道/全局默认（默认 4096）；Chat `max_completion_tokens`、Responses `max_output_tokens` 都映射到它。
- `stop` ↔ `stop_sequences`；`tool_choice` 四态（auto/none/required/指定工具）双向映射；Anthropic 的 `any` ↔ required。
- stop_reason：`stop/end_turn/completed`、`length/max_tokens/incomplete(max_output_tokens)`、`tool_calls/tool_use/function_call`、`content_filter/refusal` 互转。
- usage：Anthropic `cache_read_input_tokens` → cached_tokens，`cache_creation_input_tokens` 单独记录；Chat/Responses 的 `*_details` 分别取 cached/reasoning。
- 明确不可表示时返回 400 并说明，而不是静默改语义：如 `n>1` 转 Anthropic、音频/`modalities`、带图片的 tool_result 转 Chat、`logit_bias`/`seed`/惩罚项转 Anthropic、无签名的 thinking 回传 Anthropic。
- 可降级字段：`response_format/json_schema` 转 Anthropic 若上游支持 `output_format` 则映射，否则 400；`cache_control` 转非 Anthropic 上游时丢弃（OpenAI 自动缓存），日志记 warning；跨协议 reasoning 签名/encrypted_content 丢弃。

**Responses 本地状态**：无论上游是什么协议，都把最终输出按 Responses 格式的 output items 存入 `responses`。收到 `previous_response_id` 时从库中取出历史 input+output 展开成完整对话再送上游（不把该 id 转给上游），保证换渠道/跨协议仍然可用；`store:false` 时不落库，后续引用返回明确错误。存储上限 10MB，超限报错；按保留天数清理。

## 6. 网关流水线

1. 生成 `request_id`，限制 body（32MB），解析 JSON，记录入站协议。
2. 鉴权：`Authorization: Bearer sk-...` 或 Anthropic 的 `x-api-key`；Key 用 SHA-256 哈希查库，检查 enabled/过期；`last_used_at` 节流更新。
3. Responses 路由先处理 `previous_response_id` 历史展开。
4. 选渠道：`enabled=1` 且 `disabled_until` 未到；`models` 命中或为 `"*"`；按 priority 降序分组，同组按 weight 加权随机；记录上游模型名。
5. 逐渠道尝试：构造上游请求（同协议只改 model；跨协议编码），用渠道专属 client（代理、extra_headers、超时）发送。
6. 失败切换：连接错误/超时/401/403/404/408/409/425/429/5xx 可重试；400/413/422 等客户端错误不重试；**一旦已向客户端写出字节就停止切换**。流式在提交前先缓冲首个 SSE event，若为 error 事件则继续切换。失败累计 `consecutive_failures`：首次失败只记录错误，第二次起进入冷却，时长按 60s × 2^(n-2) 指数增长并在 30 分钟封顶；成功清零。冷却到期后仅放行一个进程内半开探测请求，探测成功即恢复，失败则继续翻倍冷却。
7. 响应：
   - 同协议：字节透传 + tee 观察者；Chat 流可注入 include_usage 并过滤尾块。
   - 跨协议：解码→IR 事件→编码；非流式先完整解码再编码。
8. 完成后写 usage_log（含 attempts、首 token 延迟）；流被客户端断开时用 Drop guard 通过 mpsc 记 499。
9. Responses 入口按 store 持久化并返回 `resp_...` id。

超时：连接 10s；非流式总超时 600s；流式空闲 120s（逐 chunk timeout）；渠道可覆盖。响应头统一 `x-request-id`，SSE 加 `Cache-Control: no-cache`、`X-Accel-Buffering: no`。

## 7. 认证与安全

- API Key：`sk-` + 48 位 base62；库里只存 SHA-256 哈希 + 前缀用于展示；创建时仅返回一次明文。
- 管理员：Argon2id 密码；登录后签发随机 opaque session，存哈希到 `sessions`，HttpOnly + SameSite=Lax Cookie（HTTPS 时 Secure），7 天过期，登出即删。
- 初始化：首次启动且无管理员时，浏览器访问进入 Web 安装页，创建管理员后自动写入 `<data_dir>/.env` 并登录；也支持设置 `ONEAPI_ADMIN_USER/PASSWORD` 无界面自动初始化。未设置密码时不再生成随机密码，而是等待 Web 安装。提供 `oneapi reset-password` 子命令。
- 上游 Key 用 AES-256-GCM 加密存储（`v1:nonce:ciphertext`），主密钥来自 `ONEAPI_SECRET`，否则自动生成 `data/master.key`（0600）；接口返回一律掩码。
- CSRF：SameSite + 对 `/api` 写请求校验 Origin；日志脱敏 Authorization/x-api-key。
- CORS：`/v1/*` 允许任意 Origin（Bearer 鉴权，不携带 Cookie）；`/api/*` 同源。
- 在线更新：只从 GitHub Release 或 gh-proxy.com 获取官方资产，强制校验 SHA256SUMS；容器环境默认拒绝自更新。

## 8. 后台 API

```
POST /api/auth/login|logout   GET /api/auth/me   PUT /api/auth/password
GET /api/setup/status   POST /api/setup   # 首次运行安装向导（无会话）
GET /api/metrics/stream   # 实时 tok/s SSE（管理员会话）
GET /api/update/check   POST /api/update/apply   # 在线更新
GET|POST /api/channels        GET|PUT|DELETE /api/channels/:id
POST /api/channels/test       POST /api/channels/fetch-models   # 支持未保存配置
GET|POST /api/keys            PUT|DELETE /api/keys/:id
GET /api/logs                 GET /api/logs/:request_id
GET /api/dashboard/summary|timeseries|breakdown?range=24h|7d|30d
GET /api/models   GET|PUT /api/settings   POST /api/playground   GET /api/system
GET /api/site   # 公开站点名称与版本（登录页/安装页使用）
GET /healthz  /readyz
```

网关：`POST /v1/chat/completions`、`POST /v1/responses`、`GET|DELETE /v1/responses/:id`、`POST /v1/messages`、`POST /v1/messages/count_tokens`（上游 Anthropic 时转发，否则按字符数粗略估算并标注）、`GET /v1/models`、`GET /v1/models/:id`。

渠道测试：用草稿配置发最小请求（Chat/Responses/Anthropic 各自最小体，`max_tokens=1`），返回状态码、延迟、错误或回复片段。模型拉取：`GET {base}/models`，兼容 OpenAI 与 Anthropic 的 `{data:[{id}]}` 结构，失败给出可读诊断。

## 9. 前端

路由：`/login`、`/`（仪表盘）、`/channels`、`/keys`、`/logs`、`/playground`、`/settings`；AntD Layout 侧边栏 + 暗色切换。

- 渠道页：表格（名称/类型/地址/模型数/状态/最近错误）；抽屉表单含类型、base_url、Key、auth_header、模型标签、model_mapping 键值编辑器、priority/weight、超时、代理、自定义头、启用开关；表单内“测试连通性”“拉取模型”。
- Key 页：创建弹窗一次性展示完整 Key；启用/禁用/删除；显示前缀、最后使用时间。
- 日志页：时间范围/模型/渠道/Key/状态筛选 + 分页 + 详情抽屉（token、耗时、首 token、错误、可选请求响应体）。
- 仪表盘：统计卡 + 请求/token 时序 + 模型/渠道占比 + 最近错误。
- Playground：选协议、模型、系统提示、多轮消息、流式开关；用 fetch + ReadableStream 自写约 50 行 SSE 解析，展示文本/推理/工具调用与耗时/token/命中渠道。
- 统一 fetch 封装（`credentials: include`，401 跳登录），TanStack Query 管理服务端状态，路由级代码分割。

## 10. 单文件构建与部署

- release 用 `#[derive(RustEmbed)] #[folder = "$CARGO_MANIFEST_DIR/web/dist"]` 嵌入；debug 下 rust-embed 从磁盘读取 `web/dist`，改动无需重新编译。`build.rs` 在目录缺失时生成占位 `index.html`，保证 `cargo check/build` 不因缺前端产物失败（release 正式构建由 Makefile 先跑前端构建）。
- SPA fallback：非 `/api`、`/v1` 的 GET 未命中文件时返回 `index.html`；带 hash 的 assets 长缓存，index no-cache。
- `build.rs` 在 `web/dist` 缺失时生成占位 `index.html`，保证 `cargo check/build` 不因缺前端产物失败（release 正式构建由 Makefile 先跑前端构建）。
- Makefile：`make dev`（后端 + vite）、`make web`、`make build`（web → `cargo build --release`）、`make test`、`make lint`、`make docker`。
- Dockerfile 三段：node:24 构建前端 → rust:1-slim-bookworm 构建（rustls，无 OpenSSL，SQLite bundled）→ debian:bookworm-slim 运行（CA 证书、非 root、`/data` 卷、EXPOSE 3000）。附 docker-compose。
- GitHub Actions：`ci.yml` 在 `main`/PR 上运行 fmt、clippy、test 以及前端 ESLint、typecheck 与构建；`release.yml` 在 `v*` 标签上校验版本并构建 Linux/Windows/macOS × amd64/arm64 六个产物，同时用 `Dockerfile.release` 将 Linux 产物组装为 `linux/amd64,linux/arm64` 镜像并推送 GHCR。
- 配置全走 env，并支持 `.env`（当前目录与数据目录，进程环境优先）：`ONEAPI_ADDR=0.0.0.0:3000`、`ONEAPI_DATA_DIR`、`ONEAPI_DATABASE_URL`、`ONEAPI_SECRET`、`ONEAPI_ADMIN_USER/PASSWORD`、`ONEAPI_LOG_LEVEL`、`ONEAPI_LOG_JSON`、`ONEAPI_COOKIE_SECURE`、`ONEAPI_TRUST_PROXY`、`ONEAPI_ALLOW_SELF_UPDATE`、`ONEAPI_UPDATE_API_BASE`。安装向导会生成 `<data_dir>/.env`。

## 11. 测试

- 适配器单测：每种协议的准备/解析 fixture，覆盖 6 条跨协议组合的请求、非流式响应、SSE 流；固定样例覆盖 tool call 并行、图片、thinking、usage、错误。
- SSE 边界：chunk 从任意字节切断、CRLF、多行 data、注释、`[DONE]`、event 类型。
- 集成测试：wiremock 模拟三类上游 + `tower::ServiceExt::oneshot` 调 axum；覆盖鉴权、路由、故障转移（首个渠道 500/超时/连接失败）、流中断、Responses 存取与 `previous_response_id`、Key 禁用/过期。
- DB 测试用临时文件库跑 migration。
- 前端：`tsc --noEmit`、ESLint、`vite build`；不引入重型 E2E。
- CI：fmt、clippy `-D warnings`、test、web build。

## 12. 实施阶段

- **M0 脚手架**：Cargo、Vite、Makefile、Dockerfile、migrations、config、`/healthz`、静态资源嵌入与 SPA fallback；把 `/root/.local/share/vite-plus/bin` 加入构建脚本 PATH。
- **M1 管理基础**：DB 层、管理员初始化/登录/会话、渠道与 Key CRUD、设置；后台可登录配置。
- **M2 协议层（非流式）**：IR、三协议请求/响应编解码、同协议 fast path、6 条跨协议组合单测。
- **M3 网关（非流式）**：路由/加权/故障转移、上游 client（代理/请求头/超时）、用量日志、错误映射、集成测试。
- **M4 Responses 状态**：落库、`previous_response_id` 展开、GET/DELETE、Key 归属校验。
- **M5 流式**：SSE 解码/编码状态机、tee 透传、首 event 失败切换、断连记 499、完整流测试。
- **M6 前端**：登录、仪表盘、渠道、Key、日志、设置、Playground。
- **M7 收尾**：Docker、README/部署文档、CI、保留期清理任务、`reset-password`、clippy/fmt 全绿。

验收：单二进制启动后能直接打开后台；三类入站协议各自跨三类上游都能非流式+流式成功（含 tools/图片/thinking）；杀掉首个渠道后请求自动切换到下一个并记录 attempts；Responses 多轮续接成功；所有单元/集成测试通过。

## 13. 主要风险与对策

1. 跨协议字段保真：用同协议透传兜底 + 不可表示显式 400 + fixture 矩阵测试。
2. 流式转换状态机复杂：以 IR 事件为中心，索引/工具 id 映射集中在一处，按协议写 golden test。
3. Responses 语义：始终本地展开历史，不依赖上游 id；严格按 Key 隔离。
4. SQLite 并发写：WAL + 日志单写队列 + 批量 flush。
5. 第三方站点差异：base_url 规范（去尾斜杠后追加固定路径）、每渠道代理/自定义头、模型通配 `*`、测试按钮给出原始错误。
6. 单文件构建缺前端产物：`build.rs` 占位 + Makefile/CI 保证 release 前先构建前端。

## 14. 默认假设

中文 UI（可后续加 i18n）、默认端口 3000、单管理员、GPLv3 保留、Rust 2021、前端 React 18.3、日志/响应保留期默认 30 天/7 天、请求体上限 32MB。现有 README 会重写为完整部署与协议兼容说明（含不可转换字段表）。

## 15. 变更记录

- 2026-10-27：创建本文档，记录已确认决策、架构、数据模型、协议转换策略、里程碑与风险。
- 2026-10-27：完成 v0.1.0/v0.1.1 发布与实时指标、在线更新；随后按 `docs/REVIEW_PLAN.md` 落地阶段 A–D（设置接线、安全加固、性能优化、工程卫生与文档同步）。
- 2026-10-27：阶段 E3 落地：渠道冷却改为指数退避（首次失败不冷却，第二次起 60s 起翻倍、30 分钟封顶），冷却到期仅放行单个进程内半开探测，探测占用时返回 `503 channel_recovering`；版本升至 `0.1.2` 并完成全量实测后发布。
- 2026-10-27：完成 M5 流式；新增 `protocol/sse.rs`、`protocol/stream.rs`、`gateway/stream.rs`。流式空闲超时默认 120s，渠道 `timeout_ms` 配置后以其作为逐 chunk 超时；首个 SSE event 若为 error 仍可在提交前切换渠道。
- 2026-10-27：完成 M6 前端与配套后台接口；新增 `/api/logs`、`/api/dashboard/*`、`/api/models`、`/api/playground`，Playground 复用网关执行链并以 `source=playground`、`api_key_id=NULL` 记录。请求体记录开关与保留期清理留待 M7。
- 2026-10-27：M7 CI/CD 落地：新增 GitHub Actions CI 与 Tag 发布，构建 Linux/Windows/macOS × amd64/arm64 六个平台产物；Linux 使用 musl 静态链接以去除 glibc 版本依赖；Actions 使用 Node 24 版本并固定 runner；构建任务上传原始二进制，Release 任务统一打包一次；Docker 使用预编译 Linux 产物组装并推送 GHCR 多架构镜像。
- 2026-10-27：新增 Web 安装向导：无管理员时首次访问进入安装页，创建管理员后写入 `<data_dir>/.env`（0600）并自动登录；支持 `.env` 从当前目录/数据目录加载，进程环境变量优先；设置 `ONEAPI_ADMIN_PASSWORD` 时仍可无界面引导。
- 2026-10-27：M7 收尾：新增每 6 小时保留期清理（0 表示永久保留）、`oneapi reset-password` 子命令、请求/响应正文记录（单条 64KB，日志详情可查看）。
- 2026-10-27：新增实时 tok/s：内存 10 秒滑动窗口，`GET /api/metrics/stream` 每秒推送全站与各渠道输出速率；前端渠道页与主页用 EventSource 订阅，流式请求增量估算并在结束时用 usage 校准。
- 2026-10-27：新增在线更新：可切换 GitHub / gh-proxy.com 更新源，后台检查最新 Release、校验 SHA256、替换二进制并自动重启；容器环境默认拒绝自更新。
