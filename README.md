# OneAPI

统一管理并调用你的所有 AI。

OneAPI 是一个自托管的 AI API 网关。在后台配置上游渠道后，客户端只需使用 OneAPI 签发的 API Key 发起请求；网关负责选择渠道、转发与记录用量，并在上游异常时自动切换。

## 功能

- **三协议接入**：同时提供 Chat Completions、Responses、Anthropic Messages 接口，并支持三种协议之间的跨协议转换。
- **流式响应**：支持 SSE 流式输出；跨协议时实时转换事件格式。
- **多渠道调度**：同一模型可配置多个渠道，按优先级分组、组内按权重随机；上游失败时自动故障转移。
- **多协议渠道**：单个渠道可同时声明 Chat Completions、Responses 与 Anthropic 上游协议；入站协议匹配时原样转发，不匹配时按渠道配置的转换目标转换。
- **渠道健康**：失败累计后指数退避冷却，冷却到期仅放行单个探测请求，探测成功自动恢复。
- **管理后台**：渠道、API Key、请求日志（含每次请求的 tok/s）、用量仪表盘、Playground 与系统设置均可在浏览器中完成。
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
tar xzf oneapi-v0.1.5-linux-amd64.tar.gz
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

管理后台的「设置」页可以指定从哪个请求头读取真实客户端 IP，例如 Cloudflare 的 `CF-Connecting-IP`、反向代理的 `X-Forwarded-For` 或 `X-Real-IP`。该地址用于登录失败限速；留空时使用 TCP 对端地址。只有确认所有流量都经过可信代理时才应配置，否则客户端可以伪造该请求头绕过限速。

健康检查接口：`GET /healthz`、`GET /readyz`。

### 数据与升级

数据目录中的 `oneapi.db`、`master.key` 和 `.env` 需要持久化。升级时保留同一数据目录，替换镜像或二进制后重启即可，数据库迁移会在启动时自动执行。
