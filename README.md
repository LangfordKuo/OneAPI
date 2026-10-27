# OneAPI

统一管理并调用你的所有 AI。

OneAPI 是一个自托管的 AI API 网关：在后台配置上游渠道，客户端用统一的 API Key 调用；入站支持 Chat Completions、Responses、Anthropic Messages 三种协议，可跨协议转换到任意一种上游协议，支持 SSE 流式、多渠道优先级/权重路由和故障转移。前端构建为静态资源并内嵌进 Rust 二进制，最终以单文件运行。

## 当前进度

M0–M7 已完成：脚手架、管理基础、三协议非流式/流式网关、Responses 本地状态、请求日志、仪表盘、Playground、Web 安装向导、CI/CD 与多架构发布、保留期清理、`reset-password` 与请求体记录。详细进度见 `docs/PROGRESS.md`。

## 支持的平台

推送 `v*` 标签时，GitHub Actions 会自动构建并发布以下平台的二进制压缩包：

| 平台 | 架构 | Runner | 产物 |
|---|---|---|---|
| Linux | amd64 | `ubuntu-latest` | `oneapi-<tag>-linux-amd64.tar.gz` |
| Linux | arm64 | `ubuntu-24.04-arm` | `oneapi-<tag>-linux-arm64.tar.gz` |
| Windows | amd64 | `windows-latest` | `oneapi-<tag>-windows-amd64.zip` |
| Windows | arm64 | `windows-11-arm` | `oneapi-<tag>-windows-arm64.zip` |
| macOS | amd64 | `macos-15-intel` | `oneapi-<tag>-macos-amd64.tar.gz` |
| macOS | arm64 | `macos-14` | `oneapi-<tag>-macos-arm64.tar.gz` |

同时构建并推送 `linux/amd64`、`linux/arm64` 多架构 Docker 镜像到 GitHub Container Registry。Linux 产物使用 **musl 静态链接**（`x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`），不依赖 glibc，可在 Debian 11、Ubuntu 20.04、Alpine 等较老或精简系统运行。镜像直接使用同一次构建产出的 Linux 二进制（`Dockerfile.release`），不会在 QEMU 下二次编译；本地源码构建仍使用 `Dockerfile`。

Actions artifact 上传的是原始二进制，Release 任务只打包一次为最终 `tar.gz`/`zip`，不会再出现“压缩包里套压缩包”。

发布产物未做代码签名：macOS 首次运行可能需要执行 `xattr -dr com.apple.quarantine ./oneapi`，Windows 可能出现 SmartScreen 提示。

### 计划中的 M7 剩余项

- 日志与 Responses 保留期清理任务；
- `reset-password` 子命令；
- 请求/响应正文记录开关落地；
- 最终部署与验收文档。

## 技术栈

- 后端：Rust、Axum、SQLite（sqlx）、reqwest、rustls
- 前端：React 18、TypeScript、Ant Design 5、Vite 7、TanStack Query
- 单文件：rust-embed 将 `web/dist` 嵌入 release 二进制

## 开发

需要 Rust stable 和 Node.js 20.19+。本仓库环境中的 Node 位于 `/root/.local/share/vite-plus/bin`，Makefile 会自动把它加入 PATH。

```bash
# 终端 1：后端
cargo run -- serve

# 终端 2：前端开发服务器（代理 /api、/v1 到 127.0.0.1:3000）
cd web && npm install && npm run dev
```

## 构建

```bash
make build
./target/release/oneapi serve
```

`make build` 会先构建前端到 `web/dist`，再编译 release 二进制。构建完成后运行不再依赖 `web/dist`。

也可以分步执行：

```bash
cd web && npm install && npm run build
cargo build --release
```

## 首次运行

首次启动不需要预先创建 `.env` 或数据库：

```bash
# 可选：指定数据目录；不指定时使用 ./data
ONEAPI_DATA_DIR=./data ./oneapi serve
```

1. 打开 `http://localhost:3000`，如果还没有管理员，会进入 **安装页**，不会显示登录页。
2. 在安装页填写管理员用户名、密码和站点名称。
3. 提交后程序会：
   - 创建管理员（Argon2id 哈希存入 SQLite）；
   - 在数据目录写入 `.env`（Unix 下权限 `0600`）；
   - 自动登录并进入后台。
4. SQLite 数据库 `oneapi.db` 和加密主密钥 `master.key` 会在启动时自动创建；数据目录里的 `.env` 会随数据库一起持久化。

安装页会显示当前的监听地址、数据目录、SQLite 路径和 `.env` 路径。监听地址与数据目录仍由启动环境变量/默认值决定；需要修改时，编辑 `.env` 或设置环境变量后重启。

安装完成前请勿将端口直接暴露到公网；如需远程安装，建议先通过防火墙或反向代理限制访问。

如果希望无界面自动初始化（Docker/服务端部署），设置 `ONEAPI_ADMIN_USER` 和 `ONEAPI_ADMIN_PASSWORD` 后启动，程序会直接创建管理员并跳过安装页。

后续升级只需替换二进制并重启，保留同一个数据目录（`oneapi.db`、`master.key`、`.env`）；数据库迁移会在启动时自动执行。

## 配置

`.env` 会从当前工作目录和数据目录两处加载，且不会覆盖已经存在的进程环境变量。安装向导生成的 `.env` 位于数据目录中。

也可以直接使用环境变量：

- `ONEAPI_ADDR`：监听地址，默认 `0.0.0.0:3000`
- `ONEAPI_DATA_DIR`：数据目录，默认 `./data`
- `ONEAPI_DATABASE_URL`：可选，显式 SQLite URL
- `ONEAPI_LOG_LEVEL`：日志级别，默认 `info`
- `ONEAPI_SECRET`：上游 Key 加密主密钥；未设置时在数据目录生成
- `ONEAPI_ADMIN_USER` / `ONEAPI_ADMIN_PASSWORD`：无界面首次启动的管理员引导账号；未设置密码时使用 Web 安装页
- `ONEAPI_COOKIE_SECURE`：HTTPS 部署时设为 `true`

健康检查：`GET /healthz`、`GET /readyz`。

## 实时 tok/s

- 渠道页每行显示该渠道最近 10 秒的输出 tok/s；主页顶部显示全站实时输出 tok/s。
- 通过 `GET /api/metrics/stream`（SSE）每秒推送，页面使用 EventSource 自动订阅，不需要刷新。
- 非流式请求在完成时计入输出 tokens；流式请求按增量文本估算，并在使用量事件到达后用实际 completion tokens 校准。
- 指标为最近 10 秒滑动窗口，空闲时自动回落到 0。

## 在线更新

后台“设置 → 软件更新”支持：

- **更新源**：GitHub 官方，或国内加速 `https://gh-proxy.com/`（元数据与安装包都走代理）。
- **检查更新**：显示当前版本、最新版本、发布时间、安装包和更新说明。
- **一键更新并重启**：下载对应平台的 Release 资产，校验 `SHA256SUMS`，替换当前二进制并自动重启。

说明：

- 更新只接受 GitHub Release 中符合 `oneapi-<tag>-<platform>` 命名的官方资产，并强制校验 SHA256。
- 信任模型：更新信任 GitHub Release 账号与 HTTPS 传输，安装包未做代码签名；如需更强保证，请自行下载并校验后再替换二进制。
- Linux/macOS 使用 `exec` 原进程重启，Windows 在服务退出后启动新进程。
- 容器部署不建议自更新，程序会拒绝并提示拉取新镜像；可用 `ONEAPI_ALLOW_SELF_UPDATE=1` 强制测试。
- Windows 更新需要程序所在目录可写；运行目录外的数据目录（SQLite、master.key、.env）不会被修改。

## 自动发布

打标签前确保 `Cargo.toml` 的 `version` 与标签一致（例如 `version = "0.1.0"` 对应 `v0.1.0`）：

```bash
git tag v0.1.0
git push origin v0.1.0
```

标签推送后 `.github/workflows/release.yml` 会：

1. 校验标签与 `Cargo.toml` 版本一致；
2. 构建前端静态资源；
3. 在六个原生 runner 上构建 release 二进制并打包（附 `README.md`、`LICENSE`、`.env.example`）；
4. 构建并推送多架构镜像到 `ghcr.io/langfordkuo/oneapi`；
5. 创建 GitHub Release，附六个平台产物与 `SHA256SUMS`。

Docker 镜像标签包括 `latest`、`0.1.0`、`0.1`、`v0.1.0` 与 `sha-<commit>`。首次发布后，如 GHCR 包默认为私有，可在 GitHub Packages 设置中改为 public。

`push` 到 `main` 或提交 PR 时，`.github/workflows/ci.yml` 会运行 Rust 格式检查、Clippy、测试与前端构建。

## Docker

```bash
docker pull ghcr.io/langfordkuo/oneapi:latest

docker run -d --name oneapi \
  -p 3000:3000 \
  -v oneapi-data:/data \
  -e ONEAPI_ADMIN_USER=admin \
  -e ONEAPI_ADMIN_PASSWORD='change-me-please' \
  ghcr.io/langfordkuo/oneapi:latest
```

也可以使用仓库内 Compose 从源码构建：

```bash
docker compose up --build
```

数据持久化在 `./data`，服务监听 `3000`。

## 维护

- **保留期清理**：服务每 6 小时自动清理一次。用量日志和请求/响应正文按 `usage_log_retention_days`，Responses 按 `response_retention_days`；设置为 `0` 表示永久保留。
- **重置管理员密码**：

```bash
./oneapi reset-password --data-dir ./data --password 'new-strong-password'
# 或不带 --password，从标准输入读取
./oneapi reset-password --data-dir ./data
```

重置后会注销该管理员的全部会话。

- **请求/响应正文记录**：默认关闭；在设置页开启后，日志详情可查看请求与响应正文，单条最多保存 64KB。

## 文档

- `docs/IMPLEMENTATION_PLAN.md`：范围、架构、数据模型、协议映射、里程碑与风险
- `docs/PROGRESS.md`：当前进度与交接约定

## License

GPL-3.0
