# OneAPI 进度

- 最后更新：2026-10-27
- 当前阶段：M7 进行中（CI/CD 与多平台发布已完成）
- 当前分支：`main`
- 最近提交：`390cf24 feat: 完成管理后台日志、仪表盘与 Playground`
- 计划文档：`docs/IMPLEMENTATION_PLAN.md`

## 里程碑

- [x] **M0 脚手架**：Cargo / Vite / Makefile / Dockerfile / migrations / config / `/healthz` / 静态资源嵌入与 SPA fallback。
- [x] **M1 管理基础**：DB 层、管理员初始化/登录/会话、渠道与 Key CRUD、设置。
- [x] **M2 协议层（非流式）**：统一 IR、三协议请求/响应编解码、同协议 fast path、6 条跨协议组合单测。
- [x] **M3 网关（非流式）**：路由/加权/故障转移、上游 client（代理/请求头/超时）、用量日志、错误映射。
- [x] **M4 Responses 状态**：落库、`previous_response_id` 展开、GET/DELETE、Key 归属校验。
- [x] **M5 流式**：SSE 解码/编码状态机、tee 透传、首 event 失败切换、断连记 499。
- [x] **M6 前端**：登录、仪表盘、渠道、Key、日志、设置、Playground。
- [ ] **M7 收尾**：Docker、README/部署文档、CI、保留期清理任务、`reset-password`、clippy/fmt 全绿。（CI/CD 与多平台发布已完成，剩余清理、命令与最终验收）

## 当前状态

- M0 已完成：Rust 后端、Vite + React + Ant Design 前端、SQLite 迁移、静态资源嵌入、健康检查、Makefile/Dockerfile 均已落地。
- 环境：Rust 1.99.1；Node 24.22.1 / npm 11.19.1 位于 `/root/.local/share/vite-plus/bin`，Makefile 已自动把它加入 PATH。
- 前端依赖：Vite 7.3.7、React 18.3.1、Ant Design 5.29.3、React Router 7.18.4、TanStack Query 5.105.0、@ant-design/plots 2.6.8。
- M1 已完成：管理员引导登录、HttpOnly Cookie 会话、上游 Key AES-256-GCM 加密、渠道 CRUD、API Key CRUD、设置接口，以及登录/渠道/Key/设置前端页面。
- M2 已完成：统一 IR、三协议请求/响应编解码、同协议往返与跨协议转换。
- M3 已完成：网关 `/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1/models`；API Key 鉴权、渠道优先级/权重选择、失败重试与冷却、上游代理/自定义头/超时、用量日志、同协议透传与跨协议转换；后台渠道测试与模型拉取。
- M4 已完成：Responses 本地落库、`previous_response_id` 历史展开、GET/DELETE、Key 归属校验；`store=false` 不落库且后续引用返回 400。
- M5 已完成：三协议 SSE 解码/编码、同协议 tee 透传、跨协议 6 条流路径、首 event 失败切换、逐 chunk 空闲超时、断连 499、Responses 流式落库与回读。
- M6 已完成：请求日志页与详情抽屉、仪表盘统计/时序/占比/最近错误、Playground 三协议（含流式）、`/api/logs`、`/api/dashboard/*`、`/api/models`、`/api/playground`、路由级懒加载与暗色模式。
- M7 进行中：已完成 GitHub Actions CI、Tag 触发的六平台 Release（Linux/Windows/macOS × amd64/arm64）与 GHCR 多架构 Docker 镜像；剩余保留期清理、`reset-password`、请求体记录开关与最终验收。
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

## M5 验证记录

- `cargo fmt --all`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test`：56 个测试通过（新增 21 个流式相关用例）。
- 流式覆盖：SSE 增量解析（跨 chunk/CRLF/多行 data/未闭合帧）、三协议解码器与编码器、Chat→Anthropic、Chat→Responses、Anthropic→Chat、Anthropic→Responses、Responses→Chat、Responses→Anthropic、工具调用分片与 id 映射、同协议透传与 usage 尾块过滤、首 event error 失败切换、客户端断开记录 499、Responses 流式落库与 GET 回读。
- `cargo build --release`：通过，二进制约 11 MB。
- 进程级端到端：release 二进制 + 本地 mock SSE 上游；Chat 流式透传返回上游分片与 `[DONE]`；Responses 流式请求经跨协议转换生成本地 `resp_...` id、落库后可 GET 回读；两条 usage_logs 均为 `stream=1`、`status_code=200`、tokens 5/2、`first_token_ms` 非空，`responses` 表存在对应记录。
- 流式空闲超时默认 120s，渠道配置了 `timeout_ms` 时以该值作为逐 chunk 空闲超时；非流式仍使用渠道总超时（默认 600s）。

## M6 验证记录

- `cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test`：58 个测试通过（新增 2 个后台集成测试，覆盖日志/仪表盘/模型列表与 Playground 非流式/流式）。
- 前端 `npm run build`：通过；路由按页面拆分为独立 chunk，图表组件 `DashboardCharts` 单独懒加载（约 1.47 MB / gzip 434 KB），主包约 790 KB（gzip 253 KB）。
- 新增后台接口：`GET /api/logs`、`GET /api/logs/{request_id}`、`GET /api/dashboard/summary|timeseries|breakdown`、`GET /api/models`、`POST /api/playground`。
- Playground 复用网关选路/故障转移/协议转换/日志链路，身份为 `source=playground` 且 `api_key_id=NULL`；Responses 在 Playground 中强制 `store=false` 并拒绝 `previous_response_id`。
- `cargo build --release`：通过，二进制约 12 MB。
- 进程级端到端：release 二进制 + mock 上游；网关非流式/流式各 1 次、Playground Chat 非流式 1 次、Responses 流式 1 次、Anthropic 流式 1 次；`/api/logs` 返回 5 条、Playground 3 条且 `api_key_id=NULL`；summary 统计 requests=5/tokens=30；timeseries 25 个 bucket 合计 5；breakdown 按模型返回 gpt-test；`/api/models` 返回 gpt-test；SPA `/` 与 `/logs` 均返回 200。
- 新增暗色模式切换（localStorage 记忆）；`api` 客户端对非 auth 请求的 401 会回到登录态。
- 请求/响应正文记录仍未接入网关（日志详情会显示“未记录”）；保留期清理任务仍在 M7。

## M7 CI/CD 验证记录

- 新增 `.github/workflows/ci.yml`：push 到 `main` / PR 触发 Rust fmt、Clippy、测试与前端 `npm ci && npm run build`。
- 新增 `.github/workflows/release.yml`：推送 `v*` 标签触发，先校验标签与 `Cargo.toml` 版本一致，再构建六个平台：
  - Linux amd64 `x86_64-unknown-linux-gnu`（`ubuntu-latest`）
  - Linux arm64 `aarch64-unknown-linux-gnu`（`ubuntu-24.04-arm`）
  - Windows amd64 `x86_64-pc-windows-msvc`（`windows-latest`）
  - Windows arm64 `aarch64-pc-windows-msvc`（`windows-11-arm`）
  - macOS amd64 `x86_64-apple-darwin`（`macos-15-intel`）
  - macOS arm64 `aarch64-apple-darwin`（`macos-15`）
- 发布产物命名 `oneapi-<tag>-<platform>.tar.gz|.zip`，包含二进制、`README.md`、`LICENSE`、`.env.example`；Release 同时上传 `SHA256SUMS`。
- Docker 任务使用 Buildx + QEMU 构建 `linux/amd64,linux/arm64` 多架构镜像并推送 `ghcr.io/langfordkuo/oneapi`，标签包含 semver、`v<tag>`、`sha-<commit>` 与 `latest`。
- Dockerfile 升级为 `node:24-alpine` + `rust:1-slim-bookworm` + `debian:bookworm-slim`，SQLite 使用 bundled 构建，运行镜像无需额外 SQLite 库；增加 cargo registry/npm 缓存与 OCI label。
- 本地校验：两个 workflow 通过 PyYAML 解析；runner 标签、Windows ARM64 Tier 1 host 工具链、`windows-11-arm` 的 VS ARM64 工具与 Rust 预装均已核对。
- 尚未推送 `v*` 标签，因此 GitHub Actions 实跑与 GHCR 首次推送待首次正式发版时验证。

## AOCI 状态

- 版本：`0.1.0-rc22`，二进制 `/usr/local/bin/aoci`。
- ZCode workspace MCP 已配置：`.zcode/config.json`（该文件为机器相关路径，已在 `.git/info/exclude` 中排除）。
- 已完成 `init --locale zh-CN` 与 `scan`。
- 首次完整索引（M0）已完成；M1–M7 持续做增量维护，当前索引覆盖后端、前端与文档受管理文件。
- 每个里程碑结束调用 `aoci_maintain` 做增量维护，完成后执行 verify/check/guide。

## 下一步

1. 完成 M7 剩余项：日志与 Responses 保留期清理、`reset-password` 子命令、请求体记录开关落地、最终部署与验收文档。
2. 首次正式发版时推送 `v0.1.0`（需与 `Cargo.toml` 一致），观察 GitHub Actions 六平台构建与 GHCR 推送。
3. M7 完成后更新本文件并做 AOCI 增量维护。

## 交接约定

- 新会话开始先读 `AGENTS.md`、本文档和 `docs/IMPLEMENTATION_PLAN.md`。
- 每个里程碑结束更新本文档；计划或决策变化同步更新 `docs/IMPLEMENTATION_PLAN.md`。
- 不提交 AOCI 的机器运行时资产；`aoci.txt`、`aoci.meta.txt`、`aoci.code.txt`、`.aoci/config.json`、`.aoci/baseline.json`、`AGENTS.md`、`.gitattributes` 属于应纳入版本管理的认知资产。
