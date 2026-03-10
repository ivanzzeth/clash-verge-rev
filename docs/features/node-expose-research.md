# 节点统一暴露为 HTTP/HTTPS/SOCKS5 调研报告

## 1. 需求概述

将 verge-cli 管理的底层节点（SS、HTTP、HTTPS、VMess、Trojan 等）**一一对应**地暴露为本地 HTTP/HTTPS/SOCKS5 代理，用于：

- **Web3 撸毛**：不同应用绑定不同代理端口，实现多 IP 并行
- **零额外成本**：复用现有订阅节点，无需购买专门代理服务

## 2. 当前架构

- **verge-cli** 管理订阅、规则、生成 mihomo 配置
- **mihomo** 提供单一 mixed-port（SOCKS5+HTTP），按规则选节点
- 订阅节点类型：`ss`、`http`、`https`、`vmess`、`trojan`、`vless`、`socks5` 等

## 3. 方案对比

### 3.1 方案 A：GOST（推荐）

**GOST** (GO Simple Tunnel) - Go 实现，6.4k stars，协议支持最全。

| 维度 | 评价 |
|------|------|
| 协议支持 | SS、HTTP、HTTPS、SOCKS5、VMess、Trojan、VLESS 等全覆盖 |
| 链式转发 | `-L socks5://:1080 -F ss://...` 或 `-F http://...` 一行搞定 |
| 多端口 | 支持 `-L socks5://:1080 -L http://:3128` 同时暴露 |
| 成熟度 | 生产级，文档完整 (gost.run) |
| 集成方式 | 作为子进程由 verge-cli 管理，或生成配置/脚本 |

**命令行示例**（每节点一个 GOST 进程）：

```bash
# SS 节点 -> 本地 SOCKS5 + HTTP
gost -L socks5://:10001 -L http://:10002 -F ss://method:password@server:port

# HTTP 节点 -> 本地 SOCKS5 + HTTP
gost -L socks5://:10011 -L http://:10012 -F http://user:pass@server:port

# HTTPS 节点
gost -L socks5://:10021 -L http://:10022 -F https://user:pass@server:port
```

**优点**：单二进制、协议全、配置简单、社区活跃  
**缺点**：Go 实现，非 Rust；需作为外部依赖或 sidecar

---

### 3.2 方案 B：Rust 生态组合

| 工具 | 能力 | 适用场景 |
|------|------|----------|
| **shadowsocks-rust** | SS 客户端，暴露 SOCKS5/HTTP | 仅 SS 节点 |
| **socks-hub** | HTTP/HTTPS 上游 -> 本地 SOCKS5 | HTTP/HTTPS 节点 |
| **sthp** | SOCKS5 -> HTTP | 需要 HTTP 输出时 |
| **tun2proxy** | TUN 设备到代理 | 透明代理，非本需求 |

**组合逻辑**：

- **SS 节点**：`sslocal -b 127.0.0.1:PORT --protocol socks` 或 `--protocol http`
- **HTTP/HTTPS 节点**：`socks-hub -l socks5://0.0.0.0:PORT -r http(s)://upstream`

**局限**：

- VMess、Trojan、VLESS 等需额外库或工具
- 多进程管理（每节点 1–2 个进程）
- socks-hub 参数需确认（listen vs remote 角色）

---

### 3.3 方案 C：多 mihomo 实例

每个节点生成一个只含单代理的 mihomo 配置，启动多个 mihomo 进程。

**缺点**：mihomo 较重，50 节点 = 50 进程，资源占用高，不推荐。

---

## 4. 推荐实现路径

### 4.1 首选：GOST 作为后端

1. **verge-cli 新增子命令** `expose`（已实现）：
   - `verge-cli expose list`：列出所有节点及对应端口
   - `verge-cli expose start [--base-port 10000] [--nodes NODE1,NODE2...]`：为节点启动 GOST 进程；`--nodes` 可指定要暴露的节点（默认全部）
   - `verge-cli expose stop`：停止所有 expose 进程
   - **自动安装 GOST**：检测到 GOST 未安装时，自动下载到 `~/.local/bin`（Linux/macOS，无需 root）

2. **Clash 代理格式 -> GOST 格式** 转换：
   - SS：`ss://base64(method:password)@server:port`（SIP002）
   - HTTP：`http://[user:pass@]server:port`
   - HTTPS：`https://[user:pass@]server:port`
   - SOCKS5：`socks5://[user:pass@]server:port`
   - VMess/Trojan：GOST 支持对应 URI，需从 Clash YAML 转成标准 URI

3. **端口分配**：`base_port + index * 2`（SOCKS5、HTTP 各一端口）或 `base_port + index * 3`（+HTTPS）

4. **进程管理**：verge-cli 用 `tokio::process` 管理 GOST 子进程，或写入 systemd user units / launchd plist

### 4.2 备选：Rust 原生（开发成本较高）

若坚持纯 Rust：

1. **SS**：`shadowsocks` crate，或直接 spawn `sslocal` 二进制
2. **HTTP/HTTPS**：`socks-hub` 作为库或子进程
3. **VMess/Trojan**：需评估 `v2ray-rust`、`trojan-rust` 等，或暂时只支持 SS/HTTP/HTTPS

---

## 5. Clash 代理类型与 GOST 对应

| Clash type | GOST -F 参数格式 | 说明 |
|------------|-----------------|------|
| ss | `ss://...` | SIP002 URI |
| http | `http://...` | 直连 |
| https | `https://...` | TLS 包装 |
| socks5 | `socks5://...` | 直连 |
| vmess | `vmess://...` | 需从 Clash 字段组装 |
| trojan | `trojan://...` | 需从 Clash 字段组装 |
| vless | `vless://...` | GOST 需确认支持 |

---

## 6. 依赖与安装

### GOST

```bash
# 安装
bash <(curl -fsSL https://github.com/go-gost/gost/raw/master/install.sh) --install

# 或下载 release
# https://github.com/go-gost/gost/releases
```

### Rust 工具（方案 B）

```bash
cargo install shadowsocks-rust  # sslocal
cargo install socks-hub
```

---

## 7. 输出示例（目标 UX）

```bash
$ verge-cli expose start --base-port 10000

Exposing 12 nodes:
  HK-01   (ss)    -> socks5://127.0.0.1:10001  http://127.0.0.1:10002
  HK-02   (ss)    -> socks5://127.0.0.1:10003  http://127.0.0.1:10004
  JP-01   (vmess) -> socks5://127.0.0.1:10005  http://127.0.0.1:10006
  US-01   (http)  -> socks5://127.0.0.1:10007  http://127.0.0.1:10008
  ...

$ verge-cli expose list
# 表格输出：节点名、类型、SOCKS5 端口、HTTP 端口、状态
```

---

## 8. 结论与建议

| 方案 | 开发成本 | 维护成本 | 协议覆盖 | 推荐度 |
|------|----------|----------|----------|--------|
| GOST 后端 | 低 | 低 | 全 | ⭐⭐⭐⭐⭐ |
| Rust 组合 | 中高 | 中 | SS+HTTP 为主 | ⭐⭐⭐ |
| 多 mihomo | 低 | 高（资源） | 全 | ⭐ |

**建议**：优先采用 **GOST 作为后端**，在 verge-cli 中实现 Clash 代理到 GOST 的转换与进程管理。若后续有强需求保持纯 Rust 栈，再考虑基于 shadowsocks-rust + socks-hub 的渐进式实现。
