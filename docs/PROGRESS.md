# OneAPI 进度

- 最后更新：2026-10-29
- 当前阶段：v0.1.14 已发布（跨协议推理修复与测试代码清理）
- 当前分支：`main`
- 最近发布：`v0.1.14`
- 计划文档：`docs/IMPLEMENTATION_PLAN.md`

## 里程碑

- [x] **M0 脚手架**：Cargo / Vite / Makefile / Dockerfile / migrations / config / `/healthz` / 静态资源嵌入与 SPA fallback。
- [x] **M1 管理基础**：DB 层、管理员初始化/登录/会话、渠道与 Key CRUD、设置。
- [x] **M2 协议层（非流式）**：统一 IR、三协议请求/响应编解码、同协议 fast path、6 条跨协议组合单测。
- [x] **M3 网关（非流式）**：路由/加权/故障转移、上游 client（代理/请求头/超时）、用量日志、错误映射。
- [x] **M4 Responses 状态**：落库、`previous_response_id` 展开、GET/DELETE、Key 归属校验。
- [x] **M5 流式**：SSE 解码/编码状态机、tee 透传、首 event 失败切换、断连记 499。
- [x] **M6 前端**：登录、仪表盘、渠道、Key、日志、设置、Playground。
- [x] **M7 收尾**：Docker、README/部署文档、CI、保留期清理任务、`reset-password`、clippy/fmt 全绿。

## 当前状态

- M0 已完成：Rust 后端、Vite + React + shadcn/ui 前端、SQLite 迁移、静态资源嵌入、健康检查、Makefile/Dockerfile 均已落地。
- 环境：Rust 1.99.1；Node 24.22.1 / npm 11.19.1 位于 `/root/.local/share/vite-plus/bin`，Makefile 已自动把它加入 PATH。
- 前端依赖：Vite 7.3.7、React 18.3.1、Tailwind CSS 4.3、shadcn/ui（base-nova / Base UI 1.9）、lucide-react、React Router 7.18.4、TanStack Query 5.105.0、Recharts 3.8、sonner、next-themes。
- M1 已完成：管理员引导登录、HttpOnly Cookie 会话、上游 Key AES-256-GCM 加密、渠道 CRUD、API Key CRUD、设置接口，以及登录/渠道/Key/设置前端页面。
- M2 已完成：统一 IR、三协议请求/响应编解码、同协议往返与跨协议转换。
- M3 已完成：网关 `/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1/models`；API Key 鉴权、渠道优先级/权重选择、失败重试与冷却、上游代理/自定义头/超时、用量日志、同协议透传与跨协议转换；后台渠道测试与模型拉取。
- M4 已完成：Responses 本地落库、`previous_response_id` 历史展开、GET/DELETE、Key 归属校验；`store=false` 不落库且后续引用返回 400。
- M5 已完成：三协议 SSE 解码/编码、同协议 tee 透传、跨协议 6 条流路径、首 event 失败切换、逐 chunk 空闲超时、断连 499、Responses 流式落库与回读。
- M6 已完成：请求日志页与详情抽屉、仪表盘统计/时序/占比/最近错误、Playground 三协议（含流式）、`/api/logs`、`/api/dashboard/*`、`/api/models`、`/api/playground`、路由级懒加载与暗色模式。
- M7 已完成：GitHub Actions CI、Tag 触发的六平台 Release（Linux/Windows/macOS × amd64/arm64）、GHCR 多架构 Docker 镜像、Web 安装向导、保留期清理、`reset-password` 与请求/响应正文记录。
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
  - Linux amd64 `x86_64-unknown-linux-musl`（`ubuntu-24.04`，musl 静态）
  - Linux arm64 `aarch64-unknown-linux-musl`（`ubuntu-24.04-arm`，musl 静态）
  - Windows amd64 `x86_64-pc-windows-msvc`（`windows-2025`）
  - Windows arm64 `aarch64-pc-windows-msvc`（`windows-11-arm`）
  - macOS amd64 `x86_64-apple-darwin`（`macos-15-intel`）
  - macOS arm64 `aarch64-apple-darwin`（`macos-15`）
- 发布产物命名 `oneapi-<tag>-<platform>.tar.gz|.zip`，包含二进制、`README.md`、`LICENSE`、`.env.example`；Release 同时上传 `SHA256SUMS`。构建任务上传原始二进制，Release 任务统一打包一次，Actions artifact 与 Release asset 都不会出现嵌套压缩包。
- Actions 全部升级到 Node 24 版本（checkout/setup-node/upload-download-artifact/docker/*/gh-release）；runner 固定为 `ubuntu-24.04`、`windows-2025`、`macos-15-intel`、`macos-15`、`ubuntu-24.04-arm`、`windows-11-arm`，避免 `ubuntu-latest` 迁移提示。
- Docker 任务在六个平台构建完成后，下载 Linux amd64/arm64 产物，使用 `Dockerfile.release` 组装并推送 `ghcr.io/langfordkuo/oneapi` 多架构镜像；避免 QEMU 下二次编译 Rust，标签包含 semver、`v<tag>`、`sha-<commit>` 与 `latest`。
- Dockerfile 升级为 `node:24-alpine` + `rust:1-slim-bookworm` + `debian:bookworm-slim`，SQLite 使用 bundled 构建，运行镜像无需额外 SQLite 库；增加 cargo registry/npm 缓存与 OCI label。
- 本地校验：两个 workflow 通过 PyYAML 解析；runner 标签、Windows ARM64 Tier 1 host 工具链、`windows-11-arm` 的 VS ARM64 工具与 Rust 预装均已核对。
- GitHub Actions dry-run（`workflow_dispatch`，run `40464773247`）：validate、web、六个平台 build（Linux/Windows/macOS × amd64/arm64）与 Docker `linux/amd64,linux/arm64` 组装全部成功；注解仅剩 macOS arm64 容量提示 notice，Node 20 弃用与 `ubuntu-latest` 迁移提示已清除。手动触发按设计跳过 Release 发布且不推送镜像。
- Linux 产物已验证为 musl 静态链接：amd64 为 `static-pie`、arm64 为 `static`，`ldd` 分别显示 `statically linked` / `not a dynamic executable`，`NEEDED=0`、`GLIBC` 符号数为 0；x86_64 产物在本地启动并通过 `/healthz`。因此 Linux 产物不再要求 glibc 版本，可用于 Debian 11、Ubuntu 20.04、Alpine 等系统。
- 已下载并校验 artifact：六个 artifact 均为原始可执行文件（ELF x86_64/aarch64、Mach-O x86_64/arm64、PE x86_64/ARM64）；按 Release 脚本打包后，Linux tar 与 Windows zip 根目录仅含二进制、README、LICENSE、`.env.example`，无嵌套压缩包；`gh run download` 全量下载成功。
- 已关闭 Buildx 构建记录 artifact 上传，避免 `gh run download` 全量下载遇到非 zip 缓存文件；Docker 任务使用 `pattern: oneapi-*` / `oneapi-linux-*` 精确下载。
- 尚未推送 `v*` 标签，因此 GHCR 首次镜像推送与 GitHub Release 发布待正式发版时验证。

## M7 安装向导验证记录

- 新增 `GET /api/setup/status` 与 `POST /api/setup`：无管理员时返回 `initialized=false` 及监听地址、数据目录、SQLite 路径、`.env` 路径；提交管理员后创建 Argon2id 哈希、写入 `<data_dir>/.env`（Unix 0600）、签发会话并自动登录；初始化后再次提交返回 400。
- 首次启动引导逻辑：未设置 `ONEAPI_ADMIN_PASSWORD` 且无管理员时不再生成随机密码，改为提示通过 Web 安装页完成初始化；设置了 `ONEAPI_ADMIN_PASSWORD` 时仍支持无界面自动创建管理员（Docker/服务端场景）。
- 配置加载：启动时先读当前目录 `.env`，再读数据目录 `.env`，且不覆盖已存在的进程环境变量；安装向导写入绝对 `ONEAPI_DATA_DIR`，避免从不同工作目录重启时找错数据。
- 前端新增安装页：显示版本、监听地址、数据目录、SQLite 与 `.env` 路径，填写管理员用户名/密码/站点名；成功后展示 `.env` 路径并自动登录。App 启动时先查询安装状态，未初始化直接显示安装页，初始化后才进入登录/后台。
- `cargo fmt` / `clippy -D warnings` / `cargo test`：通过，60 个测试（新增安装向导与 `.env` 加载测试）。
- 进程级验证：首次启动 `/api/setup/status` 返回 `initialized=false`；提交安装后 `/api/auth/me` 立即 200，`.env` 权限为 `600` 且不含明文密码；仅用 `ONEAPI_DATA_DIR` 重启时从数据目录 `.env` 读取监听地址，管理员登录仍成功。
- `.env` 不写入管理员密码（仅注释占位），密码只以 Argon2id 哈希存入 SQLite；如需重新引导空管理员表，设置 `ONEAPI_ADMIN_PASSWORD` 后启动。

## M7 收尾验证记录

- 保留期清理：服务启动时及之后每 6 小时执行；`usage_log_retention_days` 清理 `usage_logs` 与 `request_details`，`response_retention_days` 清理 `responses`，`0` 表示永久保留。单元测试覆盖旧/新数据边界与 0 禁用；进程级验证中旧 usage/detail 被删除、近期记录保留。
- `reset-password`：新增 `oneapi reset-password --password ...`（也可从 stdin 读取），更新 Argon2id 哈希并删除该管理员全部会话；进程级验证旧密码返回 401、新密码 200。
- 请求/响应正文记录：设置页开启后，非流式与流式请求都会保存正文到 `request_details`，单条最多 64KB；日志详情接口可直接读取。进程级验证 Chat 非流式与 SSE 流式均写入且可回读。
- `cargo fmt` / `clippy --all-targets -D warnings` / `cargo test`：通过，65 个测试。
- `npm run build` 与 `cargo build --release`：通过。

## Post-v0.1.0 实时指标验证

- 新增内存滑动窗口指标（10 秒、按秒分桶），非流式请求在完成时计入 completion tokens；流式请求在文本/推理/工具参数增量中估算 tokens，结束时用 usage 校准。
- 新增 `GET /api/metrics/stream` SSE：每秒推送 `{timestamp, global_tok_s, channels:[{channel_id,tok_s}]}`，需要管理员会话。
- 渠道页新增“实时 tok/s”列与全站汇总标签；主页新增“全站实时输出”卡片；前端通过 EventSource 订阅，自动重连，无需刷新页面。
- `cargo fmt` / `clippy -D warnings` / `cargo test`：通过，67 个测试。
- 进程级验证：非流式请求后 SSE 推送 `global_tok_s=0.2`、渠道 `tok_s=0.2`；慢速流式请求进行中 SSE 依次推送 0.1 → 0.4 → 0.7 tok/s。

## Post-v0.1.0 在线更新验证

- 新增设置项 `update_source`（`github` / `ghproxy`），以及 `GET /api/update/check`、`POST /api/update/apply`。
- 默认从 GitHub Releases API 获取最新版本；切换 `ghproxy` 后 API 与下载 URL 都走 `https://gh-proxy.com/`。
- 更新流程：按平台选择 `oneapi-<tag>-<platform>.tar.gz|.zip` → 下载 `SHA256SUMS` → 校验 → 解压出二进制 → `self-replace` 替换 → 优雅关闭服务 → Linux/macOS `exec` 新二进制，Windows 启动新进程。
- 安全检查：仅管理员可调用；只接受 GitHub/proxy 官方资产命名；强制 SHA256SUMS；容器环境默认拒绝；更新进行中拒绝并发。
- 验证：真实 GitHub 与 gh-proxy 的检查接口均返回 v0.1.0；本地 mock 发布 v9.9.9 完成“检查→下载→校验→替换→重启”，重启标记文件写入成功；71 个测试通过。

## v0.1.1 发布记录

- 版本号从 `0.1.0` 升至 `0.1.1`，包含实时 tok/s 与后台在线更新功能。
- 发布流程仍为六平台构建 + GitHub Release + GHCR 多架构镜像；Linux 使用 musl 静态链接。
- 安装包命名 `oneapi-v0.1.1-<platform>`，`SHA256SUMS` 可供在线更新校验。
- 发布后发现 Linux/macOS 归档中的二进制权限为 0644，已重新打包并覆盖 v0.1.1 资产；Release workflow 已补 `chmod +x`，后续版本不会再出现该问题。
- 修复空数据库仪表盘 500：`COALESCE(AVG(latency_ms), 0)` 在无数据时返回 INTEGER，与 SQLx 的 `f64` 不兼容；改为 `0.0`，并新增空库 summary/timeseries/breakdown 回归测试（72 个测试通过）。

## v0.1.2 发布记录

- 版本号从 `0.1.1` 升至 `0.1.2`，包含审查计划阶段 A–D 与阶段 E3（渠道指数退避 + 半开探测）。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（89 个测试）、`npm run lint`、`npm run build` 全部通过。
- 发布前全量实测：release 二进制 + 本地 mock 上游 + 真实 HTTP 流程，51 项断言全部通过，覆盖安装向导/登录/Key/渠道 CRUD、同协议与跨协议（Chat↔Anthropic/Responses）非流式与流式、故障转移、首次失败不冷却、第二次失败进入冷却、冷却期跳过、并发半开探测仅放行一个请求、探测成功清零、探测占用时返回 `503 channel_recovering`、Responses 落库与 `previous_response_id`、401/404 错误形状、仪表盘/日志聚合与内嵌前端。

## v0.1.3 发布记录

- 修复日志列表筛选只作用于 `total`、不作用于返回行的问题：`usage_logs` 行查询缺少 WHERE 入口，筛选片段被并入最后一个 `LEFT JOIN` 的 ON 子句，导致仪表盘“最近错误”混入 200 请求，日志页切换筛选后当前页内容不变。
- 补充筛选回归测试：`status=error` 仅返回 `>=400`、`status=success` 仅返回 `<400`、`model` 过滤与不存在的模型返回空列表。
- 验证：89 个测试通过，clippy/fmt/lint/build 通过；release 二进制实测筛选结果与 `total` 一致（错误 2 行、成功 13 行、不存在的模型 0 行）。

## 真实客户端 IP 请求头

- 新增设置项 `client_ip_header`：后台「设置」页可指定从哪个请求头读取客户端 IP，留空时使用 TCP 对端地址。
- 支持 `CF-Connecting-IP`（Cloudflare）、`X-Forwarded-For`（取列表中第一个可解析地址）、`X-Real-IP`、`True-Client-IP`，也接受任意合法 HTTP 头名；头缺失或值无法解析时回退 TCP 对端地址。
- 登录失败限速改为按解析后的 IP + 用户名计数，解决反向代理或 Cloudflare 后所有请求共享代理 IP 的问题。
- 安全边界：只有在所有流量都经过可信代理时才应配置；直接暴露服务时客户端可伪造该请求头绕过限速，设置页与 README 均有提示。
- 验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（97 个测试）与前端 `npm run lint`、`npm run build` 全部通过。

## v0.1.4 发布记录

- 版本号从 `0.1.3` 升至 `0.1.4`，发布真实客户端 IP 请求头设置。
- 新增 `client_ip_header` 设置：后台可指定 `CF-Connecting-IP`、`X-Forwarded-For`、`X-Real-IP`、`True-Client-IP` 或任意合法头名；留空使用 TCP 对端地址。
- 登录失败限速改为按解析后的客户端 IP + 用户名计数；头缺失或值无效时回退 TCP 对端地址。
- 新增 6 个解析单元测试与 2 个后台集成测试，覆盖头名校验、保存回读、XFF 多值、IPv6/端口、无效值回退与按请求头分桶限速。
- 验证：97 个测试通过；`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`npm run lint`、`npm run build` 全部通过；release 二进制端到端实测 54 项断言通过。

## 日志请求速度

- `/api/logs` 与 `/api/logs/{request_id}` 新增 `tokens_per_second`：按输出 tokens 除以生成耗时计算；流式请求扣除首 token 等待时间，非流式使用整个请求耗时。
- 日志列表新增“速度”列，详情抽屉新增“生成速度”，统一显示为 `xx.x tok/s`；无 token 或耗时为零时显示 `-`。
- 验证：新增 4 个计算单元测试，并在日志集成测试中断言列表与详情速度一致；101 个测试通过。

## 渠道多协议

- 单个渠道新增 `kinds`（支持的协议集合）与 `fallback_kind`（协议不匹配时的转换目标）；支持同时勾选 Chat Completions、Responses 与 Anthropic。
- 路由时若渠道支持入站协议，则原样转发、不做转换；否则转换到 `fallback_kind`，未配置时使用第一个支持的协议。
- 新增迁移 `0004_channel_kinds.sql`：为已有渠道回填单协议 `kinds` 与 `fallback_kind`，旧字段 `kind` 保留为主协议以兼容旧客户端。
- 设置页渠道表单改为多选协议，并显示当前可选的转换目标；自定义请求头与模型映射仍由整个渠道共享。
- `count_tokens` 只在渠道原生支持 Anthropic 时转发，否则保持原有字符估算。
- 验证：新增 4 个协议选择单元测试、2 个网关集成测试（原生匹配不转换、缺失协议走 fallback）与 1 个渠道 CRUD 测试；108 个测试通过。

## v0.1.14 发布记录

- 版本号从 `0.1.13` 升至 `0.1.14`。
- 跨协议推理输出修复：Anthropic 编码器在流式与非流式响应中输出 `thinking`/`thinking_delta`，不再因缺少上游签名丢弃推理；流式 IR 新增 `ReasoningSignature`，Anthropic 的 `signature_delta` 可转发，Responses 写入 `encrypted_content`；Anthropic 内容块改为顺序输出。
- 测试代码清理：删除全部 `#[cfg(test)]` 模块、`src/admin/tests.rs` 与 test-only helper；移除 dev-dependencies `tempfile`/`tower`/`wiremock`；CI 与 Makefile 不再执行测试。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo build` 与前端 `npm run build` 通过；`cargo test` 为 0 个测试；真实二进制冒烟确认 Chat→Anthropic 先输出 thinking 再输出正文。

## v0.1.13 发布记录

- 版本号从 `0.1.12` 升至 `0.1.13`。
- 轻量备份与恢复：启动时 `PRAGMA quick_check`；有未应用迁移时先 `VACUUM INTO` 快照到 `data/backups`（保留最近 5 份）；设置页支持导出/导入 SQLite 快照，导入校验后自动重启换库并保留当前库。
- 故障转移预算：单请求最多尝试 3 个渠道、累计 60 秒后停止切换，同一请求语义错误在第二个渠道重复出现时提前结束。
- 运行状态：`/api/system` 与仪表盘新增数据库/WAL 大小、最近备份时间、写队列深度、冷却渠道数与 24 小时故障转移次数。
- 每日额度：API Key 新增 `daily_token_limit`，按 UTC 自然日统计，超限返回 429 且次日自动恢复；迁移 `0008_api_key_daily_limit.sql` 同时为 `usage_logs.created_at` 增加索引。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（196 个测试）与 `web/` `npm run build` 通过；本地二进制实测初始化、导出、导入换库与重启后会话保持。

## v0.1.12 发布记录

- 版本号从 `0.1.11` 升至 `0.1.12`。
- 同模型渠道无缝故障转移：任何上游失败（任意 4xx/5xx、连接/超时、空响应、非 JSON、解析/编码失败、2xx 内 `error`）都会切换到下一个同模型候选，候选耗尽才向客户端报错；全部失败时优先返回第一个请求语义错误的状态码，纯渠道故障仍为 502；渠道冷却只由渠道侧失败驱动，400/413/422 不冷却渠道；同模型渠道全部冷却时返回 503 `channel_recovering`；`count_tokens` 同样支持多候选切换。
- 冷却期主动恢复：新增 `src/recovery.rs`，每 60 秒探测有失败记录且处于冷却的渠道，单请求 10 秒超时、最多 8 路并发，成功立即清零失败状态并失效渠道缓存，失败不增加失败计数且不缩短既有退避。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（189 个测试）通过。

## v0.1.11 发布记录

- 版本号从 `0.1.10` 升至 `0.1.11`。
- 完成两轮全仓复审：SSE 单帧上限与裸 CR、Responses 透传零拷贝与轻量观测、健康渠道成功跳过写库、`count_tokens` 资源复用、同协议 usage 精简解析、Responses 增量历史存储与链式展开、请求详情并入写队列、渠道掩码缓存、选路 `Arc<PreparedChannel>`、缓存 single-flight、启动前加载 `.env`、`client_ip_header` 缓存等。
- 归档并删除已完成的 `docs/REVIEW_PLAN.md` 与 `docs/OPTIMIZATION_PLAN.md`；后续能力与已知限制移入 `docs/IMPLEMENTATION_PLAN.md` §16。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（177 个测试）与 `web/` `npm run build` 通过。

## v0.1.10 发布记录

- 版本号从 `0.1.9` 升至 `0.1.10`，完成优化计划剩余项 P1-8、P2-6、P2-8、P3-8；P1-6（密钥派生 v2）按决定不处理。
- P1-8：流式 Responses 的 `response.completed` 终帧延后到落库之后再发送；超过 10MB 或落库失败时终帧改写为 `store:false`，客户端看到的 `store` 与后续 GET 一致。新增 11MiB 流式响应的集成测试。
- P2-6：渠道失败改为更新进程内健康覆盖层（`RuntimeCache.health`），不再每次失败都失效并重载全部渠道；冷却、半开探测与 30 秒 TTL 行为保持不变，新增 `Arc::ptr_eq` 回归测试。
- P2-8：非流式响应改为字节级读取与解析；同协议直接透传上游字节，Responses 注入字段后只序列化一次，去掉 `String` 中间层和整棵 JSON clone。
- P3-8：更新客户端强制 HTTPS 且最多 5 跳、拒绝降级；`ONEAPI_UPDATE_API_BASE` 必须为 HTTPS；写入前校验资产名不能逃逸 updates 目录；README 补充更新信任模型。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（156 个测试）全部通过。

## v0.1.9 发布记录

- 版本号从 `0.1.8` 升至 `0.1.9`，完成优化清单批次 1–5（含 P3-7，原文件已归档至实施计划 §16）。
- 安全：登录/改密限速、设置事务化、CSRF 转发头取值、短 Key 掩码；上游重定向限制与故障转移总预算；协议化 JSON 解析错误与 `include_usage` 兼容重试。
- 性能：日志分页上限与可选总数、运行时缓存补齐、可配置连接池与 `/readyz` 队列指标、加权选路 O(n log n)、实时指标分片、SSE 字节级解析。
- 前端与响应头：SSE 读取器与增量换行、Playground 流取消、Key 限额整数校验、全局 `nosniff`/`X-Frame-Options`/`Referrer-Policy`/CSP。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（152 个测试）、`npm run build`、`npm run lint`（0 error）全部通过；`/readyz` 与静态资源响应头实测通过。

## v0.1.8 发布记录

- 版本号从 `0.1.7` 升至 `0.1.8`，API Key 新增三类可选限制，留空即不限制。
- 可用渠道：按渠道 ID 限制路由候选；模型目录同样只展示允许渠道能提供的模型。
- 可用模型：按模型名限制，支持 `*` 通配；网关请求与 `/v1/models` 均按限制过滤，未授权模型返回 403 `permission_error`。
- Token 上限：`token_usage` 累计 total_tokens，写入 usage_logs 的同一事务内累加；达到上限自动置 `enabled=0`，后续请求返回 403。迁移 `0006_api_key_limits.sql` 会按历史 usage_logs 回填现有 Key 的用量。
- Key 页支持创建/编辑限制，列表展示渠道/模型摘要、用量进度、上限与「已超限」状态。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test`（122 个测试）、`npm run typecheck`、`npm run lint`、`npm run build` 全部通过；运行时验证模型限制 403、模型目录过滤、渠道限制选路、Token 达限自动停用，Playwright 验证创建/编辑/列表展示。

## v0.1.7 发布记录

- 版本号从 `0.1.6` 升至 `0.1.7`，包含渠道按协议内分页与 Token 图表轴修复。
- 渠道编辑改为 OpenAI Chat / OpenAI Responses / Anthropic 三个协议页，每页独立 Base URL、Key、鉴权头、模型、映射与自定义请求头；每页可「不转换」或转换到另一个原生协议，转换时复用目标协议配置。转换目标只能指向原生协议，至少保留一个原生协议。
- 新增迁移 `0005_channel_protocol_configs.sql` 与启动 backfill；旧渠道升级后自动生成三条路由，行为不变，旧扁平 API 字段继续兼容。
- 网关按入站协议解析最终使用的原生配置；模型目录汇总原生协议；探活与拉取模型可按当前协议页执行。
- Token 用量图表 Y 轴改用「万 / 亿」紧凑格式并加宽，避免大数值超出卡片；Tooltip 保留千分位完整值。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test`（117 个测试）、`npm run typecheck`、`npm run lint`、`npm run build` 全部通过；旧 v0.1.6 数据目录升级、三协议独立地址/Key、转换路由与 Playwright 交互均实测通过。

## v0.1.6 发布记录

- 版本号从 `0.1.5` 升至 `0.1.6`，包含前端整体重写、网站域名设置与实时速度改造。
- 前端移除 Ant Design 与 `@ant-design/plots`，改用 shadcn/ui（base-nova / Base UI）+ Tailwind CSS 4 + Recharts；安装、登录、仪表盘、渠道、Key、日志、Playground、设置全部重写，补齐移动端布局与暗色模式。
- 设置页新增「网站域名」`site_domain`：反向代理未透传外部 Host 时，CSRF 校验额外接受该域名；Origin、Host 与配置值统一归一化为 `host[:port]`，忽略协议、路径与 80/443 默认端口。
- 实时 tok/s 移除 10 秒滑动窗口，改为上一完整秒的 token 增量；流结束或空闲一秒后归零。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test`（113 个测试）、`npm run typecheck`、`npm run lint`、`npm run build` 全部通过；Playwright 全流程与慢速流 SSE 实测通过。

## v0.1.5 发布记录

- 版本号从 `0.1.4` 升至 `0.1.5`，包含日志每请求 tok/s 与渠道多协议。
- 渠道可同时配置 Chat Completions、Responses、Anthropic；入站协议匹配时原样转发，否则转换到配置的 `fallback_kind`。
- 新增迁移 `0004_channel_kinds.sql`，旧渠道自动回填 `kinds` 与 `fallback_kind`；已在 v0.1.4 数据目录上验证升级。
- 日志列表与详情新增 `tokens_per_second`，流式按扣除首 token 等待的生成耗时计算。
- 验证：108 个测试通过，`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`npm run lint`、`npm run build` 全部通过；release 二进制端到端实测 54 项断言通过。

## 审查计划阶段 A（v0.1.2 修复与一致性）

- 新增审查计划，按阶段记录审查结论与验收标准（原文件已归档至实施计划 §16）。
- 设置项接线：`default_max_tokens` 用于跨协议编码到 Anthropic 时的缺省值；`nonstream_timeout_seconds` / `stream_idle_timeout_seconds` 作为渠道未配置 `timeout_ms` 时的默认超时；`site_name` 通过公开 `GET /api/site` 用于登录页、安装页、侧边栏与浏览器标题。
- 新增 `POST /v1/messages/count_tokens`：Anthropic 上游原样转发，其他上游按字符数估算并返回 Anthropic 形状。
- 通配符渠道的模型列表回退到最近 30 天 `usage_logs` 中出现的模型；Playground 模型选择改为可自由输入的 AutoComplete。
- 渠道测试/拉取模型新增未保存配置接口 `POST /api/channels/test` 与 `/api/channels/fetch-models`；前端抽屉直接提交当前表单。
- 转发客户端白名单请求头（`anthropic-beta`、`openai-beta` 等），鉴权头与 Cookie 不转发；渠道 `extra_headers` 优先。
- 新增迁移 `0002_usage_cache_creation.sql`，日志记录并展示 `cache_creation_tokens`。
- 后端校验渠道 `weight`（1–1000）与 `priority`（-10000–10000）；日志页空态文案修正。
- 验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（79 个测试）、`npm run build` 全部通过。

## 审查计划阶段 B（安全与稳定性）

- `/api/system` 改为需要管理员会话，未登录不再暴露数据目录。
- 管理员登录新增失败限速：按客户端 IP + 用户名计数，5/10/15 次失败分别锁定 30s/5min/15min，返回 429；服务启动改为携带 ConnectInfo。
- 新增 `ONEAPI_TRUST_PROXY`：受信反代后可改用 `X-Forwarded-Host` 做 CSRF 同源判断；安装向导同步写入该配置。
- Responses `previous_response_id` 展开增加 2000 条 / 16MB 上限，超限返回 400。
- 保留期清理任务顺带删除过期会话（启动与每 6 小时）。
- 在线更新完成后清理临时二进制；README 补充“GitHub 账号 + HTTPS”信任模型说明。
- 前端实时指标改用 fetch + SSE 解析，401 时停止重连；网关 SSE 在等待上游时每 15 秒发送心跳注释，且心跳不会延长上游空闲超时。
- 验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（82 个测试）、`npm run build` 全部通过。

## 审查计划阶段 C（性能与资源效率）

- 新增渠道级 reqwest Client 缓存：按渠道 id + updated_at + 超时 + 流式标记复用连接池；渠道更新/删除与设置变更时清空。
- 新增进程内渠道与网关设置缓存（30 秒 TTL），后台写接口主动失效；`GatewayDefaults` 改为一次查询读取全部设置。
- usage 日志改为单写队列（容量 1024、批量 100、事务写入），队列满时回退同步写；渠道成功状态更新只在确需重置时执行。
- 对所有非 SSE 响应启用 gzip 压缩。
- 日志 model / request_id 过滤改为 GLOB 前缀匹配并转义元字符；新增 status/source/channel + created_at 复合索引迁移 `0003_usage_log_indexes.sql`。
- 跨协议请求只解析 IR 一次，失败切换时复用。
- 实时指标改为原子分桶，减少锁竞争；前端构建按 react/query/antd 拆分 vendor chunk。
- 验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（84 个测试）、`npm run build` 全部通过。

## 审查计划阶段 D（工程卫生与文档同步）

- 依赖清理：移除未使用的 `eventsource-stream`；`tower` 仅测试使用，移入 `[dev-dependencies]`；`tower-http` 去掉未启用的 `timeout`、`limit` feature。
- 移除 `src/error.rs` 中已无必要的 `#[allow(dead_code)]`，Clippy 仍保持零警告。
- 文档同步：README 更新发布进度、真实 CI runner、环境变量与 `0.1.1` 镜像/标签示例；`docs/IMPLEMENTATION_PLAN.md` 补充 `GET /api/site`、ESLint 与新环境变量，并新增变更记录。
- 前端版本对齐 `package.json` / `package-lock.json` 到 `0.1.1`。
- 引入 ESLint 10 flat config（typescript-eslint、react-hooks、react-refresh），新增 `npm run lint`，CI 前端任务增加 lint 步骤；Playground 默认模型改为派生值，消除 effect 内 setState。
- `.env.example` 补齐 `ONEAPI_TRUST_PROXY`、`ONEAPI_ALLOW_SELF_UPDATE`、`ONEAPI_UPDATE_API_BASE` 说明。
- 验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`、`npm run lint`、`npm run build` 全部通过。

## 审查计划阶段 E3（渠道健康与退避）

- 渠道冷却从固定 60s 改为指数退避：首次失败只记录错误；第二次连续失败起冷却 60s，之后按 120s/240s/480s/960s 翻倍，30 分钟封顶；成功清零。
- 冷却到期后进入半开状态：进程内 `ChannelProbes` 只放行一个探测请求，其余请求跳过该渠道；探测成功立即恢复，失败则继续按指数退避冷却。流式探测期间 lease 随流任务持有，首帧成功后其他请求即可正常进入。
- 新增 `AppState::mark_channel_failure` / `mark_channel_success`：失败写入后立即失效渠道缓存，冷却不再受 30s 缓存 TTL 影响；健康渠道成功时不产生额外写放大。
- 当所有候选渠道都处于探测占用状态时，网关返回 `503 channel_recovering`，避免把可恢复状态误报为 502。
- 新增退避序列与探测闸门单测；验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test --locked`（89 个测试）、`npm run lint`、`npm run build` 全部通过。

## 前端重写（shadcn/ui，2026-10-28）

- 移除 Ant Design 5、`@ant-design/icons` 与 `@ant-design/plots`，整套前端改用 shadcn/ui（base-nova，Base UI 原语）+ Tailwind CSS 4；主题变量集中在 `web/src/index.css`，暗色模式由 next-themes 的 class 策略驱动。
- 重写应用外壳与全部页面：安装、登录、仪表盘、渠道、API Key、日志、Playground、设置；侧边栏移动端折叠为 Sheet，表格按断点隐藏次要列，图标按钮在窄屏省略文字。
- 图表改用 Recharts（`DashboardCharts` 懒加载），构建分块调整为 react / query / charts。
- 表单迁移为受控组件；渠道抽屉保留未保存配置的“测试连通性”和“拉取模型”；Base UI Select 统一通过 `items` 显示中文标签。
- 修复迁移中暴露的交互问题：`Button` 改为 forwardRef 以配合 Base UI `render`；账户菜单 `MenuGroupLabel` 包进 `MenuGroup`；Sheet 宽度用 `data-[side=right]` 变体覆盖；CodeBlock 增加 `min-w-0` 避免长 Key 撑破对话框；toast 移至顶部居中避免遮挡抽屉关闭按钮；修改密码返回 401 时不再整页跳转并提示“当前密码不正确”。
- 开发代理将 `Origin` 重写为后端地址，使 Vite 代理下的写请求通过 CSRF 同源校验。
- 清理未使用的 calendar / popover / tooltip / separator 组件与 `react-day-picker`、`date-fns`、`clsx`、`tailwind-merge` 直接依赖（`cn` 已内置合并逻辑）。
- 验证：`npm run typecheck`、`npm run lint`（0 error，4 个 react-refresh warning）、`npm run build` 通过；生产产物 `index` 258 kB（gzip 88 kB）、`charts` 381 kB（gzip 112 kB）、CSS 92 kB（gzip 15.5 kB）。
- Playwright 全流程（本地后端 + mock 上游）：安装向导、登录、渠道创建/编辑/删除、Key 创建/启停、日志筛选与详情、Playground 流式回复、设置保存与密码错误提示、暗色模式、390px 移动端与导航抽屉均通过；控制台无运行时错误（仅登录前 `/api/auth/me` 的预期 401）。

## 反向代理域名与实时速度（2026-10-28）

- 新增设置项 `site_domain`（设置页「网站域名」）：后端把 Origin、Host 与配置值统一归一化为 `host[:port]`，忽略协议、路径和 80/443 默认端口；当 Origin 与请求 Host 不一致时，只要匹配配置域名即放行写请求。留空保持原有行为，仅校验 Host。
- 域名保存时做校验与归一化，仅接受 ASCII 域名/IP（IPv6 用方括号）和合法端口；非法值返回 400。配置随 `RuntimeCache` 缓存 30 秒，设置更新后立即失效。
- CSRF 中间件改为直接使用 `AppState`，以便读取数据库设置；`admin::routes` 接收 `AppState`。
- 实时 tok/s 移除 10 秒滑动窗口：内存改为“当前秒 + 上一完整秒”两个计数桶，SSE 每次推送上一完整秒的 token 增量；流式结束或空闲一秒后归零。前端提示改为「最近 1 秒实测」。
- 慢速流式实测（5 tok/s）：SSE 连续推送 `global_tok_s=5.0`、`channel 1 tok_s=5.0`，结束后 1 秒内回到 0；非流式请求的 token 会在完成后显示 1 秒。
- 验证：`cargo fmt`、`cargo clippy --all-targets -D warnings`、`cargo test`（113 个测试）通过；设置页保存域名、代理 Origin 放行、恶意 Origin 拒绝均通过 curl/Playwright 实测；前端 `npm run typecheck`、`npm run lint`、`npm run build` 通过。

## 渠道按协议内分页（2026-10-28）

- 渠道表单把 OpenAI Chat / OpenAI Responses / Anthropic 做成内分页，每页独立保存 `base_url`、加密 Key、`auth_header`、`models`、`model_mapping`、`extra_headers`；渠道级继续共用名称、启用、优先级/权重、超时与代理。
- 每页「转换目标」支持「不转换」和另一个原生协议：选「不转换」展示本页详细配置；选某协议后隐藏本页配置，入站请求转换成该协议并完全复用目标页的地址、Key、模型与映射。转换目标只能指向 `mode=native` 的协议，禁止链式转换和循环。
- 新增迁移 `0005_channel_protocol_configs.sql` 与 `protocol_configs` JSON 列；启动时 `backfill_protocol_configs` 为旧渠道生成三条路由（原 `kinds` 为 native，其余转换到原 `fallback_kind`），升级后行为不变。旧列保留为第一个原生协议的兼容视图。
- 网关路由改为按入站协议解析 `native` 或 `convert -> target`，渠道选择与模型映射使用最终生效的原生配置；`/api/models` 与 `/v1/models` 汇总所有原生协议的模型；`count_tokens` 只在解析到 Anthropic 原生上游时转发。
- 后端 API 兼容旧的扁平字段（`kinds`/`fallback_kind`/`base_url` 等），新客户端改传 `protocols` 数组；探测接口新增 `probe_protocol`，用当前 Tab 的路由测试/拉取模型。
- 验证：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test`（117 个测试）通过；用旧 v0.1.6 数据目录启动验证 backfill；三个原生协议分别命中不同 Base URL 与不同 Key，Chat/Response 转换到 Chat 的路由与日志均正确；Playwright 验证内分页、转换隐藏配置、最后一个原生协议无法再转换、探活使用当前页配置。

## 代码审查修复（2026-10-28）

基于 v0.1.8 全仓审查，按优先级完成以下修复（未修改已发布迁移，新增 `0007_request_details_key.sql`）：

- **Token 计费兜底**：流式请求在客户端提前断开、上游不返回 usage 或 usage 只到中途时，使用本地估算（输入按请求正文、输出按增量字符数/4）补齐 `usage_logs` 与 API Key 的 `token_usage`，避免额度被绕过；非流式响应缺 usage 时同样估算。
- **请求 ID 与请求详情隔离**：`x-request-id` 仅接受不超过 128 字符、只含 `[A-Za-z0-9._:-]` 的值，否则生成 UUID；`request_details` 改为 `(request_id, api_key_id)` 主键（`api_key_id=0` 表示 Playground 与历史行），不同 Key 复用同一请求 ID 不再互相覆盖；新增 `created_at` 索引。
- **渠道 Client 缓存**：缓存键移除易变的 `updated_at`，渠道失败计数更新不再每次都创建新 `reqwest::Client` 并永久保留旧连接池。
- **上游响应上限**：非流式与流式错误响应体最多读取 64MB（`Content-Length` 与分块累计双重校验）；上游错误消息/类型截断后写入日志与渠道 `last_error`。
- **保留期清理**：`usage_logs`、`request_details`、`responses`、`sessions` 改为每批 5000 行、独立事务删除，避免长时间持有 SQLite 写锁导致 `busy_timeout` 丢日志。
- **客户端 IP 请求头**：留空时默认读取 `X-Forwarded-For`，但只对回环/内网对端生效；显式填写后视为管理员已确认可信并直接采用，适配 CDN 从公网地址回源。显式配置时需确保源站端口只接受 CDN/反向代理访问，否则可被伪造绕过登录限速；README、`.env.example` 与设置页提示同步更新。
- **性能**：运行时缓存保存预解析的 `PreparedChannel`（`Arc<Vec<_>>` 共享，不再每请求克隆全量渠道/重复解析 `protocol_configs`）；SSE 解析器改为按偏移扫描、每 chunk 只压缩一次，并限制单帧缓冲 8MB；Chat 帧的 usage/首内容判断改为先做廉价子串检查，Anthropic/Responses 不再为判断首内容二次解析 JSON；请求体只在仍有后续候选时克隆。
- **一致性**：Responses 超过 10MB 未落库时返回 `store:false`；优雅退出时关闭并等待用量写队列排空。

验证：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked` 通过。

本轮未执行的安全/性能/兼容性事项整理到优化清单（原文件已归档至实施计划 §16）。

## 优化计划批次 1–5（2026-10-28）

按优化清单（原文件已归档至实施计划 §16）的顺序完成批次 1–5（含 P3-7），未修改已发布迁移。

- **批次 1（安全）**：登录对不存在的用户执行一次 dummy Argon2 校验以抹平时序差；改密复用登录限速（key 为 `password|{ip}|{admin_id}`）；设置更新改为先校验、再单事务写入；CSRF 的 `X-Forwarded-Host` 取最后一个非空值；长度 ≤4 的短 Key 掩码只返回 `••••`。
- **批次 2（可用性与协议）**：上游 Client 的 3xx 只跟随同 host、最多 3 次；请求入口引入总预算（非流式 600s、流式提交前 60s），候选循环与首帧等待共用绝对 deadline；`/v1/*` 改用 `ApiJson`，解析失败返回各协议 JSON 错误；Chat 流式注入 `include_usage` 被上游 400 拒绝时去掉字段重试一次。
- **批次 3（数据库）**：日志 `offset` 上限 100000；`include_total=false` 时跳过 `COUNT(*)`，前端只在第一页请求总数并缓存；`/api/site` 与通配渠道最近模型并入 30s `RuntimeCache`；新增 `ONEAPI_DB_POOL_SIZE`（1–64，默认 5）；`/readyz` 返回连接池大小/空闲连接与用量队列深度、同步回退、失败计数。
- **批次 4（性能）**：同优先级渠道改为 Efraimidis–Spirakis 单次加权排列（O(n log n)，替代逐轮求和 + `Vec::remove`）；`LiveMetrics` 按 16 分片拆分全局与渠道窗口，降低每 chunk 的锁竞争并保持“上一完整秒”语义；SSE 解析器按字节处理行，只有 `event`/`data` 才构造 String。
- **批次 5（前端与响应头）**：`readSse` 增量处理 LF/CRLF/裸 CR，结束时冲刷最后一帧；Playground 使用 `AbortController` 在重发与卸载时取消流并忽略中止错误；Keys 的 Token 上限改为 `Number.isSafeInteger`；所有响应增加 `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY`、`Referrer-Policy: no-referrer` 与 CSP；安装向导使用 `admin::create` 返回的 id。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（152 个测试）与 `web/` `npm run build` 均通过；`npm run lint` 为 0 error（仅 4 条既有 fast-refresh warning）。
- 响应头与前端加载实测：以 `127.0.0.1:3110` 启动 debug 二进制，`/` 返回 4 个安全响应头；`index.html` 只引用 `/assets/*` 外部模块与样式（无内联脚本），5 个资源均返回 200；`/readyz` 返回 `database_pool_size=7` 与队列指标。当前环境没有 Playwright/浏览器驱动，控制台级 CSP 验证未执行。
- 未纳入本轮：P1-6（密钥派生 v2）、P1-8（流式 Responses `store`）、P2-6（失败缓存抖动）、P2-8（大响应多次缓冲）、P3-8（更新信任边界），按计划需先出方案再排期。

## 复审性能与稳定性修复（2026-10-28，未发布）

基于 v0.1.10 的第二次全仓审查，按“先稳定性、再热路径分配、最后数据库压力”的顺序完成：

- **SSE 单帧上限修正**：`SseParser::buffered_len` 现在计入未完成帧已收到的完整行（`raw`+`buffer`）。此前持续发送 `data:` 行但不发空行可绕过 8MB 单帧上限，导致内存无界增长。解析器同时支持裸 CR 行尾（跨 chunk 的 CR 保留到下一 chunk 再判定 CRLF）。
- **流式透传减少解析与拷贝**：Responses 透传只对 `response.created/in_progress/completed/incomplete/failed` 等可能携带完整 response 对象的帧做 `id`/`store` 改写，`*.delta`/`*.added`/`*.done` 原字节转发；透传帧不再复制 `raw`（`Bytes::from(frame.raw)` 移动所有权）。
- **成功路径不再无谓写库**：新增 `Channel::has_failure_state`、`RuntimeCache::channel_has_failure_override` 与 `AppState::mark_channel_success_if_needed`。只有半开探测、健康覆盖层有记录或缓存行仍带失败字段时才执行 `UPDATE channels`；健康渠道不再为 0 行更新获取 SQLite 单写者锁。
- **`count_tokens` 资源复用**：改用 `state.clients` 的按渠道 Client（复用连接池/TLS）、`state.cache.defaults` 与 `read_body_limited_bytes`，不再每次请求新建 Client 或无界读取响应体。
- **同协议响应只解析 usage**：新增 `response_usage`，同协议成功响应不再为取 token 计数而把整个响应转换成 IR。
- **请求体按需克隆**：Responses 在 `store=false`（或 Playground）时不再克隆整个请求体；`client_request` 仅在确定要落库时生成。
- **模型与选路**：`/v1/models` 的 Key 渠道白名单只解析一次；`select::candidates` 的白名单改用 `HashSet`。
- **缓存与静态资源**：`RuntimeCache::channels` 增加 single-flight，TTL 到期时仅一个任务回源；内嵌前端资源改为借用 `Cow`，不再每次请求复制整个文件。
- **仪表盘 1h 时序**：桶大小从 3600s 改为 60s，避免只画出 1–2 个点。

验证：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（168 个测试）与 `web/` 下 `npm run build` 均通过。版本号未变更，改动基于 `v0.1.10`。

## 复审追加项（2026-10-28，未发布）

在上述复审基础上，继续完成当时列为“有意保留/低优先级”的项目：

- **Responses 历史改为增量存储**：`responses.request_json` 现在保存客户端本轮请求（保留 `previous_response_id`），`expand_previous` 沿链回溯重建完整输入；旧版已展开的历史行没有 `previous_response_id`，会自然终止回溯并继续可用。新增 256 链、2000 项、16MB 三个上限，避免一次请求产生无界数据库工作。
- **请求详情并入写队列**：`request_details` 与用量行共用 `UsageWriter` 的有界队列和批量事务，开启正文记录时不再为每个请求额外等待一次写事务；队列满或退出阶段回退同步写。
- **Key 掩码缓存**：`Crypto::mask` 按密文缓存结果（密文内容寻址，上限 256 项），渠道列表不再逐协议解密。
- **透传流式轻量观测**：`observe_passthrough` 每帧只解析一次并统计字符数，不再构建 IR 事件或复制 delta 字符串；Responses 终帧仍交给状态机以保留落库对象。
- **SSE 帧改写保真**：改写 `id`/`store` 时保留 `id:`、`retry:` 与注释行。
- **选路复用**：`RuntimeCache` 的元素改为 `Arc<PreparedChannel>`，`Candidate` 持有 Arc 与配置索引，不再逐候选克隆渠道行和协议配置；`last_channel` 只记录 id。
- **启动顺序**：`.env` 在 tokio 运行时启动前加载，`Config::from_env` 不再调用 `env::set_var`。
- **登录限速设置缓存**：`client_ip_header` 并入 `RuntimeCache`（30 秒，设置更新后失效），登录/改密不再每次查设置表。

验证：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（177 个测试）与 `web/` 下 `npm run build` 均通过。审查阶段列出的问题至此全部处理完毕，剩余事项只有 P4 路线图。

## 同模型渠道无缝故障转移（2026-10-29，v0.1.12）

按“只要还有同模型渠道可用，就不把上游错误直接抛给客户端”的目标调整网关失败处理：

- **所有上游失败都切换渠道**：非流式与流式提交前不再区分“可重试状态码”，400/402/404/409/413/422/429/5xx、请求构造失败、连接失败、读取超时、空响应体、非 JSON 响应、响应解析/编码失败都会继续尝试下一个同模型候选；只有候选耗尽后才向客户端返回错误。
- **2xx 内的错误也识别**：兼容部分上游用 200 返回 `{"error": ...}`（如 Responses 的 `status=failed`）的行为；空响应体同样按失败处理并切换。
- **渠道冷却与故障转移解耦**：新增 `penalizes_channel`，仅 401/402/403/404/408/409/425/429/5xx（及非 4xx 异常状态）计入渠道失败与指数退避；400/413/422 等请求语义错误会切换渠道但不会把健康渠道打入冷却。
- **全部失败时的错误语义**：优先返回第一个请求语义错误（例如 400）及其原本状态码，而不是统一 502；纯渠道故障仍返回 502。流式同样适用。
- **冷却期错误区分**：所有同模型渠道都在冷却时返回 503 `channel_recovering`（“retry shortly”），不再误报 404 “no channel available for this model”；确实没有该模型渠道时才返回 404。
- **`count_tokens` 同步支持**：多个 Anthropic 原生候选依次尝试，成功即返回；全部失败时优先返回请求语义错误，不再只试第一个渠道。
- **流式边界**：已经向客户端写出首帧后无法回退，中途错误仍按原逻辑记录；`stream::start` 在首帧前发现的上游错误事件会继续切换渠道。

验证：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（184 个测试）通过；新增/更新覆盖 400 切换、全部 400 返回 400、2xx 非 JSON 切换、流式 400 切换与全部失败、冷却期 503、渠道惩罚分类与字符串错误体解析、`count_tokens` 渠道切换。版本号未变更。

## 渠道冷却主动恢复（2026-10-29，v0.1.12）

冷却不再只等客户端流量触发半开探测，后台每分钟主动检查一次：

- 新增 `src/recovery.rs`：`main` 启动时 `recovery::spawn(state)`，每 60 秒扫描有失败记录且处于冷却的渠道。
- 探测方式：优先取渠道原生协议中第一个具体模型，发一次最小补全请求（Chat `max_tokens=1`、Anthropic 16、Responses 16 且 `store=false`）；只有 `*` 模型时改请求 `/models`。同一渠道有多个原生协议时任一成功即视为恢复。
- 资源边界：单请求 10 秒超时，最多 8 路并发；探测占用 `ChannelProbes` 的同一闸门，与客户端半开探测互斥，不会同时打同一渠道。
- 结果处理：成功调用 `mark_channel_success` 清零 `consecutive_failures`/`disabled_until` 并失效渠道缓存，渠道立即重新参与选路；失败只更新 `last_error` 与 `last_error_at`，不增加失败计数，正在运行的指数退避不被缩短，冷却已到期则顺延 60 秒。
- 复用抽取：`upstream::build_client_with_timeout`、`upstream::probe_model`、`upstream::probe_body` 同时供后台恢复与后台“探活”接口使用，测试请求体只有一份定义。

验证：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（189 个测试）通过；新增覆盖恢复成功重新上线、恢复失败保持冷却且不增加失败计数、`defer_recovery` 不缩短既有退避、探测请求体协议形状。版本号未变更。

## 个人轻量运维与每日额度（2026-10-29，v0.1.13）

- **启动自检**：`db::connect` 在打开连接池后执行 `PRAGMA quick_check`，失败时拒绝启动并提示从 `data/backups` 恢复。
- **迁移前快照**：检测到 `_sqlx_migrations` 中存在未应用版本时，先 `VACUUM INTO data/backups/oneapi-<UTC 时间>.db`，再执行迁移；目录保留最近 5 份 `*.db`。
- **导出/导入**：`GET /api/backup/export` 生成一致快照并以附件下载；`POST /api/backup/import` 限制 256MB，校验 SQLite 头与 `quick_check`，写入 `<db>.import` 后先对当前库做一次 `pre-import` 快照，500ms 后自动重启；新进程在连接前把 `.import` 换入并清理旧 WAL/SHM。设置页新增“数据库备份”卡片。
- **故障转移预算**：`MAX_FAILOVER_ATTEMPTS=3`、`FAILOVER_BUDGET=60s`，仅在准备尝试下一个渠道时检查；相同 `status + error_type` 的请求语义错误重复出现时立即停止。`count_tokens` 同样最多尝试 3 个 Anthropic 候选。
- **运行状态**：`/api/system` 返回 `database_bytes`、`wal_bytes`、`last_backup_at`、`usage_queue_depth`、`cooling_channels`、`failovers_24h`；仪表盘“系统信息”卡片展示。
- **每日额度**：`api_keys` 新增 `daily_token_limit/daily_token_usage/daily_usage_day`（迁移 `0008_api_key_daily_limit.sql`）。鉴权在累计限额检查之后判断当日用量，超限返回 429 `rate_limit_error` 且不永久停用；用量写入时按 UTC 日序号懒重置。Key 页面新增“每日 Token 上限”，列表显示“今日 x / y”。

验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（196 个测试）与 `web/` `npm run build` 通过；本地二进制实测 `/api/setup` → `/api/backup/export` → `/api/backup/import` → 自动重启，导入后同一会话仍可访问 `/api/system`，`data/backups` 中存在 pre-import 快照。

## 跨协议推理输出修复（2026-10-29，v0.1.14）

- 问题：Anthropic 客户端在 Playground 或 `/v1/messages` 中使用 Chat 上游时，上游先输出 `reasoning_content`，但 Anthropic 编码器把推理全部丢弃，客户端只看到正文；首字延迟等于第一个正文 token。
- 修复：Anthropic 流式编码器输出 `thinking` 块与 `thinking_delta`，不再要求上游签名；非流式 `encode_response` 同样在无签名时输出 thinking block，有签名时继续保留 `signature`。
- 签名传播：流式 IR 新增 `ReasoningSignature`，Anthropic 解码器捕获 `signature_delta`；转回 Anthropic 时转发 `signature_delta`，转 Responses 时写入 reasoning item 的 `encrypted_content`。Chat 没有签名语义，忽略该事件。
- 块顺序：Anthropic 编码器在开始新内容块前先关闭仍在打开的块，避免 reasoning 与 text 块交叠，符合 Anthropic 顺序流。
- 覆盖范围：Anthropic 客户端 + Chat 上游（流式/非流式）、Anthropic 客户端 + Anthropic 上游、Responses 客户端 + Chat/Anthropic 上游；Playground 与普通 API 走同一路径，问题已一并修复。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --locked`（当时 200 个测试）通过；真实二进制 + mock 上游冒烟确认 Chat→Anthropic 现在先输出 thinking 再输出正文，非流式 content 为 `['thinking','text']`，Responses 方向保持正常。随后在 v0.1.14 中按使用要求移除了全部测试代码。

## 移除测试代码（2026-10-29，v0.1.14）

- 按使用要求删除全部 `#[cfg(test)]` 模块、`src/admin/tests.rs`、test-only helper（`ClientCache::len`、`catalog_models`、`api_key::create`）。
- 移除 dev-dependencies `tempfile`、`tower`、`wiremock`；CI 去掉 `cargo test` 步骤，Makefile 去掉 `test` 目标。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo build` 通过；`cargo test --locked` 可执行但为 0 个测试。
- 说明：`#[cfg(test)]` 代码原本不会编译进 release 二进制；本次清理主要减少源码与开发依赖面。后续行为验证使用仓库外的临时冒烟脚本。

## AOCI 状态

- 版本：`0.1.0-rc22`，二进制 `/usr/local/bin/aoci`。
- ZCode workspace MCP 已配置：`.zcode/config.json`（该文件为机器相关路径，已在 `.git/info/exclude` 中排除）。
- 已完成 `init --locale zh-CN` 与 `scan`。
- 首次完整索引（M0）已完成；M1–M7 持续做增量维护，当前索引覆盖后端、前端与文档受管理文件。
- 每个里程碑结束调用 `aoci_maintain` 做增量维护，完成后执行 verify/check/guide。

## 下一步

1. 审查计划阶段 A–D 与 E3 已合并回 `main` 并发布 `v0.1.2`；`v0.1.3` 为日志筛选修复补丁。
2. 阶段 E 剩余项（速率限制与额度、Prometheus `/metrics`、日志导出/重放、备份恢复、Playground 增强、多管理员/RBAC 与 PostgreSQL）仍是路线图。
3. 后续版本按 `Cargo.toml` 版本打标签，保留数据目录升级。
4. 审查计划与优化清单的可执行项已全部完成，两份计划文档已删除；后续能力与已知限制见 `docs/IMPLEMENTATION_PLAN.md` §16。

## 交接约定

- 新会话开始先读 `AGENTS.md`、本文档和 `docs/IMPLEMENTATION_PLAN.md`。
- 每个里程碑结束更新本文档；计划或决策变化同步更新 `docs/IMPLEMENTATION_PLAN.md`。
- 不提交 AOCI 的机器运行时资产；`aoci.txt`、`aoci.meta.txt`、`aoci.code.txt`、`.aoci/config.json`、`.aoci/baseline.json`、`AGENTS.md`、`.gitattributes` 属于应纳入版本管理的认知资产。
