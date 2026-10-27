# OneAPI 进度

- 最后更新：2026-10-27
- 当前阶段：M4 Responses 状态已完成，准备进入 M5
- 当前分支：`main`
- 最近提交：`8f3b2c8 Initial commit`
- 计划文档：`docs/IMPLEMENTATION_PLAN.md`

## 里程碑

- [x] **M0 脚手架**：Cargo / Vite / Makefile / Dockerfile / migrations / config / `/healthz` / 静态资源嵌入与 SPA fallback。
- [x] **M1 管理基础**：DB 层、管理员初始化/登录/会话、渠道与 Key CRUD、设置。
- [x] **M2 协议层（非流式）**：统一 IR、三协议请求/响应编解码、同协议 fast path、6 条跨协议组合单测。
- [x] **M3 网关（非流式）**：路由/加权/故障转移、上游 client（代理/请求头/超时）、用量日志、错误映射。
- [x] **M4 Responses 状态**：落库、`previous_response_id` 展开、GET/DELETE、Key 归属校验。
- [ ] **M5 流式**：SSE 解码/编码状态机、tee 透传、首 event 失败切换、断连记 499。
- [ ] **M6 前端**：登录、仪表盘、渠道、Key、日志、设置、Playground。
- [ ] **M7 收尾**：Docker、README/部署文档、CI、保留期清理任务、`reset-password`、clippy/fmt 全绿。

## 当前状态

- M0 已完成：Rust 后端、Vite + React + Ant Design 前端、SQLite 迁移、静态资源嵌入、健康检查、Makefile/Dockerfile 均已落地。
- 环境：Rust 1.99.1；Node 24.22.1 / npm 11.19.1 位于 `/root/.local/share/vite-plus/bin`，Makefile 已自动把它加入 PATH。
- 前端依赖：Vite 7.3.7、React 18.3.1、Ant Design 5.29.3、React Router 7.18.4、TanStack Query 5.105.0。
- M1 已完成：管理员引导登录、HttpOnly Cookie 会话、上游 Key AES-256-GCM 加密、渠道 CRUD、API Key CRUD、设置接口，以及登录/渠道/Key/设置前端页面。
- M2 已完成：统一 IR、三协议请求/响应编解码、同协议往返与跨协议转换。
- M3 已完成：网关 `/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1/models`；API Key 鉴权、渠道优先级/权重选择、失败重试与冷却、上游代理/自定义头/超时、用量日志、同协议透传与跨协议转换；后台渠道测试与模型拉取。
- M4 已完成：Responses 本地落库、`previous_response_id` 历史展开、GET/DELETE、Key 归属校验；`store=false` 不落库且后续引用返回 400。
- 流式尚未实现，M5 处理。
- 已确认的产品决策见 `docs/IMPLEMENTATION_PLAN.md` 第 1 节。

## M0 验证记录

- `cargo check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过，无警告。
- `npm run build`：通过；产物在 `web/dist`（JS 约 734 KB，M6 再做代码分割）。
- `cargo build --release`：通过，产物 `target/release/oneapi` 约 5.8 MB。
- 从 `/tmp` 启动 release 二进制（不依赖仓库内 `web/dist`），验证：
  - `/healthz`、`/readyz`、`/api/system` 返回 200。
  - `/`、`/playground` 返回内嵌 SPA。
  - `/assets/*.js` 返回 200，`Cache-Control: public, max-age=31536000, immutable`。
  - 未知 `/api/*` 返回 JSON 404。
- SQLite 迁移已应用：`admin`、`sessions`、`channels`、`api_keys`、`usage_logs`、`responses`、`request_details`、`settings` 等表存在，默认 settings 已写入。
- AOCI 首次索引：36/36 条 Entry，`verify`、`check`、最终 Guide 全部通过。

## M1 验证记录

- `cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test`：3 个安全层单测通过（密码哈希、API Key 生成/哈希、AES-GCM 往返）。
- 后端 API 实测：登录/登出/me、未授权 401、渠道创建/更新/列表/删除、Key 创建（返回一次明文）/禁用/删除、设置读取/更新、Cookie 会话。
- 前端 `npm run build`：通过；登录页、渠道页、Key 页、设置页已接入真实 API。
- 端到端：在 `/tmp` 启动二进制，`/` 返回新前端；登录后完成渠道与 Key CRUD；登出后 `/api/auth/me` 返回 401。
- 当前前端产物约 1.27 MB（gzip 约 400 KB），M6 再做路由级代码分割。
- AOCI 增量维护覆盖 M1 新增/修改文件，索引总计 55 条 Entry。

## M2 验证记录

- `cargo fmt --all`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test`：23 个测试通过（20 个协议转换 + 3 个安全层）。
- 覆盖 6 条跨协议请求组合、3 条同协议往返、6 条响应转换，以及 `n>1`、stop→Responses、top_k→Chat、JSON object→Anthropic 等不可表示字段的显式拒绝。
- 协议层为纯函数，尚未接入 HTTP 网关；M3 再接路由、上游客户端与日志。
- AOCI 增量维护后索引总计 61 条 Entry。

## M3 验证记录

- `cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test`：33 个测试通过（20 个协议转换 + 3 个安全层 + 10 个网关集成/辅助）。
- 网关集成测试覆盖：同协议透传与未知字段保留、Chat→Anthropic 跨协议转换、500 故障转移、400 不重试、Anthropic 401 错误形状、模型不存在 404、流式请求显式拒绝、`/v1/models` 聚合。
- 进程级端到端：release 二进制 + 本地 mock 上游，登录→建渠道→建 Key→`/v1/chat/completions` 返回上游内容并保留 `custom_echo`→`/v1/models`→渠道测试→模型拉取→用量日志 attempts=1、tokens 7/3/10。
- `cargo build --release`：通过，二进制约 9.8 MB。
- 流式仍在 M5 实现；Responses 的 `previous_response_id` 仍在 M4 实现。
- AOCI 增量维护后索引总计 68 条 Entry。

## M4 验证记录

- `cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test`：35 个测试通过（新增 Responses 历史展开/Key 隔离与 `store=false` 两个集成测试）。
- 进程级端到端：release 二进制 + mock 上游，Responses 首次请求生成 `resp_...` 并落库；同 Key GET 200、另一 Key GET 404；`previous_response_id` 续接成功；DELETE 后 GET 404，数据库行数从 2 降到 1。
- `cargo build --release`：通过，二进制约 9.9 MB。
- AOCI 增量维护后索引总计 70 条 Entry。

## AOCI 状态

- 版本：`0.1.0-rc22`，二进制 `/usr/local/bin/aoci`。
- ZCode workspace MCP 已配置：`.zcode/config.json`（该文件为机器相关路径，已在 `.git/info/exclude` 中排除）。
- 已完成 `init --locale zh-CN` 与 `scan`。
- 首次完整索引（M0）已完成；M1–M4 完成后做增量维护，当前索引覆盖 70 个受管理文件。
- 每个里程碑结束调用 `aoci_maintain` 做增量维护，完成后执行 verify/check/guide。

## 下一步

1. 进入 M5：SSE 解码/编码状态机、tee 透传、首 event 失败切换、断连记 499、完整流测试。
2. M5 完成后更新本文件，标记 M5 为已完成，并记录验证结果。
3. 每个里程碑结束按 AOCI Guide 做增量维护。

## 交接约定

- 新会话开始先读 `AGENTS.md`、本文档和 `docs/IMPLEMENTATION_PLAN.md`。
- 每个里程碑结束更新本文档；计划或决策变化同步更新 `docs/IMPLEMENTATION_PLAN.md`。
- 不提交 AOCI 的机器运行时资产；`aoci.txt`、`aoci.meta.txt`、`aoci.code.txt`、`.aoci/config.json`、`.aoci/baseline.json`、`AGENTS.md`、`.gitattributes` 属于应纳入版本管理的认知资产。
