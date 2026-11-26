# Koko Phase 1 实现计划

[返回文档目录](./README.md)

首轮验收由两台已授权的 macOS 设备在同一局域网完成。控制端通过 macOS 测试应用发起一次截图请求并显示被控端主显示器的 PNG，CLI 以 JSON 返回相同请求的 Base64 PNG。双机截图验收通过后接入只读 MCP。跨网络场景使用 Tailscale / ZeroTier 等组网直连验收，KoKo 不做公网中继。

[架构设计](./architecture.md)记录 Phase 1 的完整目标。本计划把目标拆成有独立验收结果的工作项。[开发规则](./development.md)适用于各工作项。

## 现状与首轮要验证的问题

- 截至 0.1.26，工作项 1 和 2 已实现，人工远控也已提前落地，包括连续 H.264 视频、多显示器、键鼠、剪贴板、观看模式和 SSH 设备工具。工作项 4 和 6 的 Agent 部分已实现到 M5（只读 MCP、Agent 键鼠、抢占、求助、界面树和高危审批）；工作项 5 的组网验收仍待实机完成。各项状态见下文[实现顺序与验收](#实现顺序与验收)。
- 截图请求的追踪路径：`koko-agent remote` 经 `koko-p2p::remote` 发出 `koko/remote/1` 命令，`koko-daemon/src/desktop.rs` 核对授权后，以 `Desktop request` 行交给 `Koko.app`（`LocalConnection.executeDesktopAction`）。单独运行 daemon 时改由包内的 `koko-desktop` 执行。公共消息结构在 `koko-proto`。`koko-capture` 负责录屏权限探测，以及 Windows 的屏幕捕获与 UI Automation 树转换；`koko-input` 提供坐标、键位和滚动换算。
- 首轮最需要验证的是主机许可、设备身份、实际传输和系统截图能否串成一次可复现的请求。借鉴服务集成中的分段验收，先用本地真实连接定位传输和协议问题，再用双机测试定位网络和系统权限问题。

## 三种实现方案

| 方案 | 首个可验收结果 | 收益与代价 | 适用条件 |
| --- | --- | --- | --- |
| A. 先完成双机截图 | 两台 Mac 通过局域网直连，主机收到已授权请求，控制端应用显示实际 PNG | 一轮验收就能覆盖配对、协议、传输和 OS 权限。CLI 和 MCP 可复用截图请求路径。需要双机和可用的 macOS 图形会话 | 当前已确认的首轮目标 |
| B. 先完成协议和传输 | 单机两个真实 iroh Endpoint 完成身份校验和请求往返 | 最快得到可重复的自动化测试。双机网络与截图问题留到第二步暴露，较晚获得可用结果 | 暂时没有第二台 Mac，或者某个协议边界已有可复现的问题 |
| C. 先完成 MCP 截图 | MCP 客户端在单机上发现并调用截图工具 | 能较早验证工具参数和返回格式。用户操作、MCP 会话和被控端问题会同时进入调试范围，局域网授权仍需另外验收 | MCP 客户端兼容性已经成为交付约束 |

**采用 A，并把 B 的真实本地传输测试纳入 A 的第一项。** 最强反例是手工交换身份与局域网地址只能证明受控测试环境可用，无法说明上手成本或跨公网可用性。首轮记录手工操作与错误来源，跨公网再用真实网络分别验证直连和中继。若双机环境暂时不可用，先按 B 走完自动化检查，并将双机结果标记为待验收。

采用 A 后，CLI 和 MCP 可以复用一次请求路径，主机权限逻辑也集中在 daemon。反过来，若把局域网地址或 PNG 文件路径写进共享协议，后续中继和 MCP 就会被具体界面拖累。地址由传输层处理，文件由调用方写入，协议只表达远端截图请求和返回数据。当前 CLI 在标准输出返回 Base64 PNG，不写文件。

## 首轮范围与模块职责

| 模块 | 首轮职责与当前实现 |
| --- | --- |
| `koko-proto` | 声明 `koko/1` 自检帧，以及 `koko/remote/1` 的命令、回复、请求校验和大小上限。截图请求返回主显示器 PNG，不单独携带宽高，由调用方解码图像得到。 |
| `koko-p2p` | 管理本地持久化设备身份，通过 iroh 建立带公钥身份的 QUIC 连接。所有端点使用 `presets::Minimal`，不启用中继和外部地址查询，只按明确的设备标识与 IP 地址直连。 |
| `koko-daemon` | 在已登录的 macOS 图形会话中以前台模式运行。`serve` 只为一个指定控制端提供 `koko/1` 自检。`serve-pair` 通过验证码证明、局域网确认或信任列表授权控制端，并按作用域限制 Agent。根据连接认证的公钥授权后，把命令交给 `Koko.app` 或 `koko-desktop`。 |
| macOS 采集 | 图形版在 `Koko.app` 进程内检查录屏权限，通过 ScreenCaptureKit 采集所选显示器，截图编码为 PNG，远控画面编码为 H.264。CLI 独立运行时使用 `koko-desktop` 中的同一截图实现。 |
| `koko-agent` | 提供本机设备标识、授权自检、发现、配对，以及从标准输入读取 JSON 命令的单次远控命令。结果以 JSON 写到标准输出，不接受本地输出路径。后续 MCP 复用同一调用路径。 |
| `koko-input` | 提供可移植的坐标换算、Mac 与 Windows 虚拟键位映射和滚动量换算，以及 Windows 原生输入注入。macOS 键鼠由 `Koko.app` 和 `koko-desktop` 通过 Core Graphics 投递。 |

macOS 测试版通过 `koko/remote/1` 传递截图、视频、键鼠和剪贴板命令。普通命令每条双向流处理一个有长度上限的 JSON 请求和一个结果，视频订阅在同一条流上持续返回记录。daemon 在解码边界执行请求校验，并以连接认证的控制端公钥检查授权。后续 MCP 可以复用这条路径。[命令与视频格式](./architecture.md#远控命令)

被控端发出的授权码可直接开始查看和键鼠会话。从局域网结果发起远程控制时，由被控端确认。需要指定身份和地址时可使用下文的 CLI 调试入口。重启从同一身份文件读取密钥。macOS 录屏和辅助功能权限由主机所有者授予。[iroh 地址文档](https://docs.rs/iroh/latest/iroh/struct.EndpointAddr.html) · [macOS 权限设计](./macos-permissions.md)

## 实现顺序与验收

| 工作项 | 实现内容 | 通过条件 | 当前状态（0.1.20） |
| --- | --- | --- | --- |
| 1. 身份与连接 | 两端保留独立的设备身份，主机只接受显式授权的控制端。用真实 iroh 连接集成测试覆盖鉴权与协议入口。 | 本地两个 Endpoint 可以交换测试请求。重启后身份不变。换用未授权的控制端身份或指定错误的主机标识，连接或请求被拒绝。 | 已实现。`koko-daemon` 的集成测试覆盖身份复用、未授权拒绝和错误主机标识。 |
| 2. 真实截图 | 用 ScreenCaptureKit 抓主显示器，应用显示由 daemon 返回的 PNG。[macOS 权限设计](./macos-permissions.md) | 主机打开只含测试文字的窗口，控制端能看到文字和正确比例。未授权请求无法得到图像。结束会话或撤销系统录屏权限时，控制端收到可区分的错误。 | 已实现。应用内远控已改用连续 H.264 视频，`screenshot` 命令保留给 CLI。 |
| 3. 双机局域网验收 | 在两台实际 Mac 上安装同一个 DMG，记录配对、连接和截图步骤。 | 局域网验收配置禁用中继和外部地址查询。授权成功、未授权拒绝、重启复用身份三种场景通过。记录 macOS 版本、直连状态和截图尺寸。 | 步骤见 [macOS 双机测试](./macos.md)，验收记录未纳入仓库。 |
| 4. 只读 MCP | 双机截图验收后引入 `rmcp` stdio，先接 `koko_screenshot`，随后通过真实配对数据实现 `koko_list_devices`。 | MCP 客户端能发现并调用截图工具。工具返回图像，连接与授权仍走同一主机路径，诊断日志写到 stderr。[rmcp stdio 示例](https://github.com/modelcontextprotocol/rust-sdk/tree/main/examples/servers) | 已实现 M1：`koko-agent mcp` 提供 `koko_list_devices`、`koko_pair` 和 `koko_screenshot`，Agent 使用独立身份和 `observer` 作用域。自动化测试与本机端到端配对已通过，Claude Desktop 双机截图待实机验收。设计见 [Agent 与 MCP 设计规范](./agent.md)。 |
| 5. 组网验收 | 两台设备分处不同物理网络，通过 Tailscale 或 ZeroTier 组网后连接。KoKo 按设计不启用公网中继和外部地址发现。 | 用直连邀请或设备 ID 连接成功，记录组网类型与实测延迟、帧率。设备身份和授权与局域网一致。 | 未开始。组网环境可用直连邀请，前提是控制端能直接访问被控端的 UDP 端口。 |
| 6. 键鼠与人工协作 | 在完成只读验收后，落实 `AgentSafe` 范围、主机侧租约与显式停止，再打开键鼠命令。定义 `koko_request_human` 的本地人工响应流程和结果语义。 | 在授权测试窗口验证输入。撤销租约后，旧租约的请求被主机拒绝。人工请求的允许、拒绝和未响应结果可区分。实时旁观与自动抢占按 Phase 2 验收。 | M2 已实现：`agent_safe` 作用域、被控端控制权仲裁与人工抢占、指示条和停止快捷键、审计日志，以及 `koko_mouse`、`koko_keyboard`、`koko_release_control`。自动化测试与本机端到端已通过，双机实机验收待完成。M3 已实现：`koko_request_human`、被控端求助卡片与附言、本机实体输入检测；检测效果需实机验证。M4 已实现界面树读取与按元素操作，M5 已实现高危操作审批；双机实机验收与资源开销评估待完成。设计见 [Agent 与 MCP 设计规范](./agent.md)。 |

当前迭代优先验收人工远控。按 [macOS 双机测试](./macos.md) 检查窗口拖动、显示器唤醒、设备类型和双向剪贴板，完成后再扩展 Agent 能力。

第一轮交付以工作项 1–3 通过为准。工作项 4 和 5 可分别推进，Phase 1 的完整演示仍以架构文档的验收标准为准。

## 身份与连接验收命令

双机测试可以使用同一个 [macOS 测试包](./macos.md) 在两端选择角色。以下命令供开发时检查 CLI 和协议入口。

在各自的 Mac 上从仓库根目录运行对应命令。身份文件保留在本机的 `~/.koko/`，两端只交换命令输出的公钥标识。

被控端生成或读取主机身份。

```bash
mkdir -p ~/.koko
cargo run --locked -p koko-daemon -- identity ~/.koko/host.key
```

控制端生成或读取自己的身份。

```bash
mkdir -p ~/.koko
cargo run --locked -p koko-agent -- identity ~/.koko/controller.key
```

将 `CONTROLLER_ID` 设为控制端命令输出，将 `HOST_ID` 设为被控端命令输出。把示例地址 `192.168.1.10:4444` 换成被控端用于验收的局域网地址与端口。

被控端启动前台监听。

```bash
cargo run --locked -p koko-daemon -- serve ~/.koko/host.key 192.168.1.10:4444 "$CONTROLLER_ID"
```

控制端验证身份与授权。成功时输出 `Authorized by` 和主机公钥标识。用另一份控制端身份重复相同命令，应以失败退出。

```bash
cargo run --locked -p koko-agent -- check ~/.koko/controller.key "$HOST_ID" 192.168.1.10:4444
```

本地复现可在同一台机器上使用 `127.0.0.1:4444` 和两份独立的身份文件。若 Koko 的本机共享已占用该端口，先关闭共享或换用其他端口。

### 截图命令

`serve` 只处理 `koko/1` 自检，截图需要 `serve-pair`。单独运行时由 daemon 同目录的 `koko-desktop` 截图，Cargo 构建目录里没有这个程序，因此被控端使用已安装应用包内的 daemon。下面使用 `4445` 端口，避免与应用的本机共享冲突。

```bash
/Applications/Koko.app/Contents/Helpers/koko-daemon serve-pair ~/.koko/host.key 0.0.0.0:4445
```

daemon 输出 `Device ID`、`Pair code` 和 `Pair invite`。把 `PAIR_INVITE` 设为输出的直连邀请，同一局域网也可以直接使用设备 ID。控制端配对成功后输出包含 `host_id` 和 `address` 的 JSON。局域网申请需要在 daemon 所在终端输入 `approve <编号>`。

```bash
cargo run --locked -p koko-agent -- pair ~/.koko/controller.key "$PAIR_INVITE"
```

将 `HOST_ADDRESS` 设为配对输出的 `address`，再发送截图命令。成功时输出 `{"status":"frame","png":"<Base64 PNG>"}`。录屏权限的归属见 [macOS 权限设计](./macos-permissions.md#包内权限归属)。

```bash
printf '{"action":"screenshot"}' | cargo run --locked -q -p koko-agent -- remote ~/.koko/controller.key "$HOST_ID" "$HOST_ADDRESS"
```

## 测试与依赖

- Rust 自动化测试使用真实 iroh 连接，覆盖授权名单与错误主机标识、局域网申请需要确认、增强配对遵守拒绝、只把已授权请求转给应用、撤销授权后剪贴板请求失效、持续键鼠连接保持按键顺序，以及局域网优先的子网匹配。Agent 相关测试覆盖首次确认与信任重连、更高作用域需重新确认、被控端缩小作用域、同时只允许一个 Agent、人与 Agent 会话并存、`observer` 只能查看、旧信任文件迁移、设备注册表，以及 MCP 工具列表与未配对设备的报错。M2 增加了控制权仲裁（抢占、停止、继续、空闲过期）、人类控制端占用键鼠连接时 Agent 仍可操作且人的输入立即暂停 Agent、审计日志不含文本、作用域门禁、手势与组合键转换和截图坐标换算的测试。M3 增加了求助的四种结果、等待期间拒绝 Agent 输入、停止即接管，以及真实连接上 Agent 等待被控端回答的测试。M4 增加了界面树文本格式、元素中心点换算，以及 Mac 应用发送的节点 JSON 能被 Rust 解码的测试；界面树遍历在开发机上用 Finder、系统设置和 Chrome 实测。M5 增加了审批状态转换、批准后带标记重发、拒绝不执行、无 Koko.app 时拒绝 Agent 输入的真实连接测试，以及 Swift 回归中的快捷键、关键词、规则文件默认值和终端输入判定。`./scripts/test-macos-desktop.sh` 覆盖权限升级检测与复查、高级设置、更新检查、窗口顶栏拖动与缩放、辅助进程管道和文件访问被拒绝。带图形权限的截图、视频与双机网络行为用实机验收补足。
- 依赖按工作项引入，连接验证 iroh，macOS 屏幕采集使用系统 ScreenCaptureKit，MCP 阶段再引入 rmcp。以选定版本的文档和真实构建确定 API 与最小特性，Cargo 生成锁文件。
- 每项执行[开发规则里的仓库检查](./development.md#验证与复盘)，并记录该项通过的实机场景。对速度或稳定性的改动以测量数据为依据。
