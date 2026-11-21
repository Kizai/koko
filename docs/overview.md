<div align="center">
  <h1>Koko (叩叩)</h1>
  <p><b>Knock, and you're in.</b></p>
  <p>An Agent-native, P2P remote desktop in Rust — built for both <b>Humans</b> and <b>AI Agents</b>.</p>
  <p>
    <a href="./overview.zh-CN.md">简体中文</a> ·
    <a href="./README.md">Documentation</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust" alt="Built with Rust" />
    <img src="https://img.shields.io/badge/Protocol-QUIC%20%2F%20P2P-0ea5e9" alt="QUIC P2P" />
    <img src="https://img.shields.io/badge/AI%20Interface-MCP%20%2F%20Computer%20Use-8b5cf6" alt="MCP Ready" />
    <img src="https://img.shields.io/badge/License-GPL--3.0-22c55e" alt="GPL-3.0" />
  </p>
</div>

> Phase 1 supports persistent device identities and authorized LAN connection checks. The remote desktop features and MCP interfaces below are planned.

## What is Koko?

**Koko** (*from the sound of knocking — "knock, knock"*) is a lightweight, end-to-end encrypted P2P remote desktop written in Rust, designed from day one for **Human & Agent-to-Device** workflows.

Traditional remote desktop tools are built only for human eyes: they stream video and expect a person to click around. Meanwhile, AI Computer Use agents are usually trapped inside isolated cloud sandboxes without access to your real files, active sessions, or local GPU — or they require fragile port forwarding and headless scripts with zero visual oversight.

**Koko bridges both worlds:**

- **For Humans**: Fast, peer-to-peer remote desktop across your own machines without bandwidth throttling or public IP setup.
- **For AI Agents**: A native **Model Context Protocol (MCP)** bridge over the same encrypted P2P tunnel, delivering on-demand crisp frames, normalized input injection, and (planned) OS accessibility trees.
- **Human-in-the-Loop by Design**: Watch your AI agent work on your home or office machine in real time, approve sensitive actions, and **take over control instantly** the moment you move your mouse or press a key.

## Core Design Highlights

- **Direct P2P over QUIC**: Powered by [iroh](https://github.com/n0-computer/iroh) with built-in NAT traversal, relay fallback, and public-key device identity (`NodeId`) — end-to-end encrypted by default.
- **Four-Channel Multiplexing**. Control, media, input, and Agent RPC share one QUIC connection on separate channels to reduce interference from high-bitrate video.
- **Instant Human Preemption**: The remote daemon enforces a **Control Arbiter**. Human input immediately revokes the agent's active lease and notifies the model (`preempted_by_human`) instead of letting the agent fight for your cursor.
- **Scoped Capabilities**: Pair devices with granular roles (`Observer`, `AgentSafe`, `AgentFull`, `HumanAdmin`) rather than all-or-nothing access.

## Quick Example: Connecting an AI Agent via MCP

> This example shows the planned v0.1 CLI and MCP configuration.

1. Start the host daemon on your home or office machine:

   ```bash
   koko-daemon --allow-agent-safe
   # Output: Device NodeId: <your-device-node-id>
   ```

2. Add `koko-agent` to your MCP client (e.g., Claude Desktop or any MCP-compatible host):

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

3. With the planned Phase 2 desktop client, open `koko-client` on your laptop to watch the live session and step in anytime.

## Development

Install [rustup](https://rust-lang.org/install) and run these commands from the repository root. The workspace selects stable Rust with Clippy and rustfmt.

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Use the [macOS test app](./macos.md) to select roles on two Macs. The [LAN connection commands](./implementation.md#身份与连接验收命令) cover CLI debugging.

## Workspace Layout

```text
koko/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
└── crates/
    ├── koko-proto/      # Wire protocol, capabilities, and Control Arbiter state
    ├── koko-p2p/        # P2P transport, NAT traversal, and multiplexed streams (iroh)
    ├── koko-capture/    # Cross-platform frame & screen capture (xcap / scap)
    ├── koko-input/      # Cross-platform mouse & keyboard injection (enigo)
    ├── koko-daemon/     # Host background service with policy enforcement
    └── koko-agent/      # Local MCP server (rmcp) and CLI
```

`koko-client` is planned for Phase 2. See the [architecture design](./architecture.md) and the [Phase 1 implementation plan](./implementation.md).

## Roadmap

- [ ] **Phase 1 — Headless Agent Loop (MVP)**
  - `koko-p2p` encrypted QUIC tunnel + device pairing
  - `koko-daemon` on-demand screenshots + normalized input injection (Windows / macOS / Linux X11)
  - `koko-agent` built-in MCP server (`koko_list_devices`, `koko_screenshot`, `koko_mouse_action`, `koko_keyboard_action`, `koko_request_human`)
- [ ] **Phase 2 — Live Watch & Instant Takeover**
  - Desktop `koko-client` with low-latency video playback
  - Control Arbiter preemption (human mouse/keyboard immediately pauses agent)
- [ ] **Phase 3 — Structured UI & Guardrails**
  - Cross-platform Accessibility Tree extraction for lower token usage and higher click precision
  - Interactive approval cards for high-risk actions and shell execution
- [ ] **Phase 4 — Mobile & Self-Hosting**
  - iOS / Android companion apps for live monitoring, approvals, and quick takeover
  - Self-hosted relay deployment guide

## Security & Privacy

Giving an AI agent control over a real computer requires strict boundaries. Koko is designed around three non-negotiable rules:

1. **Zero-Trust Transport**: Every peer is authenticated by its cryptographic `NodeId`; relays never see plaintext screen data or input events.
2. **Host-Side Enforcement**: Permission scopes, rate limits, and human preemption are enforced on the remote host daemon, never trusted to the caller.
3. **Explicit Consent**: Sensitive operations can be gated behind interactive human approval before execution.

## License

Koko is licensed under the GNU General Public License version 3 (GPL-3.0). See [LICENSE](../LICENSE).
