# macOS 双机测试

[返回文档目录](./README.md)

将同一个带版本号的 DMG 发给两台 Mac。可在「设置 → 常规」查看版本号。连接后先确认控制端能看到被控端主屏并操作一个测试窗口。

## 安装与配对

1. 退出正在运行的旧版。两台 Mac 各自打开 0.1.15 或更新版本的 DMG，双击安装图标并完成安装，然后从「应用程序」打开 KoKo。首次打开会显示权限引导。
2. 在要分享的 Mac 上查看录屏和辅助功能权限，再打开「本机共享」里的「允许远程协助」。应用默认绑定 `0.0.0.0:4444/UDP`，可以调整端口。
3. **授权码连接**。另一台 Mac 在「连接伙伴」输入短码或粘贴组网直连邀请。点击「连接」后打开远控窗口。
4. **局域网搜索连接**。用侧栏刷新并选中设备，在详情页点「进入桌面」。首次连接由被控端确认。选择「信任并连接」后，同一设备可在本机共享开启时直接连接。选择「仅允许这次」则逐次确认。

每次打开本机共享都会生成一个新的 10 位码，有效期五分钟，允许切换显示器、查看和控制键鼠。会话开始后短码失效，局域网发现随本机共享保持开启。控制端从「控制中心」结束本次会话。被控端关闭「允许远程协助」即可撤销当前会话并关闭监听，也可以用「清除信任」撤销已记住的设备。

KoKo 的键鼠转发面向已解锁的 macOS 桌面。被控端已开启系统「屏幕共享」并允许对应账户访问时，可从 KoKo 控制中心点「在 macOS 屏幕共享中打开」，再由 Apple 的「屏幕共享」App 完成认证与登录。未开启这项系统服务时，在被控 Mac 上解锁。[Apple 屏幕共享说明](https://support.apple.com/zh-cn/guide/mac-help/mh14066/mac)

## 通过组网连接

先让两台 Mac 加入组网，再在被控端启动远程协助。直连邀请会携带启动时的本机 IPv4 地址和端口，控制端尝试从中建立 QUIC 连接。短码依靠局域网自动发现，在组网没有转发发现报文时可直接使用邀请。邀请与短码共享同一次授权和有效期。

如果组网地址发生变化，重新启动远程协助并复制新邀请。若仍无法连接，在组网策略里核对控制端能否访问被控端的 UDP 监听端口，默认端口是 `4444`。

## 设备工具与设置

选中侧栏的局域网设备后，卡片提供「进入桌面」「观看模式」「文件传输」「终端」和「端口转发」。观看模式在当前远控窗口暂停向被控端发送键鼠。点「进入桌面」可以恢复控制。

终端、文件传输和端口转发需要被控 Mac 开启「系统设置 → 通用 → 共享 → 远程登录」，并允许所填账户登录。默认使用 TCP 22。首次从「终端」连接时，按系统提示核对被控 Mac 的主机指纹，再登录。文件传输与转发复用系统的已验证主机记录，以及默认位置的 SSH 密钥或 SSH Agent。[Apple 远程登录设置](https://support.apple.com/zh-cn/guide/mac-help/mchlp1066/mac)

- **文件传输**：发送时选择本机文件，可指定远端目录。接收时输入一个远端文件路径，再选择本机保存位置。相对路径以远端账户的登录目录为起点。
- **端口转发**：将控制端的 `127.0.0.1:本机端口` 转发到被控端的 `127.0.0.1:远端端口`。关闭转发窗口即结束。
- **设备菜单**：刷新发现、设置本机别名或隐藏设备。可在「设置 → 安全」恢复已隐藏的设备。
- **设置**：右上角齿轮打开设置。「常规」中可设置开机自启动、让这台 Mac 保持唤醒（永不、开启远程协助时或始终；只阻止闲置睡眠，屏幕仍可关闭，合上屏幕后仍会睡眠）、关闭窗口时隐藏到菜单栏或退出 KoKo、新会话的默认帧率、传输文件存储路径和检查更新。隐藏到菜单栏时程序坞图标也会隐藏，可从菜单栏图标重新打开主窗口。其他分类可调整实体键盘转发、共享监听端口等。首次打开「开机自启动」时，macOS 可能要求在「系统设置 → 通用 → 登录项」中允许 KoKo。

### 高级设置

在「设置 → 高级」可以分别调整以下选项。四项默认开启。

| 选项 | 生效时机与效果 |
| --- | --- |
| 增强连接 | 控制端配对建立连接时，可重试失败或过慢的握手。 |
| 局域网优先 | 同一邀请包含多个候选地址时，优先尝试与本机局域网同子网的地址，失败后可尝试其他地址。 |
| 流畅度增强 | 控制端根据持续的 QUIC 延迟调整自动画质与自适应分辨率，网络恢复后还原。手动选择的画质和分辨率各自优先。 |
| 跨系统键位映射 | 被控 Mac 收到标记为 Windows 来源的实体按键时，把 Ctrl 映射到 Command、Win 映射到 Control，Alt 对应 Option。Mac 和未标记来源的控制端沿用原键位。 |

接入 Windows 控制端时，在 `pointer` 请求中设置 `source_os` 为 `windows`。实体按键的 `code` 使用现有协议的 Mac 虚拟键位，`flags` 对应 Core Graphics 修饰位向右移动 16 位；发送端以 Control 表示 Ctrl，以 Command 表示 Win，以 Option 表示 Alt。映射只由被控端执行一次。

## 权限

设备私钥保存在各自 Mac 的 `~/.koko/` 下。被控端可随时停止监听，退出应用也会停止。

若 macOS 提示本地网络权限，在参与测试的 Mac 上允许 Koko 访问局域网。开启本机共享时，Koko 会显示缺少的录屏和辅助功能权限。用引导中的按钮发起系统申请。连接前可以点「测试本机画面」，确认当前 Koko 能采集主屏。若系统开关已开但应用仍显示「未授权」，按权限文档处理旧授权记录。[macOS 权限设计](./macos-permissions.md#包内权限归属)

## 画面与按键映射

远控窗口用一行显示窗口按钮、被控端名称、显示器、会话网络和控制中心。显示器列表可重新检测被控端已接入的屏幕。切屏时保留上一帧，收到目标屏幕的新帧后恢复点击。会话网络显示 QUIC 延迟、首帧耗时、实测接收帧率和画面数据速率。

两端使用 0.1.9 或以上版本时，远控走连续 H.264 推流。「控制中心」提供 30、60、90、120、144、165 和 240 FPS 目标档位，默认 60 FPS。实际接收帧率取决于屏幕更新、编解码和网络。验证时让远端持续滚动或播放动画，等会话网络面板更新。自适应分辨率按窗口可用像素调整，传输画面的长边上限为 4096 像素。旧版被控端仍使用按需 JPEG。

远控画面隐藏被控端光标，操作时显示控制端的光标。在图像范围内移动鼠标、点击、拖拽或滚动会操作选中的远端显示器。点击画面获得键盘焦点，实体键盘按键按被控端的键盘布局输入。被控端需要授予辅助功能权限。系统快捷键及 KoKo 的关闭窗口、退出命令可由控制端处理。「控制中心」也提供快捷键和文本输入。

| 控制端操作 | 被控端动作 |
| --- | --- |
| 回车、Esc、Tab、删除、空格、方向键 | 按对应的 macOS 键，删除为 Backspace |
| ⌘A、⌘C、⌘V、⌘Z | 在被控端执行全选、复制、粘贴和撤销 |
| 输入文本并点「发送文本」 | 向当前聚焦的远端应用发送文本 |

### 跨设备复制与粘贴

两端安装 0.1.22 或更新版本后，点击远程画面取得焦点。复制和粘贴也可从「控制中心 → 快捷键」发起。

- **本机到远端**。在本机复制文字或 Finder 中的文件、文件夹，在远端文本框或 Finder 目标文件夹按 ⌘V。
- **远端到本机**。在远端选中文字或文件、文件夹并按 ⌘C。等画面右下角的进度消失，回到本机的目标位置按 ⌘V。
- 文件和文件夹在一条 QUIC 流中连续传输，大小不限，速度取决于网络和磁盘。画面右下角显示已传输量、总量和百分比，传输期间仍可操作远程窗口，也可以点「取消传输」。接收端在「设置 → 常规 → 接收文件位置」下为每次传输新建一个 `KoKo-…` 文件夹。纯文字每次最多 1 MiB。文件夹里的替身、符号链接等特殊文件会被跳过。
- 首次读取或写入受保护文件夹时，macOS 会在相应的 Mac 上询问，请选择允许或拒绝。被拒绝时会提示到「隐私与安全性 → 文件与文件夹」中调整 KoKo 的目录权限，调整后再复制一次。
- 观看模式暂停剪贴板快捷键。被控端低于 0.1.22 时，⌘C 与 ⌘V 只操作远端自己的剪贴板。

验收大文件：在被控端复制一个 1 GB 以上的文件或一个包含多层子文件夹的文件夹，在本机按 ⌘V，确认进度持续增长、完成后文件大小和内容一致；再反方向粘贴一次。传输中途点「取消传输」，接收目录中不应留下半个文件。

### 窗口、设备与显示器验收

开发机可运行 `./scripts/test-macos-desktop.sh`，检查窗口是否实际位移，以及辅助进程读取缓慢或提前退出时界面能否继续响应。

1. 在主窗口和远控窗口分别拖动顶栏空白处，检查窗口跟随鼠标移动。双击空白处应放大到当前屏幕的可用区域，再次双击应恢复原来的位置和大小。再点设备菜单、显示器标签和控制中心，检查各按钮响应。
2. 两端刷新局域网列表。开启 KoKo 共享的主机广播系统类型，侧栏显示对应图标，悬停可见系统名称。旧版设备缺少系统字段时显示通用图标。
3. 保持被控 Mac 开机并监听，让显示器进入节能熄屏后再进入远程桌面。主机会请求 macOS 唤醒显示器，控制端重新获取画面。整台 Mac 睡眠、合盖或显示器物理断电时，需要先让设备恢复可用。

## Agent 与 MCP

`koko-agent mcp` 让 Claude Desktop 等 MCP 客户端以独立的 Agent 身份查看并操作被控 Mac。被控端需要授予录屏和辅助功能权限。在控制端 Mac 编辑 `~/Library/Application Support/Claude/claude_desktop_config.json`，加入下面的配置后重启 Claude Desktop：

```json
{
  "mcpServers": {
    "koko": {
      "command": "/Applications/Koko.app/Contents/Helpers/koko-agent",
      "args": ["mcp", "--name", "Claude Desktop"]
    }
  }
}
```

Agent 密钥默认保存在 `~/.koko/agent.key`，已配对的设备保存在 `~/.koko/agent-hosts.json`。用 `--identity <文件>` 可以为不同客户端使用不同身份。工具说明与结果码见 [Agent 与 MCP 设计规范](./agent.md#工具)。

### 验收

1. 被控端打开「允许远程协助」。在 Claude 中说「用连接码 ABCDE-FGHJK 连接我的 Mac」，换成被控端显示的实际连接码。
2. 被控端出现「Agent「Claude Desktop」请求查看并操作此 Mac」。选「信任并连接」后，Claude 收到 `paired`，被控端的本机共享卡片显示 Agent 已连接。
3. 让 Claude 截图。确认图像与被控端主屏一致；指定另一块显示器时也能截到。
4. 在被控端打开「本机权限 → 键鼠测试区」，让 Claude 点击「测试点击」，再点文本框输入 Koko，然后用 `cmd+a`、`delete` 清空。屏幕顶部应出现橙色指示条「Agent「Claude Desktop」正在操作这台 Mac」，测试区分别显示已收到点击和文本。
5. 让 Claude 在访达中拖动一个窗口，并在长页面中滚动，确认拖拽和滚动的位置正确。指示条不应出现在 Claude 收到的截图里。
6. Claude 操作期间，点指示条上的「停止」或按 ⌃⌥⌘.。指示条变为「已暂停」，Claude 的下一步操作收到 `preempted_by_human` 并停下。点「让 Agent 继续」后 Claude 可以继续。
7. 在控制端用 KoKo 进入同一台被控端的桌面，在 Claude 操作时移动鼠标。Agent 应立即暂停，远控窗口顶栏显示「Agent 已暂停」。只观看不操作时，两个会话互不影响。
8. 查看被控端的 `~/.koko/audit.log`，每条 Agent 请求一行，不包含输入的文字。
9. 被控端点「断开 Agent」，再让 Claude 截图。已信任的 Agent 应自动重连，被控端不再询问。
10. 被控端「清除信任」后再截图，Claude 应收到 `not_paired` 一类的结果，重新配对需要再次确认。
11. 用第二个 Agent 身份（`--identity` 指向另一个文件）在第一个 Agent 连接期间配对，应收到 `agent_busy`。
12. 让 Claude 打开一个需要登录的网页，并在需要输入密码时调用 `koko_request_human`。被控端屏幕顶部出现蓝色的「请你协助」和原因，程序坞图标跳动。在主窗口填写附言后点「已完成」，Claude 收到 `resolved` 和附言后继续。再试一次，分别点「我来接管」「拒绝」和指示条上的「停止」，Claude 应停下；不回应时，到期后 Claude 收到 `timeout`。
13. Claude 操作期间，直接动一下被控 Mac 本机的鼠标或按一个键，Agent 应在半秒内暂停，指示条显示「已暂停」。Claude 单纯截图、没有操作时，本机正常使用不受影响。
14. 在被控端打开访达，让 Claude 用 `koko_get_ui_tree` 读取界面，再按元素点侧栏的「下载」。确认列表里能看到按钮和文字、点击位置正确；在登录窗口的密码框里输入内容后读取，密码框不应出现值。
15. 高危操作审批：让 Claude 在访达里按 ⌘⌫ 删除一个测试文件、在终端里输入一条命令、在系统设置里点任意项。每次指示条都应变红并写明操作，「拒绝」后操作不执行且 Claude 收到 `approval_declined`，「允许一次」后操作执行。选「本次会话都允许」后，同类操作不再询问，断开 Agent 后恢复询问。查看 `~/.koko/audit.log`，批准的操作带 `approved_by_person`。

M1 时以只读方式配对的 Agent，需要更多权限时会收到 `scope_denied`。让 Claude 用该设备的 `device_id` 和 `scope: agent_safe` 再次调用 `koko_pair`，被控端确认后即可操作。

### 命令行配对

脚本也可以直接调用应用内的 CLI。同一局域网使用短码，通过组网时也可以把完整的直连邀请作为同一命令的参数：

```bash
/Applications/Koko.app/Contents/Helpers/koko-agent pair ABCDE-FGHJK
```

把示例码换成实际连接码。命令返回包含 `status`、`host_id` 和 `address` 的 JSON。这条命令使用 `~/.koko/controller.key`，被控端会把它当作人类控制端，授予完整权限。`koko-agent discover` 列出设备，使用设备的 `request` 字段进入同一套局域网授权流程。

## 构建测试包

内测版用 KoKo 自签名证书签名，直到改用 Developer ID 公开分发。macOS 按应用的指定要求（designated requirement）记住录屏和辅助功能授权。临时签名的指定要求是本次构建的哈希，每次构建都会变；自签名证书的指定要求是「应用标识 + 证书哈希」，只要一直用同一个证书，升级后授权就能延续。

### 创建自签名身份

只在第一台构建机上做一次。在装有 Xcode 命令行工具的 Mac 上，从仓库根目录运行，参数是私钥备份文件的路径，放在仓库之外：

```bash
./scripts/macos-installer/create-self-signed-identity.sh ~/Documents/KoKo-Self-Signed.p12
```

脚本在临时目录生成 10 年有效的代码签名证书和私钥，要求输入备份密码，再把身份导入登录钥匙串，并只允许 `codesign` 免提示使用这把私钥。导入时 macOS 会再请你输入一次备份密码。证书不需要在钥匙串中设为「始终信任」，所以 `security find-identity -v` 不会列出它，属正常现象。

把 `.p12` 备份和密码保存在仓库之外的安全位置。丢失后只能换新证书，所有用户都要重新授权一次。换一台构建机时导入同一份备份：

```bash
security import ~/Documents/KoKo-Self-Signed.p12 -k ~/Library/Keychains/login.keychain-db -T /usr/bin/codesign
```

### 构建

在装有 Rust 和 Xcode 命令行工具的 Mac 上，从仓库根目录运行：

```bash
KOKO_SIGN_IDENTITY="KoKo Self-Signed Code Signing" ./scripts/build-macos.sh
```

输出为 `target/koko-macos/Koko-<version>.dmg`，包含面向 macOS 13 及以上的 Apple Silicon 和 Intel 二进制。Rust 只用于构建，测试机器打开安装好的应用即可。从第二个自签名版本起，可设置 `KOKO_PREVIOUS_APP` 指向上一版的副本，让脚本核对新旧版本的指定要求相互满足。

自签名和临时签名的应用一样，从网上下载后首次打开会被 Gatekeeper 拦截，需要在「系统设置 → 隐私与安全性」中点「仍要打开」。这一步只影响首次打开，与权限能否延续无关。

内测使用 `0.1.x`。每次发一个新的迭代包时，递增根目录 `Cargo.toml` 的 `workspace.package.version` 最后一位，使用 Cargo 更新锁文件后再打包。应用内版本、CLI 的 `--version` 和 DMG 文件名都来自同一个版本。同一版本重打包使用同一个文件名。脚本校验新 DMG 后清理输出目录里的旧版 Koko DMG。

`KOKO_SIGN_IDENTITY` 接受身份全名或证书的 SHA-1，可以是自签名身份、Apple Development 或 Developer ID。要求必须使用 Apple 签发的证书时，加上 `--require-apple-signing`。已有 Developer ID Installer 身份时，可通过 `KOKO_INSTALLER_SIGN_IDENTITY` 同时签署安装包。签名身份每变一次（临时签名换成自签名，或以后换成 Developer ID），用户都需要为新身份授权一次。

只在本机临时试验、不分发给别人时，可以不用证书，显式生成临时签名包：

```bash
./scripts/build-macos.sh --allow-ad-hoc
```

在未指定签名身份时，该命令输出至 `target/koko-macos/local/Koko-<version>.dmg`。重建后的临时签名可能触发重新授权。

### 公开分发与升级

公开分发使用同一团队的 Developer ID Application 和 Developer ID Installer 证书。构建机需要能运行 `xcrun notarytool`、`xcrun stapler`，并能使用预先存入钥匙串的公证凭据配置。在下面的命令中填入证书全名和已有的配置名称：

```bash
KOKO_SIGN_IDENTITY="Developer ID Application: YOUR NAME (TEAMID)" \
KOKO_INSTALLER_SIGN_IDENTITY="Developer ID Installer: YOUR NAME (TEAMID)" \
KOKO_NOTARY_PROFILE="koko-notary" \
./scripts/build-macos.sh --release
```

脚本用加固运行时和安全时间戳签署应用与辅助程序，用 Installer 身份签署 PKG，再用 Application 身份签署 DMG。它把最终的 DMG 提交给 Apple 公证，等待接受后装订并验证票据，通过 Gatekeeper 校验后输出 `target/koko-macos/release/Koko-<version>.dmg`。[Apple 公证流程](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)

构建升级包时，可额外设置 `KOKO_PREVIOUS_APP=/Applications/Koko.app`，指向用户实际安装的上一版或其独立副本。脚本在打包前检查两版应用是否相互满足对方的指定要求（designated requirement）。首次发包可省略此项。由开发签名切换到 Developer ID 时，即使属于同一团队，默认指定要求也可能互不兼容，需要重新授权。升级后按权限文档复测录屏和键鼠。[权限验收](./macos-permissions.md#双机验收) · [Apple 签名要求](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements)

### 发布应用内更新

首次安装继续使用 DMG。0.1.21 起的应用会从所选 GitHub Release 加载该版本的 `appcast.xml`，并在解压前校验 ZIP 的 Ed25519 签名。KoKo 用自己的更新窗口代替 Sparkle 的默认界面：窗口显示 GitHub Release 中写的更新说明，点「立即更新」后一条进度条依次显示下载（可取消）、校验解压和安装，准备就绪后自动安装并重新启动。重启会结束当前会话。Release 的说明会原样显示在更新窗口中，支持标题、列表和行内格式。macOS 在安装位置需要更高权限时会提示授权。

构建脚本从 Sparkle 官方发行页下载固定版本并校验 SHA-256。公开签名构建默认生成更新附件。内测发布可执行 `KOKO_UPDATE_ASSETS=1 ./scripts/build-macos.sh --allow-ad-hoc`。把 DMG 与同目录下 `updates/<版本>/` 中的 `Koko-<版本>.zip` 和 `appcast.xml` 一起上传到 `v<版本>` 的 GitHub Release。

KoKo 的更新私钥保存在发布机登录钥匙串中，服务名为 `https://sparkle-project.org`、账户为 `io.github.kizai.koko.updates`，客户端的 `SUPublicEDKey` 固定了对应公钥。0.1.21 因原私钥遗失换用了新密钥，0.1.20 及更早版本无法应用内升级到 0.1.21，需要手动安装一次 DMG。

更新私钥和[自签名证书](#创建自签名身份)一样只有一份，两者都要备份。在发布机上导出私钥（导出的文件就是明文私钥，保存到加密存储后删除原文件）：

```bash
"$(./scripts/macos-installer/sparkle.sh)/bin/generate_keys" --account io.github.kizai.koko.updates -x ~/Documents/KoKo-Sparkle-Update-Key.txt
```

在另一台发布机上导入前，先在「钥匙串访问」中搜索 `io.github.kizai.koko.updates`，删除旧条目，再导入并核对公钥与 `Info.plist` 一致：

```bash
"$(./scripts/macos-installer/sparkle.sh)/bin/generate_keys" --account io.github.kizai.koko.updates -f ~/Documents/KoKo-Sparkle-Update-Key.txt
"$(./scripts/macos-installer/sparkle.sh)/bin/generate_keys" --account io.github.kizai.koko.updates -p
```自行构建分支并独立发布时应使用自己的发布仓库与密钥。[Sparkle 发布说明](https://sparkle-project.org/documentation/publishing/)

## 复测

- **重启**：退出被控端后重新打开，启动远程协助。控制端用新连接码发起请求。
- **授权拒绝与信任**：选中局域网设备后点「进入桌面」。拒绝应让配对失败。选「信任并连接」后，结束会话并再次连接，确认无需重复批准。清除信任后应恢复首次确认。
- **画面与输入**：切换远端显示器并缩放控制窗口，验证点击、拖拽、滚动和实体键盘。查看目标帧率和会话网络里的接收帧率。分别撤销录屏与辅助功能权限，核对各自的错误提示。
- **设备工具**：使用已允许远程登录的测试账户，收发一个文件，验证 localhost HTTP 服务能通过映射端口访问。切换观看模式，确认仍有画面且光标与键盘留在控制端。
- **调试**：需要查看 CLI 输出时，使用[命令行验收步骤](./implementation.md#身份与连接验收命令)。
