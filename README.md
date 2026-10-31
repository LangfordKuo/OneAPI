# OneAPI

统一管理并调用你的所有 AI。

OneAPI 是一个自托管的 AI API 网关。在后台配置上游渠道后，客户端只需使用 OneAPI 签发的 API Key 发起请求；网关负责选择渠道、转发与记录用量，并在上游异常时自动切换。

## 功能

- **三协议接入**：同时提供 Chat Completions、Responses、Anthropic Messages 接口，并支持三种协议之间的跨协议转换。
- **流式响应**：支持 SSE 流式输出；跨协议时实时转换事件格式。
- **推理输出**：跨协议转换时保留 reasoning/thinking；Anthropic 客户端也能看到 Chat 上游的推理内容。没有上游签名的 thinking 只用于展示，回放时会被网关安全丢弃。
- **多渠道调度**：同一模型可配置多个渠道，按优先级分组、组内按权重随机；上游失败时自动切换到同模型的其他渠道，单次请求最多尝试 3 个渠道且最多切换 60 秒，只有全部候选都失败才向客户端返回错误。
- **多协议渠道**：单个渠道可同时声明 Chat Completions、Responses 与 Anthropic 上游协议；入站协议匹配时原样转发，不匹配时按渠道配置的转换目标转换。
- **渠道健康**：只有渠道侧失败（鉴权、额度、限流、超时、5xx 等）才累计指数退避冷却；请求语义错误会切换渠道但不会冷却渠道。冷却期间每 60 秒主动探测一次，恢复后自动清除冷却并重新上线；冷却到期仍只放行单个客户端探测请求。
- **管理后台**：渠道、API Key（可限制可用渠道/模型、累计 Token 与每日 Token）、请求日志（含每次请求的 tok/s）、用量仪表盘（含全站实时 tok/s 与并行会话数）、Playground、账户设置（用户名与密码）、系统设置、软件更新与数据库备份均可在浏览器中完成。
- **轻量运维**：启动时校验数据库完整性，迁移前自动快照到 `data/backups`；整个数据目录可以直接拷走迁移。
- **单文件运行**：前端静态资源内嵌在 Rust 二进制中，无需单独的 Web 服务器；数据存储使用 SQLite。

## 部署

运行环境只需要一个可写的数据目录。首次启动会自动创建 SQLite 数据库、加密主密钥和配置文件。

### Docker

```bash
docker pull ghcr.io/langfordkuo/oneapi:latest

docker run -d --name oneapi \
  -p 3000:3000 \
  -v oneapi-data:/data \
  -e ONEAPI_ADMIN_USER=admin \
  -e ONEAPI_ADMIN_PASSWORD='change-me-please' \
  ghcr.io/langfordkuo/oneapi:latest
```

启动后访问 `http://<主机>:3000`，使用上面的管理员账号登录。

如果省略 `ONEAPI_ADMIN_USER` / `ONEAPI_ADMIN_PASSWORD`，首次访问会进入安装页，由页面创建管理员。

### Docker Compose

```bash
git clone https://github.com/LangfordKuo/OneAPI.git
cd OneAPI
docker compose up -d --build
```

数据保存在 `./data`，服务监听 `3000`。部署前请修改 `docker-compose.yml` 中的管理员密码和 `ONEAPI_SECRET`。

### 二进制

从 [Releases](https://github.com/LangfordKuo/OneAPI/releases) 下载对应平台的压缩包并解压：

```bash
tar xzf oneapi-v0.1.15-linux-amd64.tar.gz
ONEAPI_DATA_DIR=./data ./oneapi serve
```

支持 Linux、macOS 和 Windows 的 amd64/arm64。Linux 产物为 musl 静态链接，不依赖系统 glibc。macOS 首次运行如被 Gatekeeper 拦截，可执行 `xattr -dr com.apple.quarantine ./oneapi`。

### 首次初始化

默认情况下，首次启动后打开 `http://localhost:3000` 会显示安装页：

1. 填写管理员用户名、密码和站点名称；
2. 程序创建管理员，并在数据目录写入 `.env`（Unix 下权限 `0600`）；
3. 自动登录进入后台。

无界面部署可改为设置 `ONEAPI_ADMIN_USER` 和 `ONEAPI_ADMIN_PASSWORD`，启动时直接创建管理员并跳过安装页。

安装完成前不要把端口直接暴露到公网；需要远程安装时，先通过防火墙或反向代理限制访问。

### 配置

配置可通过环境变量或数据目录中的 `.env` 提供；进程环境变量优先。

| 变量 | 说明 | 默认值 |
|---|---|---|
| `ONEAPI_ADDR` | 监听地址 | `0.0.0.0:3000` |
| `ONEAPI_DATA_DIR` | 数据目录 | `./data` |
| `ONEAPI_SECRET` | 上游 Key 加密主密钥；未设置时自动生成 `master.key` | 自动生成 |
| `ONEAPI_ADMIN_USER` | 无界面初始化的管理员用户名 | 未设置 |
| `ONEAPI_ADMIN_PASSWORD` | 无界面初始化的管理员密码 | 未设置 |
| `ONEAPI_COOKIE_SECURE` | HTTPS 部署时设为 `true` | `false` |
| `ONEAPI_TRUST_PROXY` | 位于受信反向代理后时设为 `true`，CSRF 校验使用 `X-Forwarded-Host` | `false` |
| `ONEAPI_LOG_LEVEL` | 日志级别 | `info` |
| `ONEAPI_LOG_JSON` | 设为 `1` 输出 JSON 日志 | 关闭 |
| `ONEAPI_DATABASE_URL` | 显式指定 SQLite URL | 数据目录内的 `oneapi.db` |
| `ONEAPI_DB_POOL_SIZE` | SQLite 连接池上限（1–64） | `5` |
| `ONEAPI_UPDATE_API_BASE` | 覆盖在线更新元数据地址，必须为 HTTPS | GitHub API |

管理后台的「设置」页可以指定从哪个请求头读取真实客户端 IP，例如 Cloudflare 的 `CF-Connecting-IP`、反向代理的 `X-Forwarded-For` 或 `X-Real-IP`。该地址用于登录失败限速。留空时默认读取 `X-Forwarded-For`，但只对来自回环或内网地址（同机反代、Docker/Kubernetes 网络、Cloudflare Tunnel）的请求生效；显式填写任意请求头后视为管理员已确认可信，会直接采用，不再看对端地址——这适用于 CDN 从公网地址直接回源 `3000` 端口的部署。显式配置时请确保源站端口只接受 CDN/反向代理访问，否则直接访问的客户端可以伪造该头绕过登录限速。头缺失或值无法解析时仍回退 TCP 对端地址。

同一页面的「网站域名」用于反向代理场景：填写浏览器实际访问的域名（如 `https://api.example.com` 或 `api.example.com`）。当 Nginx 等反向代理没有把外部 Host 透传给后端时，写请求的 CSRF 校验会额外接受该域名；协议、路径与 80/443 默认端口会被忽略，留空则只校验请求 Host。

健康检查接口：`GET /healthz`、`GET /readyz`。

### 数据与升级

数据目录中的 `oneapi.db`、`master.key` 和 `.env` 需要持久化。升级时保留同一数据目录，替换镜像或二进制后重启即可，数据库迁移会在启动时自动执行。每次执行迁移前，OneAPI 会先把当前数据库快照到 `data/backups/`，并保留最近 5 份。

备份只发生在服务器端：设置页的「数据库备份」可以在 `data/backups/` 生成一份一致的 SQLite 快照并列出已有备份，快照不会通过 HTTP 下载，也不接受上传的数据库文件。这样即使管理员会话被盗，也无法把整库导走，或用一个构造过的库替换线上数据。需要恢复时，在服务器上停止服务，用目标备份覆盖数据库文件并删除同名的 `-wal`/`-shm` 后再启动。直接整体复制数据目录同样可以完成迁移或备份。

在线更新默认信任「GitHub 账号 + HTTPS」：只从本仓库的 latest Release 下载资产，要求同源 `SHA256SUMS` 精确匹配，所有请求和重定向都必须是 HTTPS。`ONEAPI_UPDATE_API_BASE` 可覆盖 Release 元数据地址（必须为 HTTPS），这等于把更新信任完全委托给该端点；当前版本没有二进制签名校验，对供应链安全要求更高的部署应关闭自动更新、固定镜像摘要或自行校验发布产物。容器内默认拒绝自更新，仅测试时可设置 `ONEAPI_ALLOW_SELF_UPDATE=1`。
