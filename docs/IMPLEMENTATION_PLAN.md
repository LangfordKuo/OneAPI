# OneAPI 实施计划

- 状态：M0–M7 已实现；审查计划阶段 A–D 与 E3 随 v0.1.2 发布，v0.1.3 修复日志筛选，v0.1.4 新增客户端 IP 请求头，v0.1.5 新增日志 tok/s 与渠道多协议，v0.1.6 重写前端并新增网站域名与 1 秒实时速度，v0.1.7 渠道按协议内分页并修复 Token 图表轴，v0.1.8 API Key 支持渠道/模型/Token 限额，v0.1.9 完成审查优化批次 1–5，v0.1.10 完成流式 Responses `store` 一致性、渠道健康缓存覆盖、大响应字节级缓冲与在线更新信任边界，v0.1.11 完成两轮复审修复、归档计划文档并清理工程债；v0.1.12 实现同模型渠道无缝故障转移与冷却期主动恢复；v0.1.13 增加轻量备份/恢复、故障转移预算、运行状态信息与每 Key 每日额度；v0.1.14 修复跨协议推理输出并移除全部测试代码；v0.1.15 实现日志与仪表盘实时刷新、渠道列表快捷启停并修复前端 lint
- 最后更新：2026-10-30
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

前端：Vite 7 + React 18.3 + TypeScript 5 + Tailwind CSS 4 + shadcn/ui（base-nova 风格，基于 Base UI）+ lucide-react + React Router 7 + TanStack Query 5 + Recharts 3（懒加载）+ sonner + next-themes + dayjs。中文界面，内置暗色模式。

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
    ├── package.json  vite.config.ts  tsconfig.json  components.json
    └── src/{main.tsx,pages/,components/{ui,common,layout}/,api/,hooks/,lib/,index.css}
```

## 4. 数据模型

SQLite，时间统一存 unix 毫秒 `INTEGER`。

- `admin(id=1, username, password_hash, created_at)`
- `sessions(token_hash PK, admin_id, created_at, expires_at)`
- `channels(id, name, kind, kinds TEXT JSON, fallback_kind, protocol_configs TEXT JSON, base_url, api_key_enc, auth_header TEXT 'bearer'|'x-api-key', models TEXT JSON, model_mapping TEXT JSON, extra_headers TEXT JSON, proxy_url, priority INT, weight INT, timeout_ms, enabled, last_error, last_error_at, disabled_until, consecutive_failures, created_at, updated_at)`：`protocol_configs` 为每个协议保存一套 `{protocol, mode: native|convert, target, base_url, api_key_enc, auth_header, models, model_mapping, extra_headers}`；`mode=convert` 时使用 `target` 指向的原生协议配置。旧列保留为第一个原生协议的兼容视图，旧数据由启动时的 backfill 自动生成三条路由。
- `api_keys(id, name, key_hash UNIQUE, key_prefix, enabled, expires_at, last_used_at, allowed_channel_ids TEXT JSON, allowed_models TEXT JSON, token_limit INT, token_usage INT, daily_token_limit INT, daily_token_usage INT, daily_usage_day INT, created_at)`：两个 JSON 数组为空表示不限制；`token_limit` 为空/0 表示不限制，`token_usage` 累计该 Key 的 total_tokens，达到上限时自动置 `enabled=0`；`daily_token_limit` 为空/0 表示不限，按 `daily_usage_day`（UTC 日序号）懒重置，达到上限时请求返回 429 但不永久停用 Key。
- `usage_logs(id, request_id, source 'gateway'|'playground', api_key_id, channel_id, model, upstream_model, inbound_protocol, upstream_protocol, stream, status_code, attempts, error_type, error_message, prompt_tokens, completion_tokens, total_tokens, cached_tokens, reasoning_tokens, latency_ms, first_token_ms, created_at)`
- `responses(id 'resp_...' PK, api_key_id, model, request_json, response_json, status, created_at)`
- `request_details(request_id, api_key_id) PK, request_body, response_body, created_at`（可选，默认关闭，截断 64KB）
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
6. 失败切换：只要还有同模型候选，上游失败都继续切换到下一个渠道（任意 4xx/5xx、连接/超时、空响应、非 JSON、响应解析/编码失败、2xx 内 `error`）；单次请求最多尝试 3 个渠道，且从请求开始累计 60 秒后不再切换，同一请求语义错误在第二个渠道重复出现时立即停止；候选耗尽才向客户端返回错误，优先返回第一个请求语义错误（如 400）的状态码，纯渠道故障返回 502。**一旦已向客户端写出字节就停止切换**。流式在提交前先缓冲首个 SSE event，若为 error 事件则继续切换。渠道冷却与切换解耦：仅 401/402/403/404/408/409/425/429/5xx 等渠道侧失败累计 `consecutive_failures`，首次失败只记录错误，第二次起进入冷却，时长按 60s × 2^(n-2) 指数增长并在 30 分钟封顶；400/413/422 等请求语义错误不冷却渠道；成功清零。冷却到期后仅放行一个进程内半开探测请求，探测成功即恢复，失败则继续翻倍冷却。冷却期间后台每 60 秒主动探测一次不健康渠道（每请求 10 秒超时、最多 8 路并发），成功立即清除冷却并失效渠道缓存，失败只更新 `last_error` 并保持或延长到下一次探测，不增加 `consecutive_failures`；无客户端流量时也能自动恢复上线。同模型渠道全部处于冷却时返回 `503 channel_recovering`，仅当确实没有该模型渠道时返回 404。
7. 响应：
   - 同协议：字节透传 + tee 观察者；Chat 流可注入 include_usage 并过滤尾块。
   - 跨协议：解码→IR 事件→编码；非流式先完整解码再编码。
8. 完成后写 usage_log（含 attempts、首 token 延迟）；流被客户端断开时用 Drop guard 通过 mpsc 记 499。
9. Responses 入口按 store 持久化并返回 `resp_...` id。

超时：连接 10s；非流式总超时 600s；流式空闲 120s（逐 chunk timeout）；渠道可覆盖。响应头统一 `x-request-id`，SSE 加 `Cache-Control: no-cache`、`X-Accel-Buffering: no`。

## 7. 认证与安全

- API Key：`sk-` + 48 位 base62；库里只存 SHA-256 哈希 + 前缀用于展示；创建时仅返回一次明文。可选限制：允许的渠道 ID、允许的模型名（空数组表示全部，模型支持 `*`）、累计 Token 上限（空/0 表示不限）；用量在写入 usage_logs 的同一事务累加，达到上限自动停用，网关返回 403 `permission_error`。
- 管理员：Argon2id 密码；登录后签发随机 opaque session，存哈希到 `sessions`，HttpOnly + SameSite=Lax Cookie（HTTPS 时 Secure），7 天过期，登出即删。
- 初始化：首次启动且无管理员时，浏览器访问进入 Web 安装页，创建管理员后自动写入 `<data_dir>/.env` 并登录；也支持设置 `ONEAPI_ADMIN_USER/PASSWORD` 无界面自动初始化。未设置密码时不再生成随机密码，而是等待 Web 安装。提供 `oneapi reset-password` 子命令。
- 上游 Key 用 AES-256-GCM 加密存储（`v1:nonce:ciphertext`），主密钥来自 `ONEAPI_SECRET`，否则自动生成 `data/master.key`（0600）；接口返回一律掩码。
- CSRF：SameSite + 对 `/api` 写请求校验 Origin。比较前把 Origin、Host 与设置项 `site_domain` 归一化为 `host[:port]`（忽略协议、路径与默认端口）；`site_domain` 非空时，即使反向代理没有透传外部 Host，只要 Origin 与该域名一致也放行。日志脱敏 Authorization/x-api-key。
- CORS：`/v1/*` 允许任意 Origin（Bearer 鉴权，不携带 Cookie）；`/api/*` 同源。
- 在线更新：只从 GitHub Release 或 gh-proxy.com 获取官方资产，强制校验 SHA256SUMS；容器环境默认拒绝自更新。

## 8. 后台 API

```
POST /api/auth/login|logout   GET /api/auth/me   PUT /api/auth/password
GET /api/setup/status   POST /api/setup   # 首次运行安装向导（无会话）
GET /api/metrics/stream   # 实时 tok/s SSE（管理员会话，最近 1 秒实测值，附带 usage_total 写入计数）
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

会话状态驱动首屏：`App` 依次查询安装状态与 `/api/auth/me`，未安装渲染安装向导，未登录渲染登录页，登录后进入带侧边栏的后台路由（`/`、`/channels`、`/keys`、`/logs`、`/playground`、`/settings`）。UI 采用 shadcn/ui（base-nova，Base UI 原语）+ Tailwind CSS 4 的 CSS 变量主题；侧边栏在移动端折叠为 Sheet，暗色模式由 next-themes 切换。

- 渠道页：表格（名称/原生协议/地址/模型数/优先级/状态/实时 tok/s）；抽屉表单把三个协议做成内分页，每页独立配置 Base URL、Key、auth_header、模型、model_mapping、自定义头；每页的「转换目标」可选「不转换」或另一个原生协议，选转换后隐藏本页详细配置并复用目标页配置；priority/weight、超时、代理、启用为渠道级共用。表单内“测试连通性”“拉取模型”作用于当前协议页，拉取结果回填当前页。
- Key 页：创建/编辑弹窗一次性展示完整 Key（CodeBlock + 复制）；可配置可用渠道、可用模型与 Token 上限，留空即不限制；列表显示限制摘要、累计用量/上限与「已超限」状态；启用/禁用/删除；显示前缀、最后使用时间。
- 日志页：时间范围/模型/请求 ID/渠道/Key/状态/协议/来源/流式筛选 + 分页 + 详情抽屉（token、耗时、首 token、错误、可选请求响应体）；窄屏隐藏次要列，行点击仍可查看全部字段。
- 仪表盘：统计卡 + Recharts 请求/错误时序与 Token 柱状图 + 模型/渠道占比环形图 + 最近错误 + 系统信息。
- Playground：选协议、模型（datalist 可输入）、系统提示、多轮消息、流式开关；用 fetch + ReadableStream 自写 SSE 解析，展示文本/推理/工具调用与耗时/token/命中渠道。
- 统一 fetch 封装（`credentials: include`；401 时仅登录、`/api/auth/me` 与修改密码接口不跳转），TanStack Query 管理服务端状态，路由级代码分割；图表单独懒加载。
- 开发代理把 `Origin` 重写为后端地址，使 `npm run dev` 下经过 Vite 代理的写请求能通过 CSRF 同源检查。

## 10. 单文件构建与部署

- release 用 `#[derive(RustEmbed)] #[folder = "$CARGO_MANIFEST_DIR/web/dist"]` 嵌入；debug 下 rust-embed 从磁盘读取 `web/dist`，改动无需重新编译。`build.rs` 在目录缺失时生成占位 `index.html`，保证 `cargo check/build` 不因缺前端产物失败（release 正式构建由 Makefile 先跑前端构建）。
- SPA fallback：非 `/api`、`/v1` 的 GET 未命中文件时返回 `index.html`；带 hash 的 assets 长缓存，index no-cache。
- `build.rs` 在 `web/dist` 缺失时生成占位 `index.html`，保证 `cargo check/build` 不因缺前端产物失败（release 正式构建由 Makefile 先跑前端构建）。
- Makefile：`make dev`（后端 + vite）、`make web`、`make build`（web → `cargo build --release`）、`make lint`、`make docker`。
- Dockerfile 三段：node:24 构建前端 → rust:1-slim-bookworm 构建（rustls，无 OpenSSL，SQLite bundled）→ debian:bookworm-slim 运行（CA 证书、非 root、`/data` 卷、EXPOSE 3000）。附 docker-compose。
- GitHub Actions：`ci.yml` 在 `main`/PR 上运行 fmt、clippy 以及前端 ESLint、typecheck 与构建；`release.yml` 在 `v*` 标签上校验版本并构建 Linux/Windows/macOS × amd64/arm64 六个产物，同时用 `Dockerfile.release` 将 Linux 产物组装为 `linux/amd64,linux/arm64` 镜像并推送 GHCR。
- 配置全走 env，并支持 `.env`（当前目录与数据目录，进程环境优先）：`ONEAPI_ADDR=0.0.0.0:3000`、`ONEAPI_DATA_DIR`、`ONEAPI_DATABASE_URL`、`ONEAPI_SECRET`、`ONEAPI_ADMIN_USER/PASSWORD`、`ONEAPI_LOG_LEVEL`、`ONEAPI_LOG_JSON`、`ONEAPI_COOKIE_SECURE`、`ONEAPI_TRUST_PROXY`、`ONEAPI_ALLOW_SELF_UPDATE`、`ONEAPI_UPDATE_API_BASE`。安装向导会生成 `<data_dir>/.env`。

## 11. 测试与验证

- 自动化测试代码与测试依赖（`tempfile`、`tower`、`wiremock`）已按要求移除；仓库不再包含 `#[cfg(test)]` 模块或测试文件。
- 合并前验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、release 构建（Makefile 先构建前端），以及前端 `tsc --noEmit`、ESLint 与 `vite build`。
- 行为验证使用独立于仓库的临时冒烟脚本（mock 上游 + 真实二进制），脚本不随仓库分发。
- CI：fmt、clippy `-D warnings`、web build。

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
- 2026-10-27：完成 v0.1.0/v0.1.1 发布与实时指标、在线更新；随后按原审查计划（已归档，见 §16）落地阶段 A–D（设置接线、安全加固、性能优化、工程卫生与文档同步）。
- 2026-10-27：阶段 E3 落地：渠道冷却改为指数退避（首次失败不冷却，第二次起 60s 起翻倍、30 分钟封顶），冷却到期仅放行单个进程内半开探测，探测占用时返回 `503 channel_recovering`；版本升至 `0.1.2` 并完成全量实测后发布。
- 2026-10-27：v0.1.3 修复日志列表筛选：行查询补 `WHERE 1=1`，避免筛选条件被并入 `LEFT JOIN` 的 ON 子句；新增筛选回归测试。
- 2026-10-28：新增设置项 `client_ip_header`：可指定 `CF-Connecting-IP`、`X-Forwarded-For`、`X-Real-IP` 等请求头解析真实客户端 IP，登录失败限速按解析后的 IP 计数；头缺失或无效时回退 TCP 对端，仅在可信代理后配置。
- 2026-10-28：版本升至 `0.1.4` 并发布上述客户端 IP 请求头功能。
- 2026-10-28：日志列表与详情新增 `tokens_per_second`：输出 tokens 除以生成耗时，流式扣除首 token 等待；前端显示为 `tok/s`。
- 2026-10-28：渠道支持多上游协议：新增 `kinds` 与 `fallback_kind`，入站协议匹配时原样转发，否则转换到配置的 fallback；迁移 `0004_channel_kinds.sql` 回填旧渠道。
- 2026-10-28：渠道改为按协议内分页：新增 `protocol_configs`（迁移 `0005_channel_protocol_configs.sql`），每协议可有独立 Base URL/Key/鉴权头/模型/映射/请求头；转换目标可选「不转换」且只能指向原生协议，转换时复用目标协议配置。启动时自动为旧渠道生成三条路由，保持原有行为。
- 2026-10-28：版本升至 `0.1.5`，发布日志 tok/s 与渠道多协议功能。
- 2026-10-28：新增设置项 `site_domain`：反向代理后填写对外域名，CSRF 校验在 Origin 与请求 Host 不一致时额外接受该域名；两侧都归一化为 `host[:port]`，忽略协议、路径与 80/443 默认端口。设置页新增对应输入框。
- 2026-10-28：实时 tok/s 改为最近 1 秒实测值，移除 10 秒滑动窗口；`/api/metrics/stream` 每次推送上一完整秒的 token 增量，流结束或空闲一秒后归零。
- 2026-10-28：版本升至 `0.1.6`，发布 shadcn/ui 前端重写、网站域名设置与 1 秒实时速度。
- 2026-10-28：渠道按协议内分页，每协议独立 Base URL/Key/模型等并支持转换到原生协议；Token 图表 Y 轴改用万/亿紧凑格式。版本升至 `0.1.7`。
- 2026-10-28：API Key 新增可选限制：允许的渠道、允许的模型与累计 Token 上限；限制在网关选路/鉴权与用量写入时生效，达到上限自动停用。版本升至 `0.1.8`。
- 2026-10-28：全仓审查后的安全与稳定性修复：流式或上游未返回 usage 的请求按本地估算计入 Token 配额；`x-request-id` 增加长度/字符校验，`request_details` 改为按 API Key 隔离并新增 `created_at` 索引（迁移 `0007_request_details_key.sql`）；渠道 HTTP Client 缓存不再随失败状态无限增长；上游响应体限制 64MB 且错误信息截断后落库；保留期清理改为分批事务；客户端 IP 请求头留空时默认对回环/内网对端使用 `X-Forwarded-For`，显式配置后直接信任（适配 CDN 公网回源）；渠道协议配置在运行时缓存中预解析，SSE 解析改为线性并限制单帧大小，请求体按需克隆；Responses 存储被跳过时返回 `store:false`；优雅退出时 flush 用量写队列。
- 2026-10-28：按原优化清单（已归档，见 §16）完成批次 1–5：登录/改密限速、设置事务化、CSRF 转发头取值与短 Key 掩码；上游重定向限制、故障转移总预算、协议化 JSON 解析错误与 `include_usage` 兼容重试；日志分页上限与可选总数、站点/模型缓存、可配置连接池与 `/readyz` 队列指标；加权选路、实时指标分片与 SSE 字节级解析；前端 SSE 冲刷/增量换行、Playground 流取消、Key 限额整数校验与全局安全响应头。
- 2026-10-28：版本升至 `0.1.9`，发布上述审查优化批次 1–5。
- 2026-10-28：完成优化计划收尾项：流式 Responses 终帧在落库后发送并按实际持久化结果回写 `store`；渠道失败改用进程内健康覆盖层，避免故障期间反复全量重载；非流式响应改为字节级读取与同协议原字节透传；在线更新强制 HTTPS、限制跳转并校验资产名。版本升至 `0.1.10`。P1-6（`ONEAPI_SECRET` 密钥派生 v2）按决定不处理。
- 2026-10-28：第二次全仓复审（基线 v0.1.10，详见 §16 与原优化清单 §9）：修复 SSE 未完成帧计费绕过 8MB 上限并支持裸 CR 行尾；Responses 透传仅为生命周期帧改写 `id`/`store`，透传零拷贝；健康渠道成功时跳过 `UPDATE channels`；`count_tokens` 复用渠道 Client/默认值缓存并限制响应体；同协议响应只解析 usage；`store=false` 不再克隆请求体；模型白名单与选路白名单降低重复解析；渠道缓存重载增加 single-flight；内嵌静态资源改用 `Cow`；`1h` 仪表盘时序改为 60 秒桶。版本号未变更。
- 2026-10-28：复审追加项（版本升至 `0.1.11`，归档原计划文档至 §16）：Responses `request_json` 改为只存本轮请求并按链回溯展开（旧行兼容，新增 256 链/2000 项/16MB 上限）；`request_details` 并入用量写队列与批量事务；`Crypto::mask` 增加按密文缓存；流式透传改为轻量观测（不构建 IR 事件），Responses 终帧仍走状态机；帧改写保留 `id:`/`retry:`/注释；渠道缓存元素改为 `Arc<PreparedChannel>` 并由候选持有索引；`.env` 在 tokio 运行时启动前加载；`client_ip_header` 并入 `RuntimeCache`。版本号未变更。
- 2026-10-29：同模型渠道无缝故障转移（v0.1.12）：非流式与流式提交前的所有上游失败（任意 4xx/5xx、连接/超时、空响应、非 JSON、解析/编码失败、2xx 内 `error`）都会切换到下一个同模型候选，候选耗尽才向客户端返回错误；全部失败时优先返回第一个请求语义错误的状态码，纯渠道故障仍返回 502；新增 `penalizes_channel` 将渠道冷却与故障转移解耦，400/413/422 等请求语义错误不冷却渠道；同模型渠道全部冷却时返回 `503 channel_recovering` 而不是 404；`count_tokens` 同样在多个 Anthropic 原生渠道间切换。版本升至 `0.1.12`。
- 2026-10-29：渠道冷却主动恢复（v0.1.12）：新增 `src/recovery.rs` 后台任务，每 60 秒探测所有有失败记录且处于冷却的渠道；每个协议请求最多 10 秒、最多 8 路并发，优先用配置中的具体模型发一次最小补全请求，只有 `*` 模型时改用 `/models`。探测成功立即清零失败状态并失效渠道缓存（无需等待 30 秒 TTL 或客户端流量）；失败只更新 `last_error`，不增加 `consecutive_failures`，正在运行的指数退避不被缩短，冷却已到期则顺延 60 秒。`upstream` 新增可复用的 `build_client_with_timeout`、`probe_model`、`probe_body`，后台任务与渠道探活共用；`ChannelProbes` 增加与客户端半开探测共享的恢复探测闸门。
- 2026-10-29：个人使用与轻量运维增强（v0.1.13）：启动时执行 `PRAGMA quick_check`，有未应用迁移时先 `VACUUM INTO` 快照到 `data/backups`（保留最近 5 份）；设置页新增数据库导出/导入，导入先校验再在重启时换库并自动保留当前库；`/api/system` 增加 DB/WAL 大小、最近备份、写队列、冷却渠道与 24 小时故障转移次数；故障转移增加 3 渠道/60 秒预算与重复请求错误提前停止；`api_keys` 新增按 UTC 自然日的 `daily_token_limit`，超限返回 429 且次日自动恢复（迁移 `0008_api_key_daily_limit.sql`，并为 `usage_logs.created_at` 增加索引）。
- 2026-10-29：跨协议推理输出修复（v0.1.14）：Anthropic 编码器不再丢弃推理，流式输出 `thinking`/`thinking_delta`，非流式在无签名时也输出 thinking block；流式 IR 增加 `ReasoningSignature`，保留 Anthropic 上游的 `signature_delta` 并写入 Responses 的 `encrypted_content`；Anthropic 编码器在新块开始前关闭旧块。修复 Anthropic 客户端使用 Chat 上游时“无推理输出且首字延迟高”的问题，覆盖 Playground 与普通 API。
- 2026-10-29：移除测试代码（v0.1.14）：删除全部 `#[cfg(test)]` 模块、`src/admin/tests.rs` 与 test-only helper；移除 dev-dependencies `tempfile`/`tower`/`wiremock`；CI 与 Makefile 不再执行测试。`#[cfg(test)]` 原本不会进入 release 二进制，本次主要减少源码与开发依赖面。
- 2026-10-30：日志与仪表盘全量实时化：用量写入提交后递增进程内 `usage_total`，`/api/metrics/stream` 在原有每秒快照中附带该计数（旧前端忽略未知字段，协议保持兼容）；仪表盘摘要/时序/模型与渠道占比/最近错误/系统信息以及日志列表在计数变化时按 1.5–3 秒节流刷新，SSE 断开时退化为 5 秒轮询，后台标签页暂停刷新并在恢复可见时补一次。仪表盘「总 Tokens」与日志 Tokens 列改用 K/M/B 紧凑格式（`formatTokens`），Token 图表轴与提示同步；图表关闭重绘动画，避免实时刷新时闪烁。
- 2026-10-30：渠道列表快捷启停与前端 lint 修复（v0.1.15）：新增 `PUT /api/channels/{id}/enabled` 与 `channel::set_enabled`，只更新 `channels.enabled`/`updated_at` 并失效渠道缓存，列表可直接启用/停用渠道而不重传协议配置与密钥；同版本修复 `react-hooks` 报错——`useLiveRefresh` 的 ref 同步移入 effect，`useNow` 拆为 30 秒/1 秒共享时钟并新增 `useNowSeconds`，仪表盘运行时长不再在渲染期调用 `Date.now()`。
- 2026-10-27：完成 M5 流式；新增 `protocol/sse.rs`、`protocol/stream.rs`、`gateway/stream.rs`。流式空闲超时默认 120s，渠道 `timeout_ms` 配置后以其作为逐 chunk 超时；首个 SSE event 若为 error 仍可在提交前切换渠道。
- 2026-10-27：完成 M6 前端与配套后台接口；新增 `/api/logs`、`/api/dashboard/*`、`/api/models`、`/api/playground`，Playground 复用网关执行链并以 `source=playground`、`api_key_id=NULL` 记录。请求体记录开关与保留期清理留待 M7。
- 2026-10-27：M7 CI/CD 落地：新增 GitHub Actions CI 与 Tag 发布，构建 Linux/Windows/macOS × amd64/arm64 六个平台产物；Linux 使用 musl 静态链接以去除 glibc 版本依赖；Actions 使用 Node 24 版本并固定 runner；构建任务上传原始二进制，Release 任务统一打包一次；Docker 使用预编译 Linux 产物组装并推送 GHCR 多架构镜像。
- 2026-10-27：新增 Web 安装向导：无管理员时首次访问进入安装页，创建管理员后写入 `<data_dir>/.env`（0600）并自动登录；支持 `.env` 从当前目录/数据目录加载，进程环境变量优先；设置 `ONEAPI_ADMIN_PASSWORD` 时仍可无界面引导。
- 2026-10-27：M7 收尾：新增每 6 小时保留期清理（0 表示永久保留）、`oneapi reset-password` 子命令、请求/响应正文记录（单条 64KB，日志详情可查看）。
- 2026-10-27：新增实时 tok/s：内存 10 秒滑动窗口，`GET /api/metrics/stream` 每秒推送全站与各渠道输出速率；前端渠道页与主页用 EventSource 订阅，流式请求增量估算并在结束时用 usage 校准。
- 2026-10-27：新增在线更新：可切换 GitHub / gh-proxy.com 更新源，后台检查最新 Release、校验 SHA256、替换二进制并自动重启；容器环境默认拒绝自更新。

## 16. 路线图与已知限制（原计划文档归档）

`docs/REVIEW_PLAN.md`（v0.1.1 审查计划）与 `docs/OPTIMIZATION_PLAN.md`（v0.1.8 审查后的优化清单）中的可执行项均已完成（阶段 A–E3、批次 1–5、P1-8/P2-6/P2-8/P3-8、2026-10-28 两轮复审），两份文件已删除，尚未完成的能力项与接受的风险归档于此。

### 16.1 后续能力（v0.3+，需单独立项）

1. 按 Key / 渠道的速率限制与日/月配额（令牌桶，超限返回 429/402；需确定计费单位与后端）。
2. Prometheus `/metrics`：请求数、错误率、延迟直方图、渠道健康、队列深度。
3. 日志导出（CSV/JSON）与请求重放（含脱敏）。
4. 配置导入导出（数据库快照备份/恢复已在 v0.1.13 完成；渠道/Key 的可迁移 JSON 导出仍未做）。
5. Playground 增强：工具定义编辑、图片输入、历史会话。
6. 多协议扩展：Embeddings、图片生成、Responses 的 background/files（当前明确不做）。
7. 多管理员与 RBAC、审计日志（当前明确不做）。
8. PostgreSQL 支持（当前 SQLite 单写者限制）。

### 16.2 已知限制与接受的风险

- 显式配置 `client_ip_header` 时完全信任来源值；源站必须只允许 CDN/反向代理访问，否则可伪造绕过登录限速。
- 客户端断开时若上游已报告部分 usage，计费以上游值为准；仅缺失或为 0 时使用本地估算。
- SQLite 单写者模型：高并发写入仍受数据库锁限制，写队列只缓解突发。
- 在线更新无签名校验，信任模型为 GitHub 账号 + HTTPS；覆盖 `ONEAPI_UPDATE_API_BASE` 即委托信任。
- `ONEAPI_SECRET` 直接 `SHA-256(secret)` 作为 AES 主密钥（P1-6 决定不处理），应使用高熵值。
- Responses `previous_response_id` 链式展开上限为 256 轮 / 2000 项 / 16MB，超限返回 400。
- 开启请求体记录时，请求详情经写队列异步落库，刚结束的请求可能短暂查不到详情。
- SSE 解析的单帧上限为 8MB（覆盖整个未完成事件）。
- 数据库导出/导入只支持默认的 `data/oneapi.db`；设置 `ONEAPI_DATABASE_URL` 时接口会拒绝。恢复会短暂中断服务并依赖进程重启。
- 每日 Token 上限在请求进入时检查、请求完成后异步累加，并发请求可能短暂超过上限；按 UTC 自然日重置。
