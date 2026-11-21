# Koko (叩叩)

Koko is building a Rust-based, peer-to-peer remote desktop for people and AI agents. The goal is to let you connect to a device you own, choose what the other side can see or do, and take back control locally. An MCP bridge will let an AI agent use the same device connection under the host's permissions.

[简体中文](./docs/overview.zh-CN.md) · [Documentation](./docs/README.md) · [Architecture](./docs/architecture.md)

## Project status

The current macOS test app pairs two Macs using a short code on the same local network or a direct invite over a routed private network. Each device keeps its own identity. Koko verifies identities over an encrypted QUIC connection and requires the host to approve pairing.

| Available now | Roadmap |
| --- | --- |
| One macOS app with controller and host modes, short connection codes, and a screen recording permission panel | On-demand screenshots of the host's primary display, shown in the controller app |
| One development DMG with Apple Silicon and Intel binaries targeting macOS 13 and later | Read-only MCP tools and separate tests of internet direct connections and relay fallback |
| A `koko-agent pair <code-or-invite>` command with JSON output for agents and scripts | Keyboard and mouse control, live viewing, and local takeover |

The current verification finishes when the controller shows `配对完成`. The screenshot milestone adds the first visible remote image. [Phase 1 implementation plan](./docs/implementation.md)

## Try it on two Macs

Build the test package from a Mac with [Rust](https://rust-lang.org/install) and Xcode Command Line Tools. From the repository root, run:

```bash
./scripts/build-macos.sh
```

The output is `target/koko-macos/Koko-<version>.dmg`. Open the same image on both Macs and drag `Koko.app` into Applications. The app displays its version next to its name.

1. On the Mac to be shared, select **Host** (`被控端`) and start listening. The default address is `0.0.0.0:4444`.
2. On the other Mac, select **Controller** (`控制端`) and enter the short code on the same LAN. Over a routed or mesh network, paste the host's direct invite.
3. Approve the request on the host. The code expires after one approval or five minutes.

The app remembers the selected role on each device. The host starts listening when its user clicks the button. [Installation and agent pairing](./docs/macos.md)

## Device access

The host controls which device can connect. Koko discovers device addresses and exchanges public identities during pairing. Private keys stay on the Mac that created them.

The host panel checks and requests macOS screen recording permission. The screenshot milestone will add in-session screen sharing. Keyboard and mouse permissions are planned for the input milestone. [macOS permission design](./docs/macos-permissions.md)

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
