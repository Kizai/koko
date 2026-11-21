# Koko (叩叩)

**Knock, and you're in.**

Koko is a Rust-based, peer-to-peer remote desktop for people and AI agents. Connect to a device you own, see its screen, and work with its keyboard and mouse. The host controls access and can end the session locally. An upcoming MCP bridge will expose the same connection to AI agents under the host's permissions.

[简体中文](./docs/overview.zh-CN.md) · [Documentation](./docs/README.md) · [Architecture](./docs/architecture.md)

## Project status

The macOS test app currently supports two ways to start a session. Use a one-time authorization code issued by the host, or discover a Mac on the LAN and ask its owner to approve. A direct invite carries the authorized code and network addresses for routed networks where LAN discovery is unavailable.

| Available now | Roadmap |
| --- | --- |
| Local sharing and remote connections in one macOS app, with one-time authorization codes and LAN discovery | Read-only MCP integration and internet direct connection and relay tests |
| Multi-display viewing, adaptive capture resolution and quality, pointer movement, clicks, dragging, and scrolling | Video streaming, audio, and full keyboard forwarding |
| Versioned development DMGs for Apple Silicon and Intel, targeting macOS 13 and later | Additional platforms and live observation with local takeover |

The remote window fits the selected display to the available space. Its Control Center offers transmission resolution, JPEG quality, full screen, shortcuts, and text entry. [macOS testing guide](./docs/macos.md)

## Try it on two Macs

Build the test package from a Mac with [Rust](https://rust-lang.org/install) and Xcode Command Line Tools. From the repository root, run:

```bash
./scripts/build-macos.sh
```

The output is `target/koko-macos/Koko-<version>.dmg`. Open the same image on both Macs and drag `Koko.app` into Applications. The app displays its version next to its name.

1. On the Mac to be shared, enable screen recording and Accessibility in Koko's permissions panel, then turn on **Allow Remote Assistance** (`允许远程协助`). The default address is `0.0.0.0:4444/UDP`.
2. On the other Mac, use **Connect to a Partner** (`连接伙伴`). Enter the short code on the same LAN or paste the host's direct invite over a routed or mesh network. The code gives the connecting device view and input access for one session and expires after five minutes or one use.
3. For code-free LAN connections, refresh the sidebar and select the host. The host receives an approval request when you select it.
4. Operate the mouse within the remote image and use the display tabs to switch screens. Open **Control Center** (`控制中心`) for shortcuts, text entry, and ending the session.

The host can turn off **Allow Remote Assistance** (`允许远程协助`) to revoke access and close the listener. [Installation, permissions, and key mappings](./docs/macos.md)

## Device access

Each device has a persistent cryptographic identity. Koko uses iroh and QUIC to authenticate the other endpoint, then checks the authorized controller on the host before any desktop action. Private keys stay on the Mac that created them.

OS permissions stay under the host user's control. Screen recording allows preview images, and Accessibility allows keyboard and mouse events. Koko requests each permission only from its corresponding local button. [macOS permission design](./docs/macos-permissions.md)

## For agents and scripts

`koko-agent pair <code-or-invite>` returns the authorized host identity and address as JSON. `koko-agent discover` lists hosts advertising on the LAN, and the `remote` command accepts typed screenshot and input requests on standard input. Both use the same host authorization as the GUI. [CLI and agent examples](./docs/macos.md#agent-配对)

## Architecture

- `koko-proto` defines the pairing and desktop messages.
- `koko-p2p` resolves local devices, verifies public-key identities, and sends requests over QUIC.
- `koko-daemon` authorizes sessions and delegates desktop operations to the macOS app or the standalone CLI helper.
- `koko-agent` exposes the controller functions used by the SwiftUI app and scripts.

The macOS implementation uses ScreenCaptureKit for snapshots and Core Graphics for keyboard and mouse events. [Architecture and roadmap](./docs/architecture.md)

## Development

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The Rust workspace contains the shared protocol, P2P transport, host daemon, agent, capture, and input crates. The SwiftUI test app in `apps/koko-macos/` wraps the Rust executables. [Development rules](./docs/development.md) · [Documentation index](./docs/README.md)

## License

Licensed under [GPL-3.0](./LICENSE).
