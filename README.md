<div align="center">
  <h1>🚪 Koko (叩叩远程)</h1>
  <p><b>敲一下，就连上 —— 为「人类」与「AI Agent」打造的 Rust 全平台 P2P 远程控制底座</b></p>
  <p>
    <i>"Knock, and you're in. Built for Humans, Ready for AI Agents."</i>
  </p>
  <p>
    <img src="https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust" />
    <img src="https://img.shields.io/badge/AI%20Ready-Computer%20Use%20%2F%20MCP-8A2BE2" />
    <img src="https://img.shields.io/badge/Platform-Win%20%7C%20Mac%20%7C%20Linux%20%7C%20iOS%20%7C%20Android%20%7C%20Web-blue" />
    <img src="https://img.shields.io/badge/License-Apache--2.0-green" />
  </p>
</div>

---

## 🌟 什么是 Koko？

**Koko（叩叩）** 源自敲门声 *"Knock, Knock"*。

它首先是一个**极致轻量、不限速的全平台点对点（P2P）远程桌面**——让你像用 UU远程、向日葵一样，随时随地秒连你的任何设备。

但 Koko 不止于此。它是全球首个**原生支持 AI Agent 接管（Computer Use）** 的 P2P 远程软件。通过 Koko，你的 AI 助手（Claude / OpenAI / 本地大模型）可以跨越网络限制，安全地“敲开”并操作你远在家里或办公室的真实电脑，而无需任何复杂的公网 IP 或内网穿透配置。

## 🚀 核心特性

### 🧑‍💻 对人类用户：极致丝滑的远程桌面
- ⚡ **天生 P2P 直连**：深度优化的 NAT 穿透（STUN/TURN/QUIC），榨干物理带宽，告别商业软件的“免费版限速与画质阉割”。
- 🦀 **100% Rust 驱动**：零垃圾回收开销，毫秒级音画同步，安装包仅数 MB。
- 🌍 **真·全平台覆盖**：一套核心，运行于 Windows、macOS、Linux、Android、iOS 及 Web。

### 🤖 对 AI Agent：开箱即用的远程数字躯体 (Agent-Ready)
- 🔌 **原生 MCP & Agent SDK**：一键将任何安装了 Koko 的远程电脑暴露为 AI Agent 的工具节点（完美对接 Claude Computer Use、LangChain 等）。
- 👁️ **AI 视觉与 UI 树双通道**：除了给人类看的 H.264/AV1 视频流，Koko 专为 AI 提供按需低延迟帧抓取与系统级 UI 元素树（Accessibility Tree）读取。
- 🛡️ **人类随时接管 (Human-in-the-loop)**：你可以在手机或电脑上实时观看 AI 操作你的远程电脑，并随时通过键鼠一键打断或接管控制权，安全可控。
- 🔒 **端到端加密 (E2EE) 与精细权限**：基于公钥密码学的设备授权，可限制 Agent 仅能观察、或需确认后才能执行高危点击。

## 🗺️ 路线图 (Roadmap)

- [ ] **Phase 1: Koko Core (P2P 远程基座)**
  - 基于 Rust + QUIC/WebRTC 的点对点穿透与加密通信
  - 跨平台屏幕采集、硬件编解码与远程键鼠注入
- [ ] **Phase 2: 全客户端覆盖**
  - 桌面端（Win/Mac/Linux）与移动端（iOS/Android）极简 UI
- [ ] **Phase 3: Koko Agent Bridge (AI 赋能)**
  - 内置 MCP (Model Context Protocol) Server，让大模型一行配置直连远程电脑
  - 实时“人类旁观 + AI 操控”协同模式
