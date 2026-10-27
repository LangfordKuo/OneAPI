# OneAPI

统一管理并调用你的所有 AI。

OneAPI 是一个自托管的 AI API 网关：在后台配置上游渠道，客户端用统一的 API Key 调用；入站支持 Chat Completions、Responses、Anthropic Messages 三种协议，可跨协议转换到任意一种上游协议，支持 SSE 流式、多渠道优先级/权重路由和故障转移。前端构建为静态资源并内嵌进 Rust 二进制，最终以单文件运行。

## 当前进度

M0–M6 已完成：脚手架、管理基础、三协议非流式/流式网关、Responses 本地状态、请求日志、仪表盘、Playground 与多架构发布流水线。M7 继续补齐保留期清理、请求体记录开关与最终验收。详细进度见 `docs/PROGRESS.md`。

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

同时构建并推送 `linux/amd64`、`linux/arm64` 多架构 Docker 镜像到 GitHub Container Registry。镜像直接使用同一次构建产出的 Linux 二进制（`Dockerfile.release`），不会在 QEMU 下二次编译；本地源码构建仍使用 `Dockerfile`。

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

## 配置

复制 `.env.example` 按需修改，或直接使用环境变量：

- `ONEAPI_ADDR`：监听地址，默认 `0.0.0.0:3000`
- `ONEAPI_DATA_DIR`：数据目录，默认 `./data`
- `ONEAPI_DATABASE_URL`：可选，显式 SQLite URL
- `ONEAPI_LOG_LEVEL`：日志级别，默认 `info`
- `ONEAPI_SECRET`：上游 Key 加密主密钥；未设置时在数据目录生成
- `ONEAPI_ADMIN_USER` / `ONEAPI_ADMIN_PASSWORD`：首次启动的管理员引导账号
- `ONEAPI_COOKIE_SECURE`：HTTPS 部署时设为 `true`

健康检查：`GET /healthz`、`GET /readyz`。

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

## 文档

- `docs/IMPLEMENTATION_PLAN.md`：范围、架构、数据模型、协议映射、里程碑与风险
- `docs/PROGRESS.md`：当前进度与交接约定

## License

GPL-3.0
