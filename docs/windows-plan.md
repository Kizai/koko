# Windows 兼容开发方案

[返回文档目录](./README.md)

本文评估 KoKo 移植到 Windows 的工作量与风险，给出分阶段实施计划。阶段 0 和 0B 不需要 Windows 实机，可以立即开始。执行者（人或 Codex）在动手前先读 [开发规则](./development.md)、[架构设计](./architecture.md#当前实现macos-内测版) 和 [Agent 与 MCP 设计规范](./agent.md)。本文的每个任务都按一个 PR 的粒度拆分，附通过条件。

## 1. 现状评估

### 1.1 Rust 核心基本可移植

依赖的 iroh、mdns-sd、tokio、rmcp、blake3、if-addrs 都支持 Windows。协议（`koko-proto`）、配对、授权、仲裁器、审计、outbox、剪贴板流式传输都是平台无关的逻辑。阻碍 Windows 运行的只有以下几处：

| 位置 | 问题 | 影响 |
| --- | --- | --- |
| `crates/koko-daemon/src/desktop.rs` `serve_video` | 视频通道是 `#[cfg(unix)]` 的 Unix socket，非 Unix 直接返回 `desktop_unavailable` | Windows 被控端无法推流 |
| `crates/koko-agent/src/main.rs` `koko_directory()` | 只读 `HOME` 环境变量 | Windows 上 `pair`、`mcp` 默认身份路径直接报错 |
| `crates/koko-p2p/src/clipboard.rs` 接收文件 | 路径校验 `koko_proto::safe_relative_path` 只按 `/` 切分，并检查 `.`、`..`、空段和 NUL | **安全问题**：在 Windows 上，`..\..\x` 或 `C:\x` 这样的单个分段能通过校验，`Path::join` 后会逃出接收目录或变成绝对路径 |
| `crates/koko-p2p/src/clipboard.rs` 测试 | 使用 `std::os::unix::fs::symlink` / `PermissionsExt`，未加 `cfg(unix)` | Windows 上 `cargo test` 编译失败 |
| `crates/koko-daemon/src/desktop.rs` `execute` / `native` | CLI 独立模式依赖 macOS 的 `koko-desktop` | Windows 上独立 daemon 只能返回 `desktop_unavailable`，可接受 |
| `crates/koko-capture`、`crates/koko-input` | 前者只有 macOS 权限探测，后者是空 crate | 正是 Windows 平台层的落点 |

协议本身已为跨平台留了口子：`PointerEvent` 的 `source_os`（`KeyboardPlatform::Windows`）、mDNS TXT 的 `os` 字段（取 `std::env::consts::OS`），以及 macOS 端已有的 Windows 键位映射（`apps/koko-macos/DesktopInput.swift` 的 `windowsShortcut`）。

### 1.2 主要工作量在原生层

macOS 的原生层是约 8000 行 Swift（`apps/koko-macos/`），承担了以下职责，Windows 需要逐项重做：

- **被控端执行器**：按行读取 daemon 输出的 `Desktop request`，执行显示器枚举、截图、H.264 推流、音频、键鼠、剪贴板、UI 树、壁纸、音量、Agent 高危规则检查，再写回 `desktop-reply`。
- **控制端编排**：为不同用途启动多个 `koko-agent` 子进程，解码显示视频，合并键鼠事件队列，按 RTT 自适应画质，以及剪贴板和 outbox 的流程（集中在 `LocalConnection.swift`）。
- **应用外壳**：配对和审批界面、设置、Agent 指示条、托盘或后台运行、开机自启、保持唤醒、自动更新。

### 1.3 协议中只存在于 Swift 代码里的部分

以下格式目前没有写进文档，Windows 实现必须与之逐字节兼容：

- 视频记录类型 5、6、7（`audioFormat`、`audioFrame`、`syncFrame`）的载荷格式。当前音频优先 AAC-ELD、回退 AAC-LC，格式记录带 magic cookie，音频包带序号；类型 7 为历史带时间戳视频帧，仅保留接收兼容。源码依据及准确布局已补入 `architecture.md`。
- 每种 `Desktop request` 对应的回复 JSON，例如 `displays` 回复里的 `host_name`、`video`、`clipboard`、`clipboard_stream` 能力标记和 `control`。
- `pointer` 中的 `scroll` 单位（macOS 用像素单位滚动，`wheel1 = dy`，`wheel2 = dx`），以及修饰键位（Core Graphics 修饰位右移 16 位，掩码 `0xbf`）。
- `UiNode.role` 使用 AX 角色名（如 `AXButton`），`AXMenuBar` 会被跳过。

**第一步先把这些格式写进 `docs/architecture.md`**，再按文档实现 Windows 端。不要让 Windows 实现去模仿 Swift 的内部细节。

## 2. 三种方案

| 方案 | 做法 | 成本与收益 | 适用条件 |
| --- | --- | --- | --- |
| **A. Rust 平台层 + Tauri v2 外壳（推荐）** | 采集、编码、键鼠、剪贴板、UIA 用 Rust 的 `windows` crate 写进 `koko-capture` / `koko-input`。被控端执行器沿用现有的 daemon 行协议。界面用 Tauri v2（WebView2），控制端视频用 WebCodecs 硬解。 | 单一工具链，`cargo test` 和 clippy 可直接在 `windows-latest` CI 上跑，Codex 最擅长在这种环境下工作。平台层以后可以复用到 Linux。这也符合架构文档原定的 `koko-input` 和 Tauri 规划。代价是 Windows API 的 COM 调用在 Rust 里比较繁琐，WebView 渲染视频需要实测延迟。 | 希望长期维护一套代码，后续还要做 Linux |
| B. C# / .NET 8 + WinUI 3 原生外壳 | 结构完全对标 Swift 应用：原生外壳通过子进程行协议驱动 `koko-daemon` / `koko-agent`，平台 API 用 CsWinRT 和 Vortice（D3D11、Media Foundation）。 | Windows 界面体验最好，D3D 渲染和 Media Foundation 的资料丰富。代价是仓库变成三种语言、三套工具链，平台层无法复用到 Linux，Codex 需要同时维护 Swift、C# 和 Rust 三份对等逻辑。 | 首要目标是 Windows 控制端的原生体验 |
| C. 只做 Windows 被控端（无界面框架） | 只完成下文的阶段 0 和阶段 1：一个 Rust 写的 Windows 被控端程序（控制台或托盘），由 Mac 或 MCP Agent 控制 Windows。 | 最快拿到可用结果，大约是 A 的三分之一工作量。Windows 电脑不能反过来控制别人，审批界面也很简陋。 | 近期目标只是「Agent / Mac 操作 Windows 电脑」 |

**采用 A，按阶段推进，阶段 1 结束时即得到方案 C 的成果。** 方案 C 的产出就是方案 A 的前两个阶段，所以选 A 不浪费任何工作。

下列证据出现时，方案需要调整：

- 阶段 2 原型中，WebView2 + WebCodecs 在 1080p60 下端到端延迟比 macOS 控制端高出 30 ms 以上，或者持续掉帧：改为 Rust 侧用 D3D11 交换链渲染到原生子窗口，Tauri 只负责非视频界面。
- 若决定 Windows 控制端界面体验优先于代码复用：改用方案 B 的外壳，平台层仍保留 Rust 实现，以 DLL 或子进程的形式供 C# 调用。

## 3. 需要用户先确认的决定

以下几项会改变范围或接口，默认值按推荐填写。若用户没有另作说明，按默认执行。

| 决定 | 默认 | 说明 |
| --- | --- | --- |
| 最低系统版本 | Windows 10 21H2（19044）及以上，Windows 11 为主测平台 | Windows.Graphics.Capture 要求 1903 及以上；`WDA_EXCLUDEFROMCAPTURE`（把指示条排除在画面外）要求 2004 及以上；去掉采集黄框要求 Windows 11 |
| 数据目录 | `%USERPROFILE%\.koko\` | 与 macOS 的 `~/.koko/` 同构，文档和 MCP 配置可以通用。备选是 `%APPDATA%\Koko\` |
| 锁屏、UAC 安全桌面、登录界面 | 本轮不支持 | 支持这些需要 Windows 服务运行在 session 0，并在 winlogon 桌面拉起辅助进程，工作量与阶段 1 相当，放到后续阶段 |
| 管理员权限窗口 | 本轮不支持注入，但要明确报错 | 受 UIPI 限制，普通权限进程注入不了高权限窗口；要支持需 `uiAccess=true`，前提是代码签名并安装到 Program Files |
| 代码签名 | 先不签，记录 SmartScreen 和 Defender 的表现 | 屏幕采集加输入注入的未签名程序容易被安全软件误报 |
| 设备工具（SSH、终端、端口转发） | 本轮不做 | Windows 作为被控端需要 OpenSSH Server；作为控制端可直接用系统自带的 OpenSSH 客户端 |

## 4. 开发环境与执行顺序

按「是否需要 Windows 实机」安排顺序，先做不需要实机的部分。

| 阶段 | 开发环境 | 验证方式 |
| --- | --- | --- |
| 0、0B | 任意电脑（Mac 或云端 Codex 均可） | GitHub Actions 的 `windows-latest` 跑 fmt、clippy、test；0.6 的协议样例需要用 Mac 实机采集 |
| 1、2、3 | 真实的 x64 Windows 电脑，建议直接在上面运行 Codex CLI | 实机验收 |

Windows 实机的要求：x64 处理器，带独立显卡或 Intel 核显，接两块缩放比例不同的显示器（例如 100% 和 150%）。不要用 Apple Silicon Mac 上的 ARM 版 Windows 虚拟机，也不要通过远程桌面（RDP）测试采集和编码：这两种环境没有真实的硬件编码器，显示驱动也不同，结果不可信。阶段 1 完成后，可以从 Mac 用 KoKo 本身控制这台电脑来调试。

## 5. 分阶段实施

### 阶段 0：工作区跨平台化（纯 Rust，CI 可验证）

| 任务 | 内容 | 通过条件 |
| --- | --- | --- |
| 0.1 CI | 新增 `.github/workflows/ci.yml`，矩阵为 `macos-latest` 和 `windows-latest`，执行 `development.md` 中的 fmt、clippy、test 三条命令 | 两个平台都通过 |
| 0.2 测试可编译 | `clipboard.rs` 里依赖 Unix 的测试加 `#[cfg(unix)]`。符号链接跳过和不可读文件这两条用例，在 Windows 上用等价方式各补一条（符号链接创建需要开发者模式，不满足时跳过并注明原因） | Windows 上 `cargo test --workspace --locked` 通过 |
| 0.3 数据目录 | 在 `koko-p2p` 增加一个函数，返回 `~/.koko`（Unix 用 `HOME`，Windows 用 `USERPROFILE`），`koko-agent` 的 `koko_directory()` 改为调用它 | Windows 上 `koko-agent pair`、`koko-agent mcp` 无需显式传入密钥路径 |
| 0.4 **剪贴板路径安全（P0）** | 接收端在 Windows 上额外拒绝：含 `\`、`:`、`<>"|?*` 和控制字符的分段；保留设备名（`CON`、`PRN`、`AUX`、`NUL`、`COM1`–`COM9`、`LPT1`–`LPT9`，不区分大小写，含扩展名形式）；以点或空格结尾的分段。拼接后再确认路径仍位于接收目录内。不合法时整批作废，返回 `clipboard_invalid`。错误码分两层：协议层（`koko_proto::safe_relative_path`，含 NUL、`.`、`..`、空段、绝对路径）在请求入口校验，所有平台都返回 `invalid_command`，保持不变；只有通过协议校验、但 Windows 文件系统不接受的名称才返回 `clipboard_invalid`。是否把 `\` 和 `:` 也加进跨平台的 `safe_relative_path`，在 PR 里给出影响评估，由用户决定 | 新增单元测试覆盖上述每一类输入，在 Windows CI 上运行 |
| 0.5 视频通道 | 把 `serve_video` 的 Unix socket 抽成「本机私有视频通道」，Unix 用现有实现，Windows 用 `tokio::net::windows::named_pipe`：管道名随机生成（`\\.\pipe\koko-video-<随机>`），设置 `first_pipe_instance(true)` 和 `reject_remote_clients(true)`，并**显式指定只允许当前用户 SID 访问的安全描述符**（Windows 管道的默认 DACL 会给 Everyone 读权限）。`Desktop request` 的 `video_socket` 字段在 Windows 上承载管道名，字段名不变。实现 Windows 视频通道时移除 relay_feedback 上的 allow(dead_code)。 | 集成测试：一个模拟应用写入视频记录，经 QUIC 原样到达订阅端；另一用户身份无法连接管道（若 CI 不便测试，写进实机验收清单） |
| 0.6 补全协议文档 | 本轮先将 1.3 节的格式、Desktop 实际回复与当前源码差异写进 `docs/architecture.md`；按 [协议样例采集](./protocol-capture.md) 交由用户在两台 Mac 上采集。拿到真实输出后再增加 `koko-proto` 的 displays、ui_tree、音视频样例测试，不手工编造样例 | 本轮验收文档；实机采集与样例测试后续补齐 |
| 0.7 子进程生命周期 | 记录 Windows 上的约定，供阶段 1 使用：外壳启动 daemon 和 agent 时使用 `CREATE_NO_WINDOW`，并放进设置了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object，保证外壳退出时子进程一并退出 | 文档说明 |

阶段 0 的总验收：在 Windows 上运行 `koko-daemon serve-pair %USERPROFILE%\.koko\host.key 0.0.0.0:27688`，Mac 的 Koko.app 能通过局域网发现它并用连接码配对，截图请求返回 `desktop_unavailable`。这一步证明网络、mDNS 和防火墙路径都是通的，需要在 Windows 防火墙放行 UDP 27688 和 5353，并且网络配置文件为「专用网络」。

#### 0.7 Windows 子进程生命周期约定

本节只确定阶段 1 被控端和阶段 2 应用外壳的实现约定；当前 PR 不增加进程管理代码。

外壳启动 `koko-daemon.exe`、`koko-agent.exe` 时使用 `CREATE_NO_WINDOW`，标准输入、输出和错误仍通过各自的重定向管道连接。该标志用于控制台程序，不能与 `CREATE_NEW_CONSOLE` 或 `DETACHED_PROCESS` 混用；隐藏控制台不负责进程回收。[Microsoft：进程创建标志](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)

外壳创建并持有一个不继承的 Job Object 句柄，通过 `SetInformationJobObject(JobObjectExtendedLimitInformation)` 设置 `BasicLimitInformation.LimitFlags` 中的 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。最后一个 Job 句柄关闭时，系统终止关联进程；因此不能把该句柄继承或复制给子进程，也不启用 breakaway。普通创建的后代进程默认属于同一 Job。[Microsoft：Job Object](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)、[终止标志](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information)

外壳先建立 Job 并设置限制，再将 Job 句柄加入 `STARTUPINFOEXW` 的 `PROC_THREAD_ATTRIBUTE_JOB_LIST`，调用 `CreateProcessW` 时同时指定 `EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW`。Windows 10/11 支持在创建进程时关联 Job，外壳在启动期间被终止也不会留下已创建但尚未归属的子进程。创建成功后，关闭不再使用的线程句柄和父进程中留给子进程的管道副本，保留进程句柄以等待退出并读取退出码。设置 Job、准备启动属性或创建进程失败时，报告原始 Win32 错误并释放本次启动占用的资源。[Microsoft 进程创建属性](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)

正常退出时先结束会话并关闭对应输入管道，在宽限期内等待子进程退出。无论子进程是否在宽限期内退出，外壳都关闭最后一个 Job 句柄，随后等待仍未退出的直接子进程终止，再释放进程句柄。宽限期时长在实现阶段确定。外壳崩溃或被终止时，系统通过关闭最后一个 Job 句柄回收子进程。关闭窗口并选择后台运行时，外壳继续持有 Job。外壳本身可能已在启动器或 CI 的 Job 中。Windows 10/11 支持嵌套 Job，启动仍须检查 `CreateProcessW` 的结果，并遵守上层 Job 的限制。

阶段 1/2 实机验收覆盖正常退出、子进程在宽限期内未退出、外壳崩溃，以及在启动期间和已启动后强制结束外壳。每种退出场景都应确认无 daemon、agent 或其后代残留。后台运行时会话保持，重启外壳可重新绑定端口与本机通道，启动失败无挂起子进程，从已有 Job 的启动器启动时正常归属或给出明确错误。覆盖正常用户桌面和含空格的安装路径，确认不弹控制台且行协议仍双向工作。

### 阶段 0B：可脱离 Windows 实机的平台逻辑

阶段 1 中有一部分是纯计算或纯协议逻辑，不调用 Windows API，可以先写成平台无关的函数，用单元测试锁定行为，在任何电脑上开发、在两个平台的 CI 上验证。到了阶段 1 只需要把它们接到 Windows API 上。这些代码放在阶段 1 将要使用的 crate 中（`koko-host-windows`、`koko-capture`、`koko-input`），不加 `cfg(windows)`，以便在所有平台上测试。

| 任务 | 内容 | 通过条件 |
| --- | --- | --- |
| 0B.1 行协议驱动 | 新建 `crates/koko-host-windows`：启动 `koko-daemon serve-pair ... --app-desktop`，解析 daemon 的输出行（`Desktop request`、`Pair request`、`Agent control` 等），把请求交给「桌面后端」，再写回 `desktop-reply` 等命令。桌面后端定义为 trait，实现只有两个：阶段 1 的 Windows 实现，以及测试用的假实现（对所有请求返回固定结果）。这里引入 trait 的依据是真实存在两个实现，且能让整条行协议在 CI 上用真实 daemon 进程测试 | 集成测试：真实的 `koko-daemon` 加假后端，由 `koko-agent` 发起配对和 `displays`、`capture` 请求，得到假后端返回的结果；在 macOS 和 Windows CI 上都通过 |
| 0B.2 坐标换算 | 0–9999 协议坐标与显示器物理像素矩形（含负坐标、多显示器、不同缩放）之间的双向换算；`SendInput` 虚拟桌面绝对坐标（0–65535）的换算；UI 树元素边框按所在显示器归一化 | 单元测试覆盖主副屏、副屏在左侧或上方（负坐标）、150% 缩放等组合 |
| 0B.3 键位映射 | Mac 虚拟键码到 Windows 扫描码（含扩展键标志）的对照表；Windows 虚拟键码到 Mac 键码的反向表（供阶段 2 控制端使用）；修饰位（Core Graphics 修饰位右移 16 位）与 Windows 修饰键的互转，以及 `source_os` 两种取值下的映射规则（见 1.2） | 单元测试：两张表互为逆映射（不可逆的键单独列出）；常见快捷键（复制、粘贴、切换应用、截图）的映射结果符合预期 |
| 0B.4 滚动换算 | 协议中的像素滚动量与 `WHEEL_DELTA` 的换算，方向约定写进文档 | 单元测试；方向在阶段 1 实机复核 |
| 0B.5 H.264 封装 | Annex B 到 AVCC 的转换，从码流中提取 SPS/PPS 并生成 `format` 记录；视频记录的读写（长度、类型、载荷） | 单元测试使用 0.6 采集的 macOS 实机样例，生成的 `format` 记录与 macOS 字节一致 |
| 0B.6 音频封装 | AAC 的 AudioSpecificConfig（magic cookie）生成，5、6、7 号记录的载荷编码 | 单元测试使用 0.6 采集的样例，编码结果能被 macOS 的解码逻辑接受（字节级比对） |
| 0B.7 UI 树映射 | UI Automation 的 ControlType 到 AX 角色名的映射表；遍历结果展平为 `UiNode` 列表的逻辑（父节点索引、截断、密码框去值），输入用简单的树结构表示 | 单元测试 |
| 0B.8 高危规则匹配 | Windows 侧的规则判定：进程名或 AUMID 匹配、快捷键规则、关键词规则，读取同一份 `agent-policy.json`，含 Windows 默认值 | 单元测试覆盖 `agent.md` 中列出的每类规则 |

阶段 0B 的总验收：上述测试在 macOS 和 Windows CI 上全部通过；阶段 1 的各任务只剩下调用 Windows API 和实机验收。

### 阶段 1：Windows 被控端执行器（Mac 作为控制端验收）

需要 Windows 实机。在阶段 0B 建好的 `crates/koko-host-windows` 上增加 Windows 桌面后端和可执行入口 `koko-host`。它启动 `koko-daemon serve-pair ... --app-desktop <设备名>`，按[应用与辅助进程的接口](./architecture.md#应用与辅助进程的接口)读写行协议，逻辑对应 `LocalConnection.swift` 的被控端部分。本阶段的审批交互用控制台完成：打印连接码，输入 `approve` / `decline`。平台实现放进 `koko-capture` 和 `koko-input` 的 `#[cfg(windows)]` 模块，macOS 的现有代码不动。

进程需要声明 Per-Monitor DPI Aware V2，所有坐标按物理像素和显示器矩形换算。

| 任务 | 内容 | 通过条件 |
| --- | --- | --- |
| 1.1 显示器与截图 | `displays`：`EnumDisplayMonitors` 结合 DisplayConfig 取友好名称，`id` 用稳定值（例如设备路径哈希后截成 u32），不要用枚举序号。`capture` / `screenshot`：Windows.Graphics.Capture 单帧采集，经 WIC 编码为 PNG 或 JPEG，`cursor` 参数对应 `IsCursorCaptureEnabled`。`wallpaper`：通过 `IDesktopWallpaper` 读取主显示器壁纸并转成 JPEG | Mac 控制端能看到 Windows 的设备卡片和封面；在 100% 与 150% 缩放的双屏组合下截图比例正确 |
| 1.2 键鼠 | `pointer`：`SendInput` 搭配 `MOUSEEVENTF_ABSOLUTE \| MOUSEEVENTF_VIRTUALDESK`，0–9999 坐标映射到对应显示器的矩形。`scroll` 把像素换算成 `WHEEL_DELTA` 倍数（可用高精度滚轮的任意值），方向要实测对齐。键盘：建立「Mac 虚拟键码 → Windows 扫描码」对照表，用 `KEYEVENTF_SCANCODE` 发送，注意扩展键标志，确保与键盘布局无关。修饰键：`source_os=windows` 时 Command→Win、Option→Alt 原样还原；`source_os=macos` 时默认把 ⌘ 映射为 Ctrl，做成可关闭的设置项，与 macOS 的「跨系统键位映射」对称。`text` 用 `KEYEVENTF_UNICODE`，正确处理代理对。收到 `Pointer closed` 或 `release` 时释放所有按住的键和按钮。前台是高权限窗口时（比较前台进程的完整性级别），返回 `input_failed` 并在被控端提示原因；若要新增错误码，先在 PR 中说明 | Mac 控制 Windows：拖动窗口、右键菜单、双击、滚动、Ctrl+C / Ctrl+V / Alt+Tab、中文与 emoji 文本输入都正确；对高权限窗口操作时有明确提示 |
| 1.3 视频 | Windows.Graphics.Capture 输出 D3D11 纹理，送入 Media Foundation H.264 编码器（优先硬件 MFT，通过 DXGI Device Manager 零拷贝；不可用时回退到软件 MFT）。设置 `CODECAPI_AVLowLatencyMode`、关闭 B 帧、最长 2 秒一个关键帧。把 MF 输出的 Annex B 转成 AVCC，提取 SPS/PPS 写成 `format` 记录，格式变化时重发。按所选 `edge` 缩放，`fps` 档位对应采集的最小帧间隔。用 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 把 KoKo 自己的窗口排除在画面外 | Mac 端稳定显示 1080p60；切换显示器、改变分辨率、锁定再解锁后都能恢复；记录被控端 CPU、内存、编码器类型和接收帧率；另一用户无法连接管道：同机另一普通用户以 In、Out、InOut 连接均须明确拒绝访问（超时或找不到管道不算通过），当前用户仍能正常传输 |
| 1.4 剪贴板 | `clipboard_read`：模拟 Ctrl+C，读取 `CF_UNICODETEXT` 或 `CF_HDROP`。`clipboard_target`：在 `%LOCALAPPDATA%\Koko\Received\<transfer>` 新建目录。`clipboard_publish`：写入 `CF_HDROP`（附带 `Preferred DropEffect = copy`）或文本，再模拟 Ctrl+V。文件字节仍由 daemon 读写 | Mac 与 Windows 之间双向传输文字、单个大文件（大于 4 GB）、含中文名和空文件夹的目录；含非法字符的 Mac 文件名得到明确的错误提示 |
| 1.5 音频与音量 | WASAPI loopback 采集，经 MF AAC-LC 编码，按 0.6 节写进文档的格式发送 5、6、7 号记录。`audio_volume` 通过 `IAudioEndpointVolume` 读取和设置系统音量 | Mac 端声画同步，误差在 macOS 互控的水平以内；双向调节音量有效 |
| 1.6 本机输入检测 | 用 `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 钩子监听本机实体输入，按 `LLMHF_INJECTED` / `LLKHF_INJECTED` 过滤掉自己注入的事件，检测到真实输入时向 daemon 写 `local-input` | Agent 操作期间动一下本机鼠标，Agent 即被暂停 |
| 1.7 outbox | 暂只提供命令行入口 `koko-host send <路径...>`，写 `outbox {"paths":[…]}`。拖拽发送浮窗放到阶段 2 | Mac 控制端能收到 Windows 发出的文件 |

阶段 1 的总验收：按 [macOS 双机测试](./macos.md) 的人工远控清单，用 Mac 控制 Windows 全部通过；`koko-agent mcp` 能对 Windows 被控端执行 `koko_screenshot`（UI 树和高危审批在阶段 3 完成，此前 Agent 的输入按现有规则会被拒绝，因为没有策略检查）。

### 阶段 2：Windows 应用与控制端（Tauri v2）

新建 `apps/koko-windows/`，用 Tauri v2。Rust 后端以库的形式链接 `koko-host-windows`，前端负责界面。

| 任务 | 内容 | 通过条件 |
| --- | --- | --- |
| 2.1 外壳 | 主窗口（本机共享开关、连接码、直连邀请、设备列表、配对审批弹窗）、设置（常规、安全、清除信任、跨系统键位映射）、托盘图标、关闭窗口后后台运行、单实例、开机自启（`HKCU\...\Run`）、保持唤醒（`SetThreadExecutionState`）。界面文案沿用 macOS 版 | 被控端的全部操作都能在界面上完成，不再依赖控制台 |
| 2.2 控制端连接 | 按 macOS 的方式为各用途启动 `koko-agent` 子进程（`discover`、`pair`、`remote-stream`、`remote-video`、`clipboard-pull` / `clipboard-push`、`outbox-pull`、`remote`），逻辑对应 `LocalConnection.swift` 的控制端部分。先保持子进程模型以对齐行为，合并成单进程留作后续优化 | 局域网发现、连接码、直连邀请三种方式都能连上 Mac |
| 2.3 视频显示 | `remote-video` 的输出经 Tauri 二进制通道送到前端，用 WebCodecs `VideoDecoder`（硬件加速）解码 AVCC，绘制到 canvas；音频用 `AudioDecoder` 和 WebAudio，按 pts 同步。根据 `path` 记录的 RTT 自适应分辨率和画质，规则与 macOS 一致。**先做一个原型测量延迟**，触发 2 节的切换条件时改为原生 D3D11 渲染 | Windows 控制 Mac 达到 1080p60，延迟与 Mac 互控相近 |
| 2.4 控制端输入 | 远控窗口获得焦点时安装低级键盘钩子，截获 Win 键、Alt+Tab 等系统组合键。Windows 虚拟键码映射为 Mac 键码，`source_os=windows`。鼠标坐标归一化到 0–9999。移植 Swift 的键鼠事件队列合并逻辑。远控时关闭本机 IME，让按键原样发给被控端，由被控端的输入法处理 | Windows 控制 Mac：快捷键映射、中文输入、拖拽都正确 |
| 2.5 剪贴板与发送 | 控制端的 ⌘C / ⌘V 流程改用 Ctrl+C / Ctrl+V；被控时拖拽文件弹出发送浮窗 | 与 macOS 功能对齐 |
| 2.6 安装与更新 | 用 Tauri bundler 打包 NSIS 安装包（按机器安装，以便添加防火墙规则），安装时为 `koko-daemon.exe` 和 `koko-agent.exe` 添加入站防火墙规则；用 Tauri updater 从 GitHub Releases 检查更新，更新包签名。`scripts/` 下新增 `build-windows.ps1` | 全新的 Windows 机器上安装后无需手动放行防火墙；能从旧版本应用内升级 |

阶段 2 的总验收：Windows↔Mac 双向控制、Windows↔Windows 互控都通过人工远控清单。

### 阶段 3：Agent 能力对齐

| 任务 | 内容 | 通过条件 |
| --- | --- | --- |
| 3.1 UI 树 | 用 UI Automation 遍历前台窗口的 Control View，把 ControlType 映射到 AX 角色名（Button→`AXButton`、Edit→`AXTextField` 等），保证 `koko-agent` 的格式化逻辑和 Agent 看到的词汇与 macOS 一致。密码框（`IsPassword`）不输出值；边框按所在显示器归一化 | `koko_get_ui_tree` 在记事本、资源管理器、Edge 中能返回可点击的元素 |
| 3.2 高危规则 | 沿用 `~/.koko/agent-policy.json` 的同一格式。`command_apps` 和 `protected_apps` 在 Windows 上按进程名或 AUMID 匹配，默认值换成 Windows 对应程序（`WindowsTerminal.exe`、`cmd.exe`、`powershell.exe`、`pwsh.exe`、`regedit.exe`、`mmc.exe`、`Taskmgr.exe`、`SystemSettings.exe` 等）。快捷键规则增加 Win+R、Alt+F4、Ctrl+Shift+Esc、Shift+Delete、Win+X。关键词规则按点击位置的 UIA 元素名称判断 | [Agent 设计规范](./agent.md#高危操作审批)中的审批场景在 Windows 上全部通过 |
| 3.3 指示条与停止 | 置顶的分层窗口作为指示条，用 `WDA_EXCLUDEFROMCAPTURE` 排除在采集画面外。提供停止、让 Agent 继续、求助卡片、审批按钮。停止快捷键用 `RegisterHotKey` 注册 Ctrl+Alt+Win+.，先确认与系统快捷键不冲突 | 指示条不出现在 Agent 的截图里；快捷键在任意应用中都能停止 Agent |

阶段 3 的总验收：用 Claude Desktop 通过 MCP 操作 Windows 被控端，跑通 `agent.md` 中 M2 至 M5 的验收项。

### 后续阶段（不在本轮）

锁屏、UAC 安全桌面和登录界面（需要 Windows 服务）；`uiAccess` 与代码签名；Windows 作为被控端的设备工具（OpenSSH Server）；控制端合并为单进程、单条 QUIC 连接；跨公网直连与中继。

## 6. 风险与已知限制

| 风险 | 应对 |
| --- | --- |
| Windows 10 上 Windows.Graphics.Capture 有黄色边框 | 写进说明；Windows 11 上设置 `IsBorderRequired = false`。若必须去掉，评估 DXGI Desktop Duplication 作为 Windows 10 的备选路径 |
| 双显卡笔记本上硬件编码器与采集所在的 GPU 不一致 | 用采集纹理所在的 D3D 设备创建编码器；失败时回退到软件 MFT，并在设置里显示当前使用的编码器 |
| 软件 MFT 在高分辨率下达不到 60 FPS | 回退时自动降到 30 FPS 和较低分辨率 |
| 安全软件误报（未签名、截屏、注入输入） | 阶段 1 起记录 Defender 的行为；正式发布前申请代码签名证书 |
| 网络配置文件为「公用网络」时 mDNS 被拦截 | 界面上提示切换为「专用网络」，并引导使用直连邀请 |
| Windows 默认 Ctrl+Alt+Del 无法通过 `SendInput` 发送 | 本轮不支持，界面上不提供该按钮 |
| 跨平台文件名不兼容（Mac 文件名可以含 `:` 等字符） | 按 0.4 节作为整批错误返回；是否改为自动重命名，待用户决定 |

## 7. 给执行者（Codex）的约定

1. 动手前读 `AGENTS.md` 和 `docs/development.md`，严格遵守其中的验证与文档规则。
2. **不修改 Swift 代码**。改动共享 Rust 代码（`koko-proto`、`koko-p2p`、`koko-daemon`、`koko-agent`）时，必须保证 macOS 行为不变，并在 macOS 和 Windows 两个平台的 CI 上都通过。
3. 每个任务一个 PR，按本文编号命名（例如 `windows: 0.4 剪贴板路径安全`）。PR 描述写明目标、改动、自动化验收结果，以及**仍需实机验证的场景**。
4. 协议格式以 `docs/architecture.md` 为准。发现 Swift 实现与文档不一致时，先在 PR 里指出，不要自行选择其中一方。
5. 任何新增错误码、新增命令字段或数据目录的变化，都属于接口变更，需要先说明影响、经用户同意后再实现，并同步更新文档。
6. 每个阶段结束时更新本文的状态，并在 `docs/README.md` 的索引中保持本文可以找到。

## 8. 进度

| 阶段 | 状态 |
| --- | --- |
| 0 工作区跨平台化 | 本轮代码与文档已提交独立 PR；实机样例、权限及总验收仍待执行，详见下表 |
| 0B 可脱离实机的平台逻辑 | 未开始 |
| 1 Windows 被控端 | 未开始 |
| 2 Windows 应用与控制端 | 未开始 |
| 3 Agent 能力对齐 | 未开始 |

阶段 0 的本轮交付按用户缩减后的范围执行，不能把 CI 通过等同于全部 Windows 产品能力可用。阶段 0B 尚未开始，后续须先汇报并由用户决定继续。

| 阶段 0 任务 | 交付与剩余验证 |
| --- | --- |
| 前置编译修复、0.2、0.1、方案文档 | #4、#1、#2、#5 已合入；0.1/0.2 均经完整 macOS / Windows CI；阶段名及任务编号已改为 0B、0B.1–0B.8 |
| 0.3 数据目录 | #6 已合入，两平台完整 CI 通过；真实 Windows↔Mac 配对待总验收 |
| 0.4 剪贴板路径安全 | #3 已合入，rebase 后完整两平台 CI 通过；真实应用双向文件传输待阶段 1 |
| 0.5 视频通道 | #7 提交实现与真实 QUIC / 实际管道 DACL 测试；另一用户身份拒绝访问及真实应用接入仍需 Windows 实机，见 PR 清单 |
| 0.7 生命周期 | #8 提交文档约定；创建、退出与崩溃清理的代码和实机验收属于阶段 1/2 |
| 0.6 协议 | 本 PR 交付源码核对后的格式与采集步骤；Mac 实机原始样例及相应测试待用户采集后补齐。已记录 audio_volume 缺字段的现存问题，未改 Swift |

本轮到此停止。阶段 0 总验收中的真实 Windows 网络、防火墙与跨设备场景、跨用户管道访问，以及两台 Mac 的协议样例均未冒充已完成。
