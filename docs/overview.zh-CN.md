<div align="center">
  <h1>Koko (叩叩)</h1>
  <p><b>敲一下，就连上。</b></p>
  <p>用 Rust 打造的 Agent 原生 P2P 远程桌面 —— 同时面向<b>人类</b>与 <b>AI Agent</b>。</p>
  <p>
    <a href="./overview.md">English</a> ·
    <a href="./README.md">文档目录</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust" alt="Built with Rust" />
    <img src="https://img.shields.io/badge/Protocol-QUIC%20%2F%20P2P-0ea5e9" alt="QUIC P2P" />
    <img src="https://img.shields.io/badge/AI%20Interface-MCP%20%2F%20Computer%20Use-8b5cf6" alt="MCP Ready" />
    <img src="https://img.shields.io/badge/License-GPL--3.0-22c55e" alt="GPL-3.0" />
  </p>
</div>

> Phase 1 的 Rust 工作区骨架已就绪。本文介绍规划中的远程桌面功能和 v0.1 接口。

## 什么是 Koko？

**Koko（叩叩）** 源自敲门声 *"Knock, Knock"*。它是一个基于 Rust 构建的端到端加密 P2P 远程控制底座，从第一天起就为 **“人类 + AI Agent 协同操作真实设备（Human & Agent-to-Device）”** 而设计。

传统的远程桌面软件（向日葵、ToDesk、RustDesk）只面向人眼，提供的是视频流；而当前的 AI Computer Use 方案大多受限于云端 Docker 沙箱，无法安全触达你家里或办公室那台有真实数据、登录态和本地 GPU 的物理电脑，或者需要繁琐的内网穿透且缺乏可视化监管。

**Koko 把这两件事合二为一：**

- **对人类用户**：轻量、不限速、无需公网 IP 的 P2P 远程桌面，随时直连自己的电脑。
- **对 AI Agent**：在同一条加密 P2P 通道上原生提供 **MCP（Model Context Protocol）** 接口，向大模型直接输出按需高清截图、坐标归一化键鼠注入以及（规划中的）系统 UI 无障碍树。
- **人机同屏，随时接管（Human-in-the-Loop）**：你可以实时旁观 AI 在远端电脑上干活，对高危操作点确认，**动一下鼠标或键盘即可接管控制权**。

## 核心设计亮点

- **基于 QUIC 的纯 P2P 直连**：底层采用 [iroh](https://github.com/n0-computer/iroh)，内置 NAT 穿透与中继回落；设备以加密公钥（`NodeId`）互认，默认端到端加密。
- **四通道多路复用**。在同一条 QUIC 连接内隔离控制、媒体、输入和 Agent RPC，减少高码率视频对控制指令的影响。
- **主机侧控制权仲裁（Control Arbiter）**：由远端守护进程强制执行人机抢占。一旦主人介入操作，系统立刻吊销 Agent 的控制租约并返回 `preempted_by_human` 状态，避免 AI 与人抢鼠标。
- **精细化权限分级**：支持按需授予 `Observer`（仅旁观）、`AgentSafe`（安全受限 + 敏感审批）、`AgentFull`（完整操控）、`HumanAdmin`（最高权限），告别“一刀切”授权。

## 快速示例：用 MCP 让 AI 操控远端电脑

> 以下示例展示 v0.1 规划中的 CLI 与 MCP 配置。

1. 在家里或办公室的被控电脑上启动守护进程：

   ```bash
   koko-daemon --allow-agent-safe
   # 输出: Device NodeId: <your-device-node-id>
   ```

2. 在支持 MCP 的客户端（如 Claude Desktop 等）中配置 `koko-agent`：

   ```json
   {
     "mcpServers": {
       "koko": {
         "command": "koko-agent",
         "args": ["mcp", "--peer", "<your-device-node-id>"]
       }
     }
   }
   ```

3. 在 Phase 2 规划的桌面客户端 `koko-client` 中实时观看 AI 操作，并在需要时接管。

## 本地开发

安装 [rustup](https://rust-lang.org/install) 后，在仓库根目录执行以下命令。工作区使用稳定版 Rust，并指定 Clippy 和 rustfmt 组件。

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run --locked -p koko-daemon
cargo run --locked -p koko-agent
```

两个程序目前在输出骨架状态后退出。

## 代码仓库结构

```text
koko/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
└── crates/
    ├── koko-proto/      # 共享协议定义、权限模型与控制权仲裁状态
    ├── koko-p2p/        # 基于 iroh 的 P2P 打洞与四通道多路复用
    ├── koko-capture/    # 跨平台按需抓帧与屏幕采集 (xcap / scap)
    ├── koko-input/      # 跨平台键鼠注入与 DPI 坐标归一化 (enigo)
    ├── koko-daemon/     # 远端被控守护进程（含仲裁器与安全门禁）
    └── koko-agent/      # 本地 MCP Server (rmcp) 与命令行工具
```

`koko-client` 计划在 Phase 2 加入。技术选型与协议设计见 [架构文档](./architecture.md)，首轮交付范围与验收方式见 [Phase 1 实现计划](./implementation.md)。

## 路线图 (Roadmap)

- [ ] **Phase 1 — 无头 Agent 闭环（MVP）**
  - `koko-p2p` 加密 QUIC 穿透与设备公钥配对
  - `koko-daemon` 按需高清截图 + 归一化键鼠注入（Windows / macOS / Linux X11）
  - `koko-agent` 内置 MCP Server（`koko_list_devices`、`koko_screenshot`、`koko_mouse_action`、`koko_keyboard_action`、`koko_request_human`）
- [ ] **Phase 2 — 实时旁观与即时接管**
  - 桌面端 `koko-client` 低延迟画面播放
  - 主机侧 Control Arbiter 抢占机制（人类动鼠标即暂停 Agent）
- [ ] **Phase 3 — UI 无障碍树与高危门禁**
  - 跨平台 Accessibility Tree 提取，降低多模态 Token 消耗并提升点击精度
  - 敏感操作与 Shell 执行的实时弹窗审批
- [ ] **Phase 4 — 移动端协同与自建中继**
  - iOS / Android 移动端旁观、审批与应急接管
  - 极简自建中继服务器部署文档

## 安全与隐私原则

让 AI 操控真实物理电脑必须有硬边界，Koko 坚持三条底线：

1. **零信任加密传输**：节点间基于公钥身份认证，中继节点无法解密任何画面或键鼠数据。
2. **远端主机强制仲裁**：权限边界、高危拦截与人类抢占全部在被控端 `koko-daemon` 执行，不依赖调用方自律。
3. **显式确认机制**：涉及敏感命令或关键交互时，支持挂起任务并由人类点击确认后放行。

## 开源协议

Koko 采用 GNU 通用公共许可证第 3 版（GPL-3.0），详见 [LICENSE](../LICENSE)。
