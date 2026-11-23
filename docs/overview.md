<p align="center">
  <img src="./assets/koko-logo.png" alt="KoKo logo" width="128">
</p>
<h1 align="center">KoKo</h1>
<p align="center">An open-source P2P remote desktop with a Rust networking and session core.</p>
<p align="center">
  <a href="../README.md">简体中文</a> ·
  <a href="https://github.com/Kizai/koko/releases">Downloads</a> ·
  <a href="./README.md">Documentation</a>
</p>

KoKo connects two authorized Macs over a local or routed network. The host decides who can connect and can end the session at any time. The same authorization is available to scripts and agents through the CLI.

## What works today

- LAN discovery, single-use codes, routed-network invitations, and reconnection for trusted controllers.
- Continuous H.264 video, multiple displays, mouse and keyboard input, and a view-only mode. The Control Center sets resolution, quality, and target frame rate. The network panel reports the actual receive rate.
- Starting with 0.1.11 on both Macs, Command-C and Command-V share plain text or regular files over the paired session.
- Single-file transfers, Terminal, and localhost port forwarding through the host's SSH account.
- A universal macOS 13+ preview for Apple Silicon and Intel. An MCP server (`koko-agent mcp`) lets AI agents pair, take screenshots, and use the mouse and keyboard. The host owner sees when an agent is in control and can stop it at any time. Internet traversal and relay testing are on the roadmap.

## Connect two Macs

1. Download the same DMG for both Macs from [Releases](https://github.com/Kizai/koko/releases). In v0.1.15 and newer, double-click the installer icon in the disk image, complete the installer, then open KoKo from Applications.
2. On the host Mac, allow Screen Recording and Accessibility, then enable **Allow Remote Assistance**. The default listen address is `0.0.0.0:27688/UDP` (with automatic fallback if occupied).
3. On the controller, refresh LAN devices and choose the host, or enter its one-time code under **Connect to a Partner**. Use the host's direct invitation when your routed network does not forward LAN discovery.
4. On the host, approve the first LAN request. Choose **Trust and Connect** to allow that controller to reconnect while sharing is enabled.

Starting with 0.1.16, KoKo checks GitHub Releases when the app becomes active, up to once a day. Open **Settings → General → Check for Updates** to check immediately. Starting with 0.1.20, choose **Download and Install** to see download progress, then **Install and Relaunch**. Older clients can install the DMG once to enable this flow. Local ad-hoc builds may require granting macOS permissions again after an upgrade. [Permissions and signing](./macos-permissions.md)

For SSH tools, enable Remote Login for the intended host account. File transfers and forwarding use SSH keys and a verified host key. [macOS setup and troubleshooting](./macos.md)

## Architecture

Rust handles protocol messages, identity, QUIC transport, session authorization, and the CLI. Swift and SwiftUI handle macOS windows and platform integration with ScreenCaptureKit, VideoToolbox, AVFoundation, and Core Graphics. [Architecture and roadmap](./architecture.md)

## Build from source

Install Rust and the Xcode Command Line Tools, then build and verify the workspace from the repository root.

```bash
./scripts/build-macos.sh --allow-ad-hoc
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Local test DMGs appear under `target/koko-macos/local/`. See the [signing instructions](./macos.md#构建测试包) and the [development guide](./development.md) for contribution details.

## License

KoKo is licensed under the [GNU General Public License, version 3](../LICENSE).
