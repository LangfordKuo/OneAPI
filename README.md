# OneAPI

统一管理并调用你的所有 AI。

OneAPI 是一个自托管的 AI API 网关：在后台配置上游渠道，客户端用统一的 API Key 调用；入站支持 Chat Completions、Responses、Anthropic Messages 三种协议，可跨协议转换到任意一种上游协议，支持 SSE 流式、多渠道优先级/权重路由和故障转移。前端构建为静态资源并内嵌进 Rust 二进制，最终以单文件运行。

## 当前进度

M0 脚手架已完成：Rust + Axum 后端、Vite + React + Ant Design 前端、SQLite 迁移、静态资源嵌入、健康检查、Makefile 与 Dockerfile。业务 API 与后台管理功能按 `docs/IMPLEMENTATION_PLAN.md` 的 M1–M7 推进。

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

## Docker

```bash
docker compose up --build
```

数据持久化在 `./data`，服务监听 `3000`。

## 文档

- `docs/IMPLEMENTATION_PLAN.md`：范围、架构、数据模型、协议映射、里程碑与风险
- `docs/PROGRESS.md`：当前进度与交接约定

## License

GPL-3.0
