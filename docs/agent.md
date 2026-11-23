# Agent 与 MCP 设计规范

[返回文档目录](./README.md)

本文定义 KoKo 的核心能力：AI Agent 通过 MCP 识别并操作已授权的远程 Mac，被控端的人可以随时看到、停止和接管。M1 只读 MCP 已实现，M2 起仍是设计稿，各里程碑的状态见[里程碑与验收](#里程碑与验收)。现有协议见[架构设计的当前实现](./architecture.md#当前实现macos-内测版)，交付顺序见 [Phase 1 实现计划](./implementation.md#实现顺序与验收)的工作项 4 和 6。

## 目标与边界

| 目标 | 说明 |
| --- | --- |
| 识别 | Agent 能获得带坐标信息的截图，后续能读取精简的界面树，准确定位要操作的元素。 |
| 操作 | Agent 能点击、拖拽、滚动、输入文本和组合键，坐标与显示器缩放无关。 |
| 区分 Agent 与人 | 被控端依据密码学身份区分 Agent 和人类控制端，按身份限定能力。 |
| 人保留控制权 | 被控端显示 Agent 正在操作，人可以一键停止。人的操作会立即抢占 Agent，Agent 收到明确的状态，不会盲目重试。 |
| 易于接入 | 在 Claude Desktop 等 MCP 客户端里加一条配置即可使用。 |

本阶段不做：Windows、Linux 和移动端被控端，公网中继，Agent 使用 Shell 或文件传输，多个 Agent 同时控制一台主机。平台扩展在这些核心能力完成后再规划。

## 现状与差距

| 需要 | 现状（0.1.20） | 差距 |
| --- | --- | --- |
| 区分 Agent 与人 | `Koko.app` 和 CLI 共用 `~/.koko/controller.key`，被控端只认一个公钥 | 被控端无法分辨 Agent |
| 按能力授权 | 授权后的会话可以查看、键鼠和使用剪贴板 | 没有作用域 |
| 人看着 Agent 操作 | 被控端同时只授权一个控制端 | 人和 Agent 不能同时连接 |
| Agent 重连已配对设备 | 控制端不保存配对结果，CLI 只打印 `host_id` 和 `address` | 需要设备注册表 |
| 稳定连接 | `koko-agent remote` 每条命令新建一个进程和连接 | MCP Server 需要长连接 |
| 截图坐标信息 | `screenshot` 只截主屏，长边 1600 像素的 PNG，不返回尺寸 | 需要显示器、尺寸和坐标映射 |
| 组合键 | `key` 只支持 13 个固定按键；`pointer` 可以发送任意 Mac 虚拟键码 | 需要键名规范 |
| 人工接管 | 无 | 需要控制权租约与抢占 |

## 方案选择

| 方案 | 做法 | 收益与代价 | 适用条件 |
| --- | --- | --- | --- |
| **A. 控制端独立 MCP Server** | `koko-agent mcp` 通过 stdio 提供 MCP，使用独立的 Agent 身份直连被控端。作用域、租约和抢占都在被控端 daemon 判定。 | 被控端能可靠识别 Agent，不依赖控制端打开 `Koko.app`，复用现有 Rust 传输与真实连接测试。需要把 daemon 的单会话改成会话表，允许人和 Agent 同时连接。 | 默认方案 |
| B. 借用控制端 `Koko.app` 的会话 | MCP Server 嵌入控制端应用，Agent 借用人的远控会话。 | 人天然在旁观，改动最少。但被控端看到的仍是人的公钥，无法在被控端识别或限制 Agent，违背授权在执行边界判定的规则。控制端应用还必须常驻。 | 只需临时演示 |
| C. 被控端本机 MCP Server | Agent 与被控 Mac 在同一台机器上，直接调用本机采集与输入。 | 没有网络延迟。但无法远程使用，与 KoKo 的 P2P 定位无关，还要另建一套权限模型。 | 只做本机自动化 |

**采用 A。** 最强的反例是用户想在自己的远控窗口里看着 Agent 操作。A 通过被控端同时允许一个人类会话和一个 Agent 会话来满足这一点，代价是 daemon 的授权从单个公钥改为会话表。出现以下证据时，需要重新评估：主流 MCP 客户端无法稳定运行 stdio 子进程，这时改用本机 Streamable HTTP；主要用户需求变成本机自动化，这时先做 C。

## 身份与识别

- **独立密钥**。Agent 默认使用 `~/.koko/agent.key`。`koko-agent mcp --identity <文件> --name <名称>` 可为不同 MCP 客户端指定不同身份。
- **识别依据**。被控端只依据 QUIC 握手确认的公钥识别 Agent。Agent 自报的名称和类型只用于显示。
- **配对声明**。`koko/pair/1` 流中，旧客户端仍发送 10 字节连接码或 `request`。新客户端发送不超过 1 KiB 的 JSON，首字节为 `{`，与旧格式不冲突。旧格式一律按人类控制端处理。

  ```json
  {"v": 2, "kind": "agent", "name": "Claude Desktop", "scope": "agent_safe", "code": "ABCDEFGHJK"}
  ```

  `code` 可省略，省略时按局域网申请处理。daemon 目前只读取 10 字节，需要放宽读取上限。
- **首次确认，之后信任重连**。Agent 第一次配对时，即使提供了有效连接码，也必须由被控端确认。连接码只用来找到并绑定主机，不能跳过确认。确认框显示类型（人 / Agent）、名称、公钥指纹和申请的作用域，被控端可以降低作用域，例如只授予 `observer`。对 Agent 的确认默认勾选「信任」。信任后，同一 Agent 公钥在本机共享开启时直接重连，沿用已授予的作用域，不再询问。只有以下情况需要再次确认：申请比已授予更高的作用域，被控端清除了信任，或 Agent 换了密钥。
- **信任记录**。升级为 `~/.koko/trusted-controllers.json`，每条记录包含公钥、类型、作用域、名称和批准时间。读取到旧的每行一个公钥的文件时，按人类控制端、完整权限迁移。
- **被控端输出**。待确认的申请输出为 `Pair request: <编号> <公钥短标识> {"kind","name","scope"}`，Agent 会话开始和结束分别输出 `Agent session: <公钥短标识> {"name","scope"}` 与 `Agent session closed`。应用用 `approve <编号> observer` 这类命令缩小作用域，用 `end-agent` 断开 Agent。

## 授权作用域

| 命令 | `observer` | `agent_safe` | `human` |
| --- | --- | --- | --- |
| `hello`、`displays`、`screenshot`、`capture`、`video`、`disconnect` | ✓ | ✓ | ✓ |
| `ui_tree`（Phase 3） | ✓ | ✓ | ✓ |
| `pointer`、`click`、`key`、`text` | — | 持有控制权租约时 | ✓ |
| `acquire_control`、`release_control`、`request_human` | — | ✓ | — |
| `clipboard` | — | — | ✓ |

`human` 对应当前的人类远控会话。`agent_full` 留给后续的 Shell 与文件能力，本阶段不实现。所有作用域检查都在 daemon 执行，未知类型或作用域一律拒绝。

## 会话与控制权

### 会话表

daemon 把当前的单个已授权公钥改为会话表：同一时间最多一个 `human` 会话和一个 Agent 会话。已有 Agent 会话时，其他 Agent 的配对或连接返回 `agent_busy`。多 Agent 并行等单 Agent 的流程稳定后再评估。每个会话分别撤销。停止共享或清除信任会撤销全部会话。

被控端无人值守、也没有人类控制端在线时，同样允许已授权的 Agent 操作。指示条、全局快捷键和审计日志照常生效，人回来后可以随时停止。

### 控制权状态机

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> AgentControl: Agent 首次输入或 acquire_control
    AgentControl --> Idle: release_control、断开或 60 秒无操作
    AgentControl --> Paused: 人的输入、被控端点「停止」
    AgentControl --> AwaitingHuman: request_human
    AwaitingHuman --> AgentControl: 人点「完成」
    AwaitingHuman --> Paused: 人选择接管或拒绝
    AwaitingHuman --> AgentControl: 超时
    Paused --> Idle: 人点「让 Agent 继续」
```

- **Idle**：没有 Agent 持有控制权。Agent 的首次输入自动申请租约。最近 3 秒内有人类输入时拒绝申请，返回 `preempted_by_human`。
- **AgentControl**：Agent 持有 `lease_id`。被控端显示指示条「Agent「名称」正在操作 · 停止」。指示条不进入 Agent 的截图，避免遮挡和误点；人类控制端的视频照常显示它。
- **Paused**：租约已吊销。daemon 丢弃该 Agent 尚未执行的输入，之后的输入请求都返回 `preempted_by_human`。只有人能让流程回到 Idle，Agent 不能自行恢复。
- **AwaitingHuman**：`request_human` 请求挂起，直到人作出回应或超时，期间 Agent 的输入被拒绝。超时后结果为 `timeout`，控制权交还 Agent。被控端的人和人类控制端都可以回应。首个版本先在被控端 `Koko.app` 弹出求助卡片，显示 Agent 名称和原因，提供「完成」「接管」「拒绝」和附言；人类控制端的回应入口在后续版本加入。

### 人工接管的触发来源

| 来源 | 首个版本 | 说明 |
| --- | --- | --- |
| 人类控制端的键鼠或剪贴板请求 | 实现 | daemon 直接可知。 |
| 被控端指示条的「停止」和全局快捷键 | 实现 | 快捷键用 Carbon `RegisterEventHotKey`，不需要额外权限。 |
| 被控 Mac 本机的实体键鼠 | 待验证 | 方案 a：KoKo 投递事件改用私有事件源，轮询 `CGEventSourceSecondsSinceLastEventType(.hidSystemState, …)`，需实测能否排除 KoKo 自己投递的事件。方案 b：用只监听的事件 tap，按 `eventSourceUserData` 标记过滤 KoKo 的事件，可能需要「输入监控」权限。验证前不承诺这一能力。 |

### 审计

被控端把 Agent 的每次请求追加到 `~/.koko/audit.log`（JSON Lines），记录时间、公钥指纹、类型、名称、命令类型、显示器和结果码。不记录截图和文本内容，文本只记长度。日志超过 10 MiB 时轮转。

## 识别：屏幕与界面

### 截图

- 扩展 `capture` 命令：增加可选字段 `format`（`jpeg` 或 `png`）和 `cursor`。`image` 回复增加 `width`、`height` 和 `format`。旧版被控端会忽略新字段，返回的 JPEG 不带尺寸，`koko-agent` 解码图像获得尺寸。
- MCP 默认长边 1280 像素，兼顾识别精度与 token 开销。上限沿用协议的 4096，建议不超过客户端模型的图像上限。
- 截图结果附带全部显示器的 ID、名称、像素尺寸和是否为主屏，Agent 据此选择显示器。

### 坐标映射

Agent 使用它看到的截图里的像素坐标。`koko-agent` 为每张截图分配 `screenshot_id`，记录设备、显示器和图像尺寸，并按以下方式换算为协议坐标：

```text
协议坐标 = min(9999, floor(像素坐标 × 10000 ÷ 图像边长))
```

动作未指定 `screenshot_id` 时，使用该显示器最近一张截图。如果显示器列表或分辨率在截图之后发生了变化，返回 `stale_screenshot`，要求 Agent 重新截图。

### 界面树（Phase 3）

- 被控端用 `AXUIElement` 读取前台应用的界面元素，使用已授予的辅助功能权限。
- 每个节点包含角色、标题或标签、值、是否可用、是否聚焦、归一化到显示器的 0–9999 边框，以及 `element_id`。`element_id` 在下一次读取界面树之前有效。
- 只保留可见且可交互或含文字的节点，默认最多 500 个节点。
- 密码框（`AXSecureTextField`）不返回值。
- 动作可以用 `element_id` 代替坐标。具体是点击元素中心还是执行 `AXPress`，在 Phase 3 验证后决定。

### 动作后截图

`koko_mouse` 和 `koko_keyboard` 支持 `screenshot_after`。执行动作后等待画面稳定（300–1000 毫秒）再返回截图，省去一次往返。

## 远程操作

### 鼠标

所有鼠标动作都通过现有的 `pointer` 命令发送，不使用只支持主屏的 `click`。

| `action` | 转换为 |
| --- | --- |
| `move` | `move` |
| `click`、`right_click`、`middle_click` | 对应按键的 `down`（`clicks: 1`）和 `up` |
| `double_click` | 两组 `down`/`up`，第二组 `clicks: 2` |
| `drag` | 在起点 `down`，插值 `move` 到终点，再 `up` |
| `scroll` | `scroll`，`dx`、`dy` 以像素计 |

### 键盘

- **文本**：`text` 原样输入到当前聚焦的元素，不受键盘布局影响，单次最多 2048 字节。
- **按键**：`keys` 是字符串或字符串数组，例如 `"cmd+shift+t"`、`["cmd+a", "delete"]`，按顺序执行。
  - 修饰键：`cmd`、`ctrl`、`opt`（别名 `alt`）、`shift`、`fn`。
  - 按键名：`a`–`z`、`0`–`9`、`enter`、`escape`、`tab`、`space`、`delete`（退格）、`forward_delete`、`up`、`down`、`left`、`right`、`home`、`end`、`page_up`、`page_down`、`f1`–`f12`，以及 ANSI 键盘上的标点字符。
  - `koko-agent` 将键名换算成 Mac 虚拟键码，作为 `key_down`/`key_up` 发送。虚拟键码对应物理键位，非 ANSI 布局下字母键可能输出不同字符，要输入字面文本时使用 `text`。
  - 每个组合键执行完都释放全部按键。出错时发送 `release`。

剪贴板、Shell 和文件传输不在本阶段的 Agent 能力范围内。

## MCP Server

### 启动与配置

`koko-agent mcp` 使用 rmcp 的 stdio 传输，诊断日志写到 stderr。Claude Desktop 的配置文件位于 `~/Library/Application Support/Claude/claude_desktop_config.json`：

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

MCP Server 为每台设备保持一条长连接，断开后按设备注册表的地址重连，地址失效时用 mDNS 按主机公钥重新查找。

### 设备注册表

注册表与 Agent 身份文件同目录，文件名为身份文件名加 `-hosts.json`，默认是 `~/.koko/agent-hosts.json`，这样每个身份各有一份列表。它记录主机公钥、名称、系统类型、最近可用的地址、被授予的作用域和配对时间，由 `koko_pair` 写入。连接失败时，MCP Server 先重连一次，再用 mDNS 按主机公钥刷新地址，最后以信任重新配对；已信任的 Agent 不会触发确认。

### 工具

| 工具 | 参数 | 返回 | 被控端要求 |
| --- | --- | --- | --- |
| `koko_list_devices` | 无 | 设备 ID、名称、系统类型、是否在线、作用域、控制权状态 | 无。在线状态取自已有连接或 2 秒内的 mDNS 发现结果 |
| `koko_pair` | `code_or_invite`，M2 起增加可选的 `scope` | 设备信息和被授予的作用域。首次配对等待被控端确认，最长 90 秒；已信任时立即返回 | 首次配对需被控端确认 |
| `koko_screenshot` | `device_id`，可选 `display`、`max_edge`（默认 1280）、`format`、`cursor` | 图像，以及 `screenshot_id`、显示器、尺寸和显示器列表 | `observer` |
| `koko_mouse` | `device_id`、`action`、`x`、`y`，可选 `to_x`、`to_y`、`dx`、`dy`、`screenshot_id`、`screenshot_after` | 状态，可附截图 | `agent_safe` 和控制权 |
| `koko_keyboard` | `device_id`，`text` 或 `keys`，可选 `screenshot_after` | 状态，可附截图 | `agent_safe` 和控制权 |
| `koko_request_human` | `device_id`、`reason`，可选 `timeout_s`（默认 300，最大 600） | `resolved`、`declined`、`taken_over` 或 `timeout`，以及人的附言 | `agent_safe` |
| `koko_release_control` | `device_id` | 状态 | `agent_safe` |
| `koko_get_ui_tree`（Phase 3） | `device_id`，可选 `display`、`max_nodes` | 精简界面树 | `observer` |

工具描述和返回给模型的消息使用英文，以兼容更多模型。用户界面仍使用中文。工具结果以 JSON 文本返回，客户端支持时同时提供 `structuredContent`。需要 Agent 停下的结果设置 `isError: true`。

### 结果与 Agent 应对

| 结果码 | Agent 应该怎么做 | 可重试 |
| --- | --- | --- |
| `preempted_by_human` | 停止操作，向用户说明，或调用 `koko_request_human` | 否 |
| `declined` | 放弃当前子任务 | 否 |
| `scope_denied` | 请用户在被控端授予 `agent_safe` | 否 |
| `screen_permission_required`、`input_permission_required` | 请用户为被控端开启对应 macOS 权限 | 否 |
| `stale_screenshot`、`display_unavailable` | 重新截图，必要时换显示器 | 是 |
| `capture_busy`、`pointer_busy`、`video_busy` | 稍后重试 | 是 |
| `agent_busy` | 告诉用户被控端已有另一个 Agent 在使用 | 否 |
| `not_paired`、`peer not authorized` | 请用户重新配对 | 否 |
| `invalid_command` | 提示两端版本不一致 | 否 |

## 协议变更

所有变更都在 `koko/remote/1` 上做向后兼容的扩展。旧版被控端对新命令回复 `invalid_command`，客户端据此判断对方只支持人类会话。

| 新增 | 说明 |
| --- | --- |
| `hello`（无参数） | 回复 `hello` `{kind, scope}`。M2 增加 `control` |
| `acquire_control`、`release_control` | 回复 `control` `{lease_id, state}` |
| `request_human` `{reason, timeout_s}` | 长请求，回复 `human` `{result, note}`。daemon 的桌面等待时间按 `timeout_s` 放宽，参照剪贴板请求的特例 |
| `ui_tree`（Phase 3） | 回复精简界面树 |
| 输入命令的可选字段 `lease_id` | Agent 会话必须携带 |
| `capture` 的 `format`、`cursor`；`image` 回复的 `width`、`height`、`format` | 用于截图坐标映射 |
| `displays` 回复的 `control` | 人类控制端据此显示 Agent 的状态 |
| 错误码 `scope_denied`、`lease_required`、`stale_lease`、`preempted_by_human`、`agent_busy` | 见上文 |

配对格式的变更见[身份与识别](#身份与识别)。

## 安全要求

- 作用域、租约和抢占只在被控端 daemon 判定。控制端和 MCP Server 的检查只为提前给出友好的错误。
- Agent 不能批准配对、扩大作用域或解除 Paused 状态。
- 截图和输入的文本不写入日志。
- 默认拒绝：未知类型、未知作用域，以及未带有效租约的 Agent 输入。
- 停止共享、清除信任或点「停止」立即生效，进行中的请求返回错误。
- 屏幕内容可能包含针对 Agent 的提示注入。KoKo 无法判断屏幕内容是否可信，依靠作用域、人工接管，以及 Phase 3 的高危操作审批来降低风险。

## 影响范围

| 模块 | 改动 |
| --- | --- |
| `koko-proto` | 新命令、新字段、新错误码和配对 v2 格式 |
| `koko-daemon` | 会话表取代单个授权公钥，配对 v2，信任文件迁移，控制权状态机，`request_human` 长请求，审计日志 |
| 被控端 `Koko.app` | 确认框显示类型与作用域，指示条与停止，全局快捷键，响应 `request_human`，截图时排除指示条。daemon 的输出需新增控制权和人工请求相关的行 |
| 控制端 `Koko.app` | 显示 Agent 的控制权状态；人的输入会抢占 Agent |
| `koko-agent` | `mcp` 子命令、设备注册表、长连接、键名换算 |
| 文档 | 更新[架构设计](./architecture.md)的当前实现，在 [macOS 双机测试](./macos.md)中补充 MCP 配置与验收 |

## 里程碑与验收

| 里程碑 | 内容 | 通过条件 |
| --- | --- | --- |
| M1 只读 MCP（工作项 4，已实现，待实机验收） | Agent 身份、配对 v2、`observer`、设备注册表、`koko_list_devices`、`koko_pair`、`koko_screenshot` | Claude Desktop 配置后能列出设备并截取任意显示器。未授权或作用域不足的 Agent 被拒绝。Agent 首次配对需要被控端确认，信任后重连不再询问，申请更高作用域时重新确认。Agent 与人类控制端同时连接时互不影响，第二个 Agent 收到 `agent_busy`。集成测试用真实 iroh 连接覆盖作用域拒绝、信任重连和 `agent_busy`。 |
| M2 Agent 操作（工作项 6） | `agent_safe`、控制权租约、`koko_mouse`、`koko_keyboard`、指示条与停止、审计日志 | 在测试窗口中点击、拖拽、组合键和文本都正确。点「停止」后，旧租约的请求被拒绝并返回 `preempted_by_human`。人类控制端的输入会立即抢占。 |
| M3 人工协作（工作项 6） | `koko_request_human`、Paused 恢复，按验证结果加入本机实体输入检测 | 被控端的求助卡片能给出完成、拒绝、接管三种结果，超时结果单独可辨。无人值守时 Agent 照常运行，指示条和审计日志生效。 |
| M4 界面树 | `koko_get_ui_tree`，按 `element_id` 操作 | 常见应用能按元素操作，密码框的值不会外泄。 |
| M5 策略与审批 | 高危操作规则与单次审批 | 命中规则的请求挂起，直到被控端允许或拒绝。 |

## 已确认的决定

| 问题 | 决定 |
| --- | --- |
| Agent 用连接码配对时是否需要被控端确认 | 只在首次配对时确认，默认信任该 Agent 公钥，之后重连不再询问。连接码不能跳过首次确认。申请更高作用域、清除信任或更换密钥时重新确认。 |
| `koko_request_human` 由谁响应 | 被控端的人和人类控制端都可以。首个版本先实现被控端。 |
| 无人值守时是否允许 Agent 操作 | 允许。指示条、全局快捷键和审计日志照常生效。 |
| 是否允许多个 Agent 同时连接同一台被控端 | 先只允许一个，其他 Agent 收到 `agent_busy`。单 Agent 流程稳定后再评估。 |
