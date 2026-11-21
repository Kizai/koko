# Koko (叩叩) 技术架构与协议设计（MVP v0.1）

[返回文档目录](./README.md)

本文档定义 **Koko** 的 MVP 技术栈选型、基于 QUIC 的双模传输协议（`koko-p2p`）、AI Agent 桥接层（`koko-agent`）以及核心差异化能力——**人机控制权仲裁器（Human-in-the-Loop Arbiter）**的工程设计。

> 本文描述 MVP v0.1 的设计目标与后续阶段规划。首轮交付范围与验收顺序见 [Phase 1 实现计划](./implementation.md)。

## 1. MVP 技术栈定案

为避免在早期陷入 6 端适配与底层编解码泥潭，MVP 阶段锁定 **Desktop 优先（Windows / macOS / Linux）+ P2P 打洞 + 原生 MCP 桥接**，优先跑通“人类旁观 + AI 远程操控真实电脑 + 随时接管”的完整闭环。

| 层级 / 模块 | 推荐选型 | 核心理由 | 备选方案与取舍 |
| :--- | :--- | :--- | :--- |
| **P2P 传输与打洞 (`koko-p2p`)** | [iroh](https://github.com/n0-computer/iroh) | 纯 Rust 实现的 QUIC + NAT 穿透 + 中继回落；按公钥（`NodeId`）拨号，天然端到端加密；单条连接支持多路复用 Stream，完美同时承载视频流与 Agent RPC | WebRTC（适合浏览器直连，但信令复杂、DataChannel 承载结构化 RPC 较笨重，留待后续 Web 网关扩展） |
| **MCP 协议层 (`koko-agent`)** | [rmcp](https://github.com/modelcontextprotocol/rust-sdk) | 官方 Rust MCP SDK，支持 `#[tool]` 宏与 stdio / HTTP 传输，直接将远端设备暴露为 Claude / OpenAI / 本地模型的标准工具节点 | 自研 JSON-RPC（维护成本高，生态兼容性差） |
| **屏幕采集** | macOS 测试包使用 [ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit) | 图形版由 Koko.app 进程按需采集选中的显示器并编码 JPEG，原有单帧 CLI 仍可请求主屏 PNG | 其他平台可评估 [xcap](https://github.com/nashaofu/xcap)，高帧率流另行验证 [scap](https://github.com/CapSoftware/scap) 或原生方案 |
| **键鼠注入** | macOS 测试包使用 [Core Graphics](https://developer.apple.com/documentation/coregraphics/cgevent) | 画面与鼠标使用独立连接，按显示器映射坐标，顺序传递按钮与拖拽事件 | 其他平台可评估 [enigo](https://github.com/enigo-rs/enigo)，Wayland 支持按实际输入后端验证 |
| **桌面客户端 (`koko-client`)** | [Tauri v2](https://github.com/tauri-apps/tauri)（MVP 首选） | 研发效率高、包体小，极适合快速构建“设备列表 + 实时旁观画面 + Agent 操作日志/审批面板 + 一键接管悬浮条” | 若明确要第一时间做 iOS/Android 且追求零拷贝纹理渲染，改用 Flutter + [flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge)；若追求纯 Rust 极简分发可选 [Slint](https://github.com/slint-ui/slint) |

## 2. 系统总体架构（Human + Agent 双模拓扑）

Koko 的核心思想是**“一套 P2P 加密通道，同时服务人类视觉与 AI 工具调用”**。本地侧（Controller）运行极简 MCP Server 与旁观 UI；远端侧（Host）运行 `koko-daemon`，负责采集、注入与权限仲裁。

```mermaid
flowchart LR
    subgraph Local["本地控制端 (Controller)"]
        AI["AI Agent (Claude / Local LLM)"]
        MCP["koko-agent (MCP Server / rmcp)"]
        UI["koko-client (旁观画面 & 接管控制台)"]
        P2P_L["koko-p2p (Iroh Endpoint)"]
        AI <-->|"MCP (stdio / HTTP)"| MCP
        MCP <--> P2P_L
        UI <--> P2P_L
    end

    subgraph Network["P2P 加密通道 (Iroh QUIC / Relay Fallback)"]
        S1["0x01 Control & Arbiter"]
        S2["0x02 Media (Video/Audio)"]
        S3["0x03 Human/Agent Input"]
        S4["0x04 Agent RPC (Screenshot / UI Tree / Shell)"]
    end

    subgraph Remote["远端被控电脑 (Host: 家用/办公物理机)"]
        P2P_R["koko-p2p (Iroh Endpoint)"]
        ARB["Control Arbiter (人机仲裁与策略门禁)"]
        CAP["koko-capture (xcap / scap)"]
        INP["koko-input (enigo)"]
        SH["Policy-Gated Shell / File"]
        P2P_R <--> ARB
        ARB --> CAP
        ARB --> INP
        ARB --> SH
    end

    P2P_L <--> S1 <--> P2P_R
    P2P_L <--> S2 <--> P2P_R
    P2P_L <--> S3 <--> P2P_R
    P2P_L <--> S4 <--> P2P_R
```

## 3. `koko-p2p` 多路复用协议设计

基于 [iroh](https://github.com/n0-computer/iroh) 的 QUIC 连接，定义统一 ALPN：`koko/1`。所有流量天然受 TLS 1.3 + Ed25519 设备公钥身份保护，无需额外搭建复杂的账号中心即可完成设备互信。

### 3.1 身份与分级授权模型

每台主机启动 `koko-daemon` 时生成持久化密钥对，公开标识为其 `NodeId`。主人在远端授权时可签发不同权限作用域（Capability Scope）的配对凭证：

| 权限级别 | 允许的操作 | 典型场景 |
| :--- | :--- | :--- |
| `Observer` | 仅拉取视频流、按需截图、读取只读 UI 树 | 手机端纯旁观、AI 仅做屏幕诊断 |
| `AgentSafe` | 截图、读取 UI 树、在允许区域内点击/输入；高危操作需人工点击确认 | 日常让 AI 整理文件、操作浏览器、跑常规任务 |
| `AgentFull` | 完整键鼠注入 + 受控 Shell / 文件传输（仍受人类随时抢占约束） | 无人值守跑脚本、视频渲染、自动化测试 |
| `HumanAdmin` | 最高优先级，随时抢占控制权、踢出 Agent 会话、修改策略 | 主人本人远程操作与应急接管 |

### 3.2 四通道 Stream 规划

在单条 QUIC 连接内按业务语义隔离四个通道，避免大体积视频帧阻塞键鼠或紧急暂停指令：

| 通道标识 | QUIC 形态 | 帧格式 | 职责说明 |
| :--- | :--- | :--- | :--- |
| **0x01 Control** | 双向可靠 Stream | Length-Prefixed CBOR / JSON | 握手鉴权、能力协商、心跳、**控制权切换事件**、高危审批弹窗回调、紧急急停（Kill-Switch） |
| **0x02 Media** | 单向 Stream 或 QUIC Datagram | 自定义帧头 + H.264 NAL 单元或 AV1 OBU | 面向人类旁观的低延迟音视频流；支持丢帧追赶，减少视频传输对控制指令的影响 |
| **0x03 Input** | 双向可靠 Stream | Compact Binary / CBOR | 标准化键鼠与剪贴板事件，每条事件携带 `actor: Human \| Agent` 与 `lease_id` |
| **0x04 Agent RPC** | 双向可靠 Stream | Length-Prefixed JSON-RPC 2.0 | 承载 `koko-agent` 发往远端的按需抓帧（PNG/WebP + 坐标缩放元数据）、UI 树查询、受控命令执行 |

身份与连接工作项在一条双向流上验证授权。每个方向发送一帧，格式依次为 `0x01`、四字节大端载荷长度和 JSON 字符串。控制端发 `"hello"`，授权后的主机回 `"ready"`。本轮使用固定的七字节 JSON 载荷。主机在读取请求前根据 QUIC 握手确认的公钥检查授权名单，检查完成后由控制端关闭连接。

短连接码通过独立的 `koko/pair/1` QUIC ALPN 配对。发现服务公布主机公钥、地址以及连接码与该公钥绑定的摘要。控制端通过 QUIC 验证主机公钥，连接码只在加密连接内提交。组网直连邀请提供公钥和候选 IPv4 地址。有效授权码直接授予当前会话的查看与键鼠权限。局域网搜索只发现设备，从列表发出的无授权码请求需要被控端本机批准。

macOS 测试版使用 `koko/remote/1` 在独立 QUIC 流中传递有限长度的 JSON 命令和响应。主机检查当前授权控制端公钥，采集和输入通过包内 helper 调用 ScreenCaptureKit 和 Core Graphics。初版支持主屏 PNG、比例坐标点击、文本、明确的常用按键及断开会话。上表中的 Media 与 Agent RPC 是后续协议规划。

## 4. `koko-agent`（MCP 工具集）与人机协同仲裁

### 4.1 暴露给大模型的标准 MCP 工具

`koko-agent` 通过 [rmcp](https://github.com/modelcontextprotocol/rust-sdk) 在本地启动 MCP Server，将远端主机抽象为以下 6 个原子工具。UI 树查询安排在 Phase 3，其余工具纳入 MVP。

| MCP Tool 名称 | 核心参数 | 返回内容 | 备注 |
| :--- | :--- | :--- | :--- |
| `koko_list_devices` | 无 | 已配对设备列表、在线状态、当前控制权归属 | 供 Agent 确认目标机器 |
| `koko_screenshot` | `device_id`, `max_width`, `format` | Base64 图像 + 原始/缩放分辨率映射比例 | 避免视频流编码损耗，按需抓高清帧 |
| `koko_mouse_action` | `device_id`, `action` (`move`/`click`/`drag`/`scroll`), `x`, `y` | 执行状态 + 是否触发人工接管 | 坐标统一按逻辑屏幕比例换算，消除 DPI 偏差 |
| `koko_keyboard_action` | `device_id`, `text` 或 `keys` | 执行状态 | 支持组合键与 Unicode 文本输入 |
| `koko_get_ui_tree` | `device_id`, `depth` | 精简后的窗口与可交互控件 JSON（Phase 3 补齐） | 降低多模态 Token 消耗，提高点击精度 |
| `koko_request_human` | `device_id`, `reason` | 阻塞等待人类确认或接管完成 | 遇到登录验证码、支付、二次确认时主动呼叫主人 |

### 4.2 控制权仲裁状态机（Human-in-the-Loop Arbiter）

这是 Koko 区别于普通远程桌面和普通 MCP 插件的核心安全机制，全部在远端 `koko-daemon` 侧强制执行（防止失控的本地 Agent 绕过）：

1. **`HumanOnly`**：默认状态，拒绝所有 `actor = Agent` 的输入与 RPC 写入。
2. **`AgentRunning`**：主人授权后进入，Agent 可通过 `lease_id` 执行操作；客户端 UI 实时高亮显示“AI 正在操作”边框与操作轨迹。
3. **`HumanTakeover`（即时抢占）**：只要人类在本地客户端移动鼠标/按键，或直接在远端物理机操作，Arbiter 立即吊销当前 Agent 的 `lease_id`，清空待执行输入队列，并向正在等待结果的 MCP 调用返回结构化状态：
   ```json
   {
     "status": "preempted_by_human",
     "message": "Human operator took over control. Pause actions and wait for resume."
   }
   ```
   这样大模型不会盲目重试，而是明确感知“主人接管了，先停下来等指令”。
4. **`ApprovalPending`（高危门禁）**：当策略命中敏感操作（如 `shell_exec`、快捷键删除、访问敏感目录）时，挂起该次 RPC，通过 `0x01 Control` 通道向主人客户端推送“允许一次 / 拒绝”卡片。

## 5. Rust Workspace 目录与分阶段落地计划

### 5.1 精简后的 Workspace 结构

```text
koko/
├── Cargo.toml
└── crates/
    ├── koko-proto/      # 共享协议定义、消息帧、权限模型、Arbiter 状态
    ├── koko-p2p/        # 基于 iroh 的连接管理、设备配对、四通道封装
    ├── koko-capture/    # 基于 xcap/scap 的按需抓帧与视频流采集
    ├── koko-input/      # 基于 enigo 的跨平台键鼠注入与坐标归一化
    ├── koko-daemon/     # 远端常驻守护进程（含 Arbiter 仲裁器与策略引擎）
    ├── koko-agent/      # 本地 MCP Server (rmcp) + CLI
    └── koko-client/     # 桌面端 UI (Tauri v2)：旁观画面、审批弹窗、一键接管
```

### 5.2 务实的四阶段里程碑

| 阶段 | 核心目标 | 验收标准（可演示 Demo） |
| :--- | :--- | :--- |
| **Phase 1: Headless Agent Loop** | 跑通 `koko-proto` + `koko-p2p` + `koko-daemon` + `koko-agent` | Claude Desktop 通过本地 MCP 配置，跨越 NAT 穿透连接到家里电脑，完成截图与键鼠点击 |
| **Phase 2: Watch & Takeover** | 引入 `koko-client`（Tauri v2）与 `Media` 通道 + Arbiter 抢占 | 主人实时看到 AI 在远端电脑的操作画面，鼠标一动立即抢占接管，AI 自动暂停 |
| **Phase 3: UI Tree & Policy** | 补齐三端 UI Accessibility Tree、高危审批卡片、硬件编解码优化 | AI 结合 UI 树与按需截图精准操控复杂应用；敏感操作触发客户端弹窗确认 |
| **Phase 4: Mobile & Self-Host** | 移动端旁观/审批客户端（iOS/Android）与自建中继文档 | 手机随时看进度、点同意、或应急接管 |

## 6. 跨平台工程避坑清单

- **macOS 权限引导**在被控端用户启用对应功能时触发。按需截图需要本次会话共享授权和系统录屏许可，键鼠权限随远程输入功能接入。[macOS 权限设计](./macos-permissions.md)
- **Windows DPI 与 UAC**：高分屏缩放会导致截图坐标与物理像素错位，必须在 `koko_screenshot` 返回值中携带逻辑/物理尺寸与显示器原点偏移；涉及管理员权限窗口时，普通用户态进程无法注入输入，需预留 Windows Service 模式。
- **Linux Wayland 兼容**：Wayland 出于安全隔离限制直接截屏与输入注入，MVP 阶段建议明确优先支持 X11，Wayland 下通过 `xdg-desktop-portal` 逐步适配。
