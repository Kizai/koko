<p align="center">
  <img src="./docs/assets/koko-logo.png" alt="KoKo 的紫色门扉标志" width="144">
</p>
<h1 align="center">KoKo（叩叩）</h1>
<p align="center">面向个人和 Agent 的开源 P2P 远程桌面，网络与会话核心由 Rust 实现。</p>
<p align="center">
  <a href="https://github.com/Kizai/koko/releases">下载内测版</a> ·
  <a href="#快速开始">安装与使用</a> ·
  <a href="./docs/README.md">文档</a> ·
  <a href="./docs/overview.md">English</a>
</p>

KoKo 让你在另一台 Mac 上查看和操作已授权的桌面。被控端决定谁能连接，并可以随时结束共享。同一套授权也可供脚本和 Agent 通过命令行使用。

## 当前版本

首个内测版支持 **macOS 13 及以上**，提供适用于 Apple Silicon 和 Intel 的通用 DMG。当前测试范围为局域网和两台设备可直接通信的组网环境。

| 功能 | 当前可用 |
| --- | --- |
| 配对 | 局域网发现、一次性连接码、组网直连邀请、信任设备重连 |
| 远程桌面 | H.264 连续推流、多显示器、观看模式、鼠标和键盘 |
| 会话剪贴板 | 通过 ⌘C / ⌘V 双向传输文字、文件和文件夹，大文件连续传输、不限大小（两端需 0.1.22 起） |
| 被控端发送文件 | 被控时拖动文件会出现发送浮窗，拖入即可发给控制端（两端需 0.1.22 起） |
| 画面设置 | 传输分辨率、画质、30–240 FPS 目标档位与实测接收帧率 |
| 设备工具 | 单文件收发、终端、两端 localhost 端口转发，使用系统 SSH |
| Agent 接入 | MCP：设备列表、配对、截图、读取界面元素、鼠标与键盘、向被控端求助；删除、付款、退出应用、终端命令等高危操作需被控端确认；被控端可随时停止或接管 Agent，本机动一下键鼠即可暂停它。另有发现、配对、截图与输入 CLI |

## 快速开始

### 1. 下载并安装

在 [GitHub Releases](https://github.com/Kizai/koko/releases) 打开内测版，下载 `Koko-<版本>.dmg`。从 0.1.15 起，在两台 Mac 上分别打开同一版本的 DMG，双击其中的安装图标，跟随 macOS 安装器完成安装。然后从「应用程序」打开 KoKo。以前的 DMG 可将 `Koko.app` 拖入「应用程序」。安装预编译包只需要 macOS。

从 0.1.16 起，打开 KoKo 或回到主窗口时，应用每天最多自动检查一次 GitHub Releases。有新版 Mac 安装包时，主窗口会显示提醒。点「查看并更新」会打开更新窗口，显示新版本的更新说明；点「立即更新」后，同一条进度条依次显示下载、校验解压和安装，完成后 KoKo 自动重新启动。0.1.21 更换了更新签名密钥并改用自签名证书，0.1.20 及更早版本的用户需要用 DMG 手动升级到 0.1.21 一次，之后的版本可以应用内更新。也可随时点击右上角「设置 → 常规 → 检查更新」。0.x 内测版会检查预发布版本，只有源码的发布会被跳过。

0.1.20 及更早的内测版使用临时签名，每次升级都要重新授予录屏和辅助功能。之后的内测版使用固定的 KoKo 自签名证书，从旧版升级时最后授权一次，以后升级不再需要。升级后可先从「设置 → 安全」复查已有授权。[权限与签名说明](./docs/macos-permissions.md)

### 2. 被控端开启共享

1. 首次打开 KoKo，跟随权限引导逐项开启。之后可从「设置 → 安全 → 权限设置」再次打开。**屏幕录制**用于分享画面，**辅助功能**用于接收键鼠。
2. 按 KoKo 的提示进入「系统设置 → 隐私与安全性」，为应用授权。权限页会自动复查。若系统要求重启，可点「重开 Koko 并复查」回到此页。可点击「测试本机画面」先确认录屏可用。
3. 打开「本机共享」中的 **允许远程协助**。默认监听 `0.0.0.0:4444/UDP`。两台 Mac 若收到本地网络权限提示，请允许 KoKo 在局域网发现和连接设备。

### 3. 控制端选择连接方式

| 使用场景 | 操作 |
| --- | --- |
| 同一个局域网 | 点击侧栏的刷新图标，选择设备并进入桌面。被控端可选择「仅允许这次」或「信任并连接」。 |
| 有一次性连接码 | 把被控端的 10 位连接码填入「连接伙伴」。连接码有效期为 5 分钟，成功使用后失效。 |
| 已组网，或局域网发现失败 | 从被控端复制直连邀请，粘贴到控制端的「连接伙伴」。组网需要能访问被控端的 UDP 监听端口。 |

连接后，直接点击远程画面使用鼠标和键盘。在顶部切换显示器，在「控制中心」调整画质、传输分辨率或目标帧率。被控端关闭「允许远程协助」即可结束当前会话。若需要撤销已记住的控制端，使用「设置 → 安全 → 清除信任」。[完整双机测试指南](./docs/macos.md)

## 常见问题

- **能连接但没画面。** 在被控 Mac 上先运行「测试本机画面」。升级后若系统开关已开启，但 KoKo 仍显示「未授权」，核对系统记录对应的是否是「应用程序」里的当前版本。[macOS 权限排查](./docs/macos-permissions.md)
- **能看画面但点不了。** 检查被控端的辅助功能权限，并用「进入桌面」打开可操作会话。实体键盘需要先点击画面取得焦点。
- **目标帧率和实际帧率不同。** 在被控端持续滚动画面，再从顶部网络状态查看「接收帧率」。实际接收速度取决于显示器更新、编解码和网络。
- **如何操作锁屏的 Mac？** 被控端已开启系统屏幕共享时，可从「控制中心」打开 macOS「屏幕共享」，由系统负责登录。[锁屏和按键说明](./docs/macos.md#画面与按键映射)
- **如何传文件或开终端？** 在局域网设备卡片打开所需工具。被控 Mac 需要开启「远程登录」并授权对应账户。文件收发和端口转发使用 SSH 密钥及已验证的主机指纹。[设备工具配置](./docs/macos.md#设备工具与设置)

## 技术架构

| 层次 | 实现 |
| --- | --- |
| 协议和传输 | Rust 的 `koko-proto` 定义消息，`koko-p2p` 使用 iroh / QUIC 完成身份认证与传输，并用 mDNS 发现局域网设备 |
| 会话和命令行 | Rust 的 `koko-daemon` 负责被控端会话与授权，`koko-agent` 提供控制端 CLI 并收发画面和输入 |
| macOS 原生层 | Swift / SwiftUI 负责窗口和权限流程，调用 ScreenCaptureKit 采集、VideoToolbox 编码、AVFoundation 显示和 Core Graphics 投递键鼠 |

低内存和低延迟是项目目标。性能评估以整条链路为单位，包括控制端辅助进程、编解码和帧缓冲。[当前实现与架构规划](./docs/architecture.md#性能边界)

## 命令行与 Agent

安装后可直接运行包内的 Rust CLI。下面的第二条命令需要替换成被控端发出的实际连接码或直连邀请。

```bash
/Applications/Koko.app/Contents/Helpers/koko-agent discover
/Applications/Koko.app/Contents/Helpers/koko-agent pair ABCDE-FGHJK
```

配对成功后返回 `host_id` 和 `address`。脚本可以用这两个字段发起已授权的截图或输入请求。[CLI 与 MCP 使用](./docs/macos.md#agent-与-mcp)

## 从源码构建

在 Mac 上安装 [Rust 工具链](https://rust-lang.org/tools/install/) 和 Xcode Command Line Tools，再克隆仓库并构建。

```bash
git clone https://github.com/Kizai/koko.git
cd koko
./scripts/build-macos.sh --allow-ad-hoc
```

本地临时包输出到 `target/koko-macos/local/`。公开发布时，配置 Developer ID 签名和公证凭据，执行 `./scripts/build-macos.sh --release`，将同版本的 DMG、更新 ZIP 和 appcast.xml 附到 GitHub Release。[构建与签名](./docs/macos.md#公开分发与升级)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## 路线图与贡献

后续优先评估远控会话的资源开销，并验证公网直连与中继。Agent 侧已提供可截图、读取界面、操作键鼠和向人求助的 MCP，被控端可随时停止和接管，高危操作需被控端确认；下一步是双机实机验收和资源开销评估。

可以通过 [Issues](https://github.com/Kizai/koko/issues) 提交反馈，附上 KoKo 与 macOS 版本、连接方式、复现步骤和预期结果。请省略连接码和私钥等信息。改动前请阅读[开发规则](./docs/development.md)。

## 开源协议

KoKo 使用 GNU 通用公共许可证第 3 版（GPL-3.0）。[查看许可证](./LICENSE)
