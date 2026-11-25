# Windows 1.1 和 1.2 实机验收

[返回文档目录](./README.md) · [Windows 兼容开发方案](./windows-plan.md)

实机配置待提供。以下命令、观察和验收项均为待验证。开始前确认 Windows 检出的提交已包含 1.1 和 1.2。Windows 端用 x64 实机本地登录，Mac 与 Windows 接入同一个允许设备互访的可信局域网。以下 PowerShell 命令逐块执行，看到预期结果再继续。

## 1. Windows 实机确认系统与显示器

打开普通 PowerShell，运行下面的命令查看系统版本、CPU 架构、显卡与驱动。

```powershell
Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber
Get-CimInstance Win32_Processor | Select-Object Name, Architecture
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion
```

预期处理器的 `Architecture` 为 `9`（x64）。建议使用更新后的 Windows 11，项目最低目标为 Windows 10 21H2（19044）。在「设备管理器 → 显示适配器」确认实际显卡型号与驱动版本。如仅显示 Microsoft 基本显示适配器，从电脑或显卡厂商的支持网站安装适用的正式驱动，并按提示重启。 [处理器架构](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-processor) · [显卡驱动信息](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-videocontroller)

接好两块物理显示器，按 `Win+P` 选择「扩展」。进入「设置 → 系统 → 显示」，点「标识」，把主屏设为 100%，副屏设为 150%，拖动副屏到主屏左侧或上方，点「应用」。记录两块屏幕的分辨率与缩放。预期鼠标可以从设置中相接的边缘跨屏。 [Microsoft 多显示器设置](https://support.microsoft.com/en-us/windows/hardware/display-graphics/how-to-use-multiple-monitors-in-windows)

## 2. Windows 实机安装 Git、MSVC 和 Rust

在普通 PowerShell 先运行 `winget --version`。预期显示版本号。若找不到 `winget`，先从 Microsoft Store 更新或安装「应用安装程序」并重开 PowerShell。 [Microsoft WinGet](https://learn.microsoft.com/en-us/windows/package-manager/winget/)

```powershell
winget install --id Git.Git -e --source winget --accept-package-agreements --accept-source-agreements
```

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget --accept-package-agreements --accept-source-agreements --override '--passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
```

按安装器提示批准 Microsoft 安装程序的管理员权限请求。若系统提示重启，先重启。若 WinGet 显示 Build Tools 已存在，打开「Visual Studio Installer → Build Tools 2022 → 修改」，确认已勾选「使用 C++ 的桌面开发」，包含 MSVC x64/x86 工具和一个 Windows SDK。 [Git for Windows](https://git-scm.com/install/windows) · [Build Tools 包](https://github.com/microsoft/winget-pkgs/tree/master/manifests/m/Microsoft/VisualStudio/2022/BuildTools) · [Microsoft 安装参数](https://learn.microsoft.com/en-us/visualstudio/install/use-command-line-parameters-to-install-visual-studio?view=vs-2022) · [C++ 工作负载](https://learn.microsoft.com/en-us/visualstudio/install/workload-component-id-vs-build-tools?view=vs-2022#desktop-development-with-c)

```powershell
winget install --id Rustlang.Rustup -e --source winget --accept-package-agreements --accept-source-agreements
```

关闭所有 PowerShell 和 Windows Terminal 窗口，再从开始菜单打开一个普通 PowerShell，让新窗口读取更新后的 PATH。确认三个版本命令都能运行，且 `rustc -vV` 包含 `host: x86_64-pc-windows-msvc`。 [Rustup WinGet 包](https://github.com/microsoft/winget-pkgs/tree/master/manifests/r/Rustlang/Rustup) · [Rust 安装与 PATH](https://rust-lang.org/tools/install/) · [MSVC 先决条件](https://rust-lang.github.io/rustup/installation/windows-msvc.html)

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc
git --version
rustc -vV
cargo --version
```

## 3. Windows 实机获取和构建 KoKo

继续在普通 PowerShell。首次克隆或更新已有检出都可逐块执行。克隆时如弹出 GitHub 登录，用有仓库读取权限的账号完成浏览器登录。

```powershell
$kokoSource = Join-Path $env:USERPROFILE 'src'
New-Item -ItemType Directory -Path $kokoSource -Force | Out-Null
$kokoRepo = Join-Path $kokoSource 'koko'
if (-not (Test-Path $kokoRepo)) {
    git clone --branch main https://github.com/Kizai/koko.git $kokoRepo
    if ($LASTEXITCODE -ne 0) { throw 'KoKo clone failed' }
}
Set-Location $kokoRepo
git remote -v
git branch --show-current
```

远端应指向 `Kizai/koko`，当前分支应为 `main`。确认后构建。

```powershell
git pull --ff-only origin main
if ($LASTEXITCODE -ne 0) { throw 'KoKo update failed' }
git log -1 --oneline
cargo build --locked --bins -p koko-daemon -p koko-host-windows
if ($LASTEXITCODE -ne 0) { throw 'KoKo build failed' }
Get-Item .\target\debug\koko-daemon.exe, .\target\debug\koko-host.exe
```

预期 `git log` 显示包含 1.1 和 1.2 的合并提交或其后继提交，Cargo 显示 `Finished`，最后列出两个可执行文件。首次构建会下载依赖。若提示找不到 `link.exe` 或 Windows SDK，先检查 C++ 工作负载，再从开始菜单打开「x64 Native Tools Command Prompt for VS 2022」，输入 `powershell`，复制下列命令重试。 [Microsoft x64 开发者命令提示符](https://learn.microsoft.com/en-us/cpp/build/building-on-the-command-line#developer-command-prompt-shortcuts)

```powershell
Set-Location (Join-Path $env:USERPROFILE 'src\koko')
cargo build --locked --bins -p koko-daemon -p koko-host-windows
```

## 4. Windows 实机配置局域网与防火墙

先在普通 PowerShell 运行 `Get-NetConnectionProfile | Format-Table InterfaceAlias, Name, NetworkCategory`。在「设置 → 网络和 Internet → Wi-Fi 或以太网 → 当前网络」把用于测试的可信网络设为「专用」，再次运行命令应显示 `Private`。

从开始菜单右键「Windows PowerShell → 以管理员身份运行」，复制下列命令。预期两条规则显示 `True` 和 `Private`。完成后关闭管理员窗口，回到普通 PowerShell 启动 KoKo。 [Microsoft 防火墙 PowerShell](https://learn.microsoft.com/en-us/powershell/module/netsecurity/new-netfirewallrule)

```powershell
if (-not (Get-NetFirewallRule -Name 'KoKo-LAN-UDP-27688' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'KoKo-LAN-UDP-27688' -DisplayName 'KoKo LAN UDP 27688' -Direction Inbound -Action Allow -Protocol UDP -LocalPort 27688 -Profile Private -RemoteAddress LocalSubnet | Out-Null
}
if (-not (Get-NetFirewallRule -Name 'KoKo-LAN-mDNS-UDP-5353' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'KoKo-LAN-mDNS-UDP-5353' -DisplayName 'KoKo LAN mDNS UDP 5353' -Direction Inbound -Action Allow -Protocol UDP -LocalPort 5353 -Profile Private -RemoteAddress LocalSubnet | Out-Null
}
Get-NetFirewallRule -Name 'KoKo-LAN-UDP-27688', 'KoKo-LAN-mDNS-UDP-5353' |
    Format-Table DisplayName, Enabled, Profile
```

## 5. Windows 实机启动主机

在本地已解锁的 Windows 桌面，用普通 PowerShell 运行。两个 exe 保持在同一目录，主机会从自身所在目录启动 daemon。首次启动会在当前 Windows 用户的数据目录创建主机身份。持续保留这个 PowerShell 窗口，用它处理配对及停止。

```powershell
Set-Location (Join-Path $env:USERPROFILE 'src\koko')
.\target\debug\koko-host.exe --help
$kokoData = Join-Path $env:USERPROFILE '.koko'
New-Item -ItemType Directory -Force -Path $kokoData | Out-Null
$kokoKeyPath = Join-Path $kokoData 'host.key'
.\target\debug\koko-host.exe $kokoKeyPath '0.0.0.0:27688' 'KoKo Windows'
```

预期先看到三个必填位置参数和可选的 `--no-cross-system-shortcuts`，再看到 `Starting KoKo host` 及 PID、`Listening on` 及端口 27688，以及设备 ID 和临时连接码。记下本轮 host PID 供清理检查。收到配对申请时核对当前申请者，直接输入 Windows 终端给出的 `approve` 或 `decline` 命令和当前编号，按回车。

默认启用跨系统键位映射，Mac ⌘/Control 分别对应 Windows Ctrl/Win，Option 对应 Alt。关闭映射时 Mac ⌘/Control 分别对应 Windows Win/Ctrl。Windows 来源一直按物理键义还原，未指定或未知来源也按物理键义转换。Mac 的 Caps Lock 位置发送 Windows Caps Lock。Fn、音量键、JIS 英数及假名等无对应项的键会跳过。普通按键的字符取决于 Windows 当前键盘布局。

## 6. Mac 控制端验证发现、画面与键鼠

在 Windows 主副屏各开一个容易区分的普通窗口，准备一份无敏感信息的长文档与一个空白的记事本标签页。建议用两张可区分的壁纸。Mac 打开已经安装的 KoKo。如 macOS 弹出本地网络访问提示，允许 KoKo 访问。如果 KoKo 自身报告权限缺失，按应用提示在 macOS「系统设置 → 隐私与安全性」为 KoKo 修改对应权限并重新打开应用。

| 机器 | 操作 | 预期 | 状态 |
| --- | --- | --- | --- |
| Mac 和 Windows | 在 Mac 刷新局域网列表，选中「KoKo Windows」，点「进入桌面」。Windows 先从本地终端拒绝，Mac 再发起一次，Windows 先尝试第一次申请的旧编号，再批准当前编号。 | 拒绝后打不开桌面。旧编号无效。批准当前编号后看到对应 Windows 桌面。 | 待验证 |
| Mac 和 Windows | 关闭远控窗口，从「连接伙伴」用 Windows 当前连接码连接。 | 授权后打开同一台 Windows。 | 待验证 |
| Mac 和 Windows | 让测试窗口遮住 Windows 主屏壁纸。Mac 返回设备列表看卡片封面，再把 Windows 背景切换为纯色并复查。 | 封面显示主屏壁纸，窗口不会出现在封面上。切换纯色后也能生成封面。 | 待验证 |
| Mac 和 Windows | 在 Mac 的远控窗口分别选择两块显示器。Windows 将副屏从主屏左侧调整到上方，Mac 刷新显示器列表并再切换。 | 主屏排首位，两屏都能选中，名称和像素尺寸合理。100% 和 150% 的两屏显示正确，边沿和比例正常。窗口内容变化时，截图轮询随之刷新。 | 待验证 |
| Windows和Mac | **Windows**将koko-host所在终端最小化，关闭其他应用窗口，使桌面露出。**Mac**点击Windows桌面、打开任务栏的「开始」菜单并再次关闭，然后从任务栏打开记事本。 | 桌面、任务栏和应用启动正常响应。该检查记录桌面操作；真正的空前台窗口分支由Sink单元测试模拟。 | 待验证 |
| Windows和Mac | **Mac**保持远控窗口连续截图轮询，切换两块显示器并持续操作约一分钟。**Windows**观察屏幕边缘，记录Windows版本、build、是否出现黄色边框、是常亮还是闪烁，以及切换屏幕后的变化。 | 留下系统版本与实际现象，黄色边框是否出现均如实记录；本项不宣称边框已关闭。 | 待验证 |
| Mac 和 Windows | 用 Mac 在两屏各点四角附近的控件，拖动窗口，双击选中一个单词，打开右键菜单，分别向上、向下及横向滚动。 | 落点跟随对应屏幕，拖动松手后停止，双击和右键有效，滚动方向与预期一致。 | 待验证 |
| Mac 和 Windows | 让副屏成为主屏，再让原主屏恢复主屏身份。 | 刷新后主屏排首，同一设备路径的 ID 保持稳定。每次换主屏后卡片封面使用当前主屏壁纸。 | 待验证 |
| Mac 和 Windows | 在可水平滚动的本地长文档上做轻微和较大的滚动；在支持三击选中一行的应用里连续点三下。若控制端鼠标具备相关按钮，用三键鼠标按滚轮中键，在有本地测试历史的浏览器里按侧键后退、前进。 | 两个轴的滚动方向正确并能响应细小滚动。三击只用实际发出的三组按下、松开。中键和两侧键按应用行为响应。缺少物理按钮时用第 7 节的远控命令补测。 | 待验证 |
| Windows和Mac | **Windows**打开普通记事本，再打开Edge并选中地址栏。**Mac**分别向这两个输入位置通过KoKo文本输入发送`中文😀👍`，逐字核对；在记事本中另测`⌘A`全选与`⌘Z`撤销。地址栏输入不按回车。 | 两处均显示完整中文、😀和👍，没有替换字符、丢字或拆开的代理对；全选与撤销正常。分别记录记事本和Edge结果。 | 待验证 |
| Mac 和 Windows | 在普通记事本中分别按左/右 Shift，再用 Caps Lock 输入一个字母并恢复原状态。 | 左右 Shift 与大小写切换均有效。Caps Lock 发送 Windows Caps Lock，保留 Windows 输入法。 | 待验证 |
| Mac 和 Windows | 在 Windows 记事本上测试连续按键与拖动。在 Mac 按住修饰键或拖动时结束会话，再重连输入一个字母并单击。 | 重连后正常输入与点击，键与鼠标按钮均无按住状态残留。 | 待验证 |
| Windows和Mac | **Windows**用普通权限运行koko-host，另开管理员PowerShell并在本机批准UAC，将该窗口置前。**Mac**点击管理员窗口，再检查远控界面与Windows主机控制台；随后关闭管理员窗口并回到普通记事本重试。管理员窗口的打开命令见第8节。 | 高权限目标的点击被拒绝，Mac显示输入失败提示，Windows控制台说明更高完整性或实际注入失败，协议返回`input_failed`；普通窗口恢复可操作。不能只记录「没有反应」作为通过。 | 待验证 |

Mac远控窗口的`⌘C`和`⌘V`在未启用剪贴板流时会回退到Windows本机复制粘贴；跨机剪贴板同步留到1.4验收。下方CLI还可单独验证Windows本机的Ctrl+C／Ctrl+V及来源键位映射。键位映射关闭后按第 8 节复核。被控端的这两步使用截图轮询和 Windows 本机剪贴板快捷键。实时视频、音频、跨机剪贴板和文件传输留到后续阶段。 [Mac 操作说明](https://github.com/Kizai/koko/blob/main/docs/macos.md)

## 7. Mac 终端补测 PNG、JPEG、光标与 Windows 本机复制粘贴

先关闭 Mac 远控窗口，保持 Windows 主机运行。下面各命令在同一个 Mac「终端」窗口逐块执行。先检查内置 CLI 和 Python 3。在提示处输入 Windows 终端当前显示的 9 位设备 ID，然后切到 Windows 按提示批准来自「KoKo Windows acceptance」的配对。预期 Mac 显示「配对结果已保存」。临时目录中的密钥、配对资料和截图保留在 Mac 本地。

```bash
agent=/Applications/Koko.app/Contents/Helpers/koko-agent
"$agent" --version
python3 --version
testdir=$(mktemp -d)
printf '请输入 Windows 终端显示的 9 位设备 ID，然后回车：'
read -r device
if "$agent" pair "$testdir/controller.key" "$device" --name 'KoKo Windows acceptance' > "$testdir/pair.json"; then
    printf '配对结果已保存。\n'
fi
```

接下来把 Windows 光标停在截图容易辨认的位置。只做画面验证时直接运行，测试键鼠时先将 Windows 的普通记事本空白标签页置于前台，再按 Mac 终端提示输入 `y`。输入测试会在该标签页写入测试文字，并用 Windows 本机剪贴板复制粘贴。脚本以同一个连接逐条发送命令，保留按下与松开之间的状态。脚本只打印尺寸、格式和状态等元数据。双屏时也可选做热插拔。每次重启 Windows 主机或完成一次 `disconnect` 后，先用本节第一段重新配对，再运行 Python 块。

```bash
python3 - "$agent" "$testdir" <<'PY'
import base64
import json
import pathlib
import subprocess
import sys

agent = sys.argv[1]
work = pathlib.Path(sys.argv[2])
pair = json.loads((work / "pair.json").read_text(encoding="utf-8"))
assert pair["status"] == "paired"

with subprocess.Popen(
    [agent, "remote-stream", str(work / "controller.key"), pair["host_id"], pair["address"]],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
    encoding="utf-8",
) as client:
    def prompt(message):
        print(message, end="", flush=True)
        with open("/dev/tty", encoding="utf-8") as terminal:
            return terminal.readline().strip()

    def request(action, **args):
        client.stdin.write(json.dumps({"action": action, **args}) + "\n")
        client.stdin.flush()
        line = client.stdout.readline()
        if not line:
            raise RuntimeError(f"{action}: KoKo CLI closed the connection")
        return json.loads(line)

    def save(filename, reply):
        data = reply.pop("image", None) or reply.pop("png", None)
        fields = ("status", "format", "width", "height", "code")
        summary = {key: reply[key] for key in fields if key in reply}
        print(filename, summary)
        if not data:
            raise RuntimeError(f"{filename}: missing image")
        (work / filename).write_bytes(base64.b64decode(data, validate=True))

    def ok(action, **args):
        reply = request(action, **args)
        if reply["status"] != "ok":
            raise RuntimeError(f"{action}: {reply.get('code', reply['status'])}")

    def pointer(display, *events, source="macos"):
        ok("pointer", display=display, source_os=source, events=events)

    def edge(code, down, flags):
        event = {"kind": "key_down" if down else "key_up", "code": code, "flags": flags}
        if down:
            event["repeat"] = False
        return event

    stream_only = False
    try:
        displays = request("displays")
        assert displays["status"] == "displays", displays.get("code", displays["status"])
        for display in displays["displays"]:
            identifier = display["id"]
            print("Display", display)
            cases = [
                ("png", True, 640, "cursor-on"),
                ("png", False, 640, "cursor-off"),
                ("jpeg", False, 640, "scaled"),
                ("jpeg", False, 4096, "large"),
            ]
            for fmt, cursor, length, label in cases:
                save(
                    f"{identifier}-{label}.{fmt}",
                    request("capture", display=identifier, edge=length, quality=80, format=fmt, cursor=cursor),
                )
        save("primary.png", request("screenshot"))
        save("wallpaper.jpeg", request("wallpaper"))

        if len(displays["displays"]) >= 2:
            secondary = displays["displays"][1]
            choice = prompt(f"可选：测试副屏 {secondary['name']} 的热插拔？输入 y 开始，直接回车跳过：")
            if choice.lower() == "y":
                stale_id = secondary["id"]
                prompt("请先将 Windows 普通记事本置前，再按回车。脚本将在拔屏前按住远控右 Shift：")
                pointer(stale_id, edge(0x3C, True, 2), source="windows")
                prompt("请拔掉指定副屏，等 Windows 显示设置不再列出它，然后回车：")
                capture = dict(display=stale_id, edge=640, quality=80, format="png", cursor=False)
                try:
                    removed = request("displays")
                    print("拔掉副屏后的列表：", removed.get("displays", []))
                    stale = request("capture", **capture)
                    print("旧 ID 截图：", stale["status"], stale.get("code", ""))
                    if (stale["status"], stale.get("code")) != ("error", "display_unavailable"):
                        print("请记录异常：旧副屏 ID 预期返回 display_unavailable。")
                    move = {"kind": "move", "x": 5000, "y": 5000}
                    pointer(stale_id, {"kind": "release"})
                    old_pointer = request("pointer", display=stale_id, events=[move])
                    assert (old_pointer["status"], old_pointer.get("code")) == ("error", "display_unavailable")
                    print("旧 ID 位置请求返回 display_unavailable，纯 release 仍返回 ok。"
                          "请在 Windows 普通记事本本地输入小写字母，确认 Shift 无残留。")
                finally:
                    prompt("请把副屏接回原接口，等 Windows 显示设置重新看到它，再按回车：")
                displays = request("displays")
                assert displays["status"] == "displays", displays.get("code", displays["status"])
                print("重新连接后的列表：", displays["displays"])
                if any(screen["id"] == stale_id for screen in displays["displays"]):
                    save(f"{stale_id}-reconnected.png", request("capture", **capture))
                else:
                    print("请记录原 ID 和新 ID；同一设备路径预期保持原 ID。")

        primary_id = displays["displays"][0]["id"]

        def primary_pointer(*events, source="macos"):
            pointer(primary_id, *events, source=source)

        # Key positions and aggregate flags use koko-input's physical mapping.
        control, command, option, tab, c, v = 0x3B, 0x37, 0x3A, 0x30, 0x08, 0x09
        control_flag, command_flag, option_flag = 1 << 2, 1 << 4, 1 << 3
        test_input = prompt(
            "将 Windows 普通记事本的空白标签页置前。默认映射请输入 y，关闭映射请输入 m，回车跳过："
        ).lower()
        if test_input == "y":

            try:
                ok("text", text="KoKo测试🙂")
                ok("key", key="select_all")
                ok("key", key="copy")
                ok("key", key="right")
                ok("key", key="paste")
                prompt("Windows 预期出现两份 KoKo测试🙂。确认后回车，继续测试逐条发送的键按下与松开。")
                ok("key", key="select_all")
                primary_pointer(edge(command, True, command_flag))
                primary_pointer(edge(c, True, command_flag), edge(c, False, command_flag))
                primary_pointer(edge(command, False, 0))
                ok("key", key="right")
                primary_pointer(edge(command, True, command_flag), edge(v, True, command_flag),
                        edge(v, False, command_flag), edge(command, False, 0))

                prompt("Windows 预期出现四份 KoKo测试🙂。确认后回车，改用 Windows 来源测试 Ctrl+C / Ctrl+V。")
                ok("key", key="select_all")
                primary_pointer(edge(control, True, control_flag), edge(c, True, control_flag),
                        edge(c, False, control_flag), edge(control, False, 0), source="windows")
                ok("key", key="right")
                primary_pointer(edge(control, True, control_flag), edge(v, True, control_flag),
                        edge(v, False, control_flag), edge(control, False, 0), source="windows")

                prompt("Windows 预期出现八份 KoKo测试🙂。确认后回车，测试 Windows 来源的 Win 键。")
                primary_pointer(edge(command, True, command_flag), edge(command, False, 0), source="windows")
                prompt("Windows 预期打开「开始」菜单。确认后回车，发送 Escape 关闭。")
                ok("key", key="escape")
                prompt("确认「开始」菜单已关闭，并保留另一个普通窗口。按回车继续测试 Alt+Tab。")
                primary_pointer(edge(option, True, option_flag), edge(tab, True, option_flag),
                        edge(tab, False, option_flag), edge(option, False, 0))
                print("Windows 预期将另一普通窗口切到前台。")
            finally:
                primary_pointer({"kind": "release"})
        elif test_input == "m":
            try:
                primary_pointer(edge(command, True, command_flag), edge(command, False, 0))
                prompt("关闭映射后，Mac 来源左 Command 预期打开 Windows「开始」，按回车关闭：")
                ok("key", key="escape")
                ok("text", text="KoKo测试🙂")
                ok("key", key="select_all")
                primary_pointer(edge(control, True, control_flag), edge(c, True, control_flag),
                                edge(c, False, control_flag), edge(control, False, 0))
                ok("key", key="right")
                primary_pointer(edge(control, True, control_flag), edge(v, True, control_flag),
                                edge(v, False, control_flag), edge(control, False, 0), source="windows")
                print("预期看到两份 KoKo测试🙂，分别验证 Mac 来源 Ctrl 和 Windows 来源 Ctrl。")
            finally:
                primary_pointer({"kind": "release"})

        if prompt("要补测中键和侧键请输入 b，回车跳过：").lower() == "b":
            def click_button(button):
                position = dict(x=5000, y=5000, button=button)
                primary_pointer(
                    {"kind": "down", "clicks": 1, **position},
                    {"kind": "up", **position},
                )

            prompt("在 Windows 主屏最大化有两个本地测试页历史的浏览器，停在后一页，按回车发送侧键后退：")
            click_button("back")
            prompt("确认浏览器回到前一页，按回车发送侧键前进：")
            click_button("forward")
            prompt("确认返回后一页。把可以滚动的页面移到前台，按回车发送中键，再观察浏览器反应：")
            click_button("middle")
            prompt("末尾检查结束前按回车退出中键自动滚动。")
            ok("key", key="escape")

        if prompt("按第 8 节准备好管理员 PowerShell 后，输入 a 验证拦截，回车跳过：").lower() == "a":
            blocked = request("text", text="k")
            print("管理员窗口回复：", blocked["status"], blocked.get("code", ""))
            assert (blocked["status"], blocked.get("code")) == ("error", "input_failed")
            prompt("确认管理员窗口没有输入，主机显示更高完整性或SendInput失败的明确原因。关闭管理员窗口，"
                   "置前普通记事本，再按回车：")
            ok("text", text="普通窗口恢复")

        close = prompt(
            "普通记事本置前，输入 d 测试关闭指针连接，输入 s 测试结束会话，回车跳过："
        ).lower()
        if close in ("d", "s"):
            primary_pointer(edge(0x3C, True, 2), source="windows")
            stream_only = close == "d"
            print("断开后请在 Windows 本地输入小写字母，确认 Shift 无残留。")
    finally:
        try:
            if not stream_only:
                print("Disconnect:", request("disconnect")["status"])
        except (OSError, RuntimeError, ValueError) as error:
            print("Disconnect:", error, file=sys.stderr)
        client.stdin.close()

print("Local screenshot directory:", work)
PY
```

用 Finder 打开本次结果，只打开测试图片。

```bash
open "$testdir"
```

| 机器 | 操作与预期 | 状态 |
| --- | --- | --- |
| Mac | 用「预览」打开两种格式。每块屏幕的 640 长边样本按比例缩小，4096 长边样本在原图不足 4096 时保持原尺寸。 | 待验证 |
| Mac | 对比相同屏幕的 `cursor-on`、`cursor-off`，前者显示 Windows 光标，后者隐藏。 `primary.png` 是带光标的主屏，`wallpaper.jpeg` 是主屏封面。 | 待验证 |
| Mac 和 Windows | 在脚本的热插拔提示中选择测试，拔掉指定副屏。用其旧 ID 截图和移动应返回 `display_unavailable`，纯 `release` 成功，普通记事本确认远控右 Shift 已释放。接回原接口后检查刷新列表，同一设备路径的 ID 应保持稳定。可调整分辨率并核对刷新后的尺寸与重连截图。 | 待验证 |
| Mac 和 Windows | 在记事本核对两份、四份、八份 `KoKo测试🙂`，分别对应语义指令、Mac 来源 Cmd 映射为 Ctrl、Windows 来源 Ctrl 原样发送。Windows 来源的 Win 键打开「开始」菜单，Escape 关闭。保留第二个普通窗口，确认 Alt+Tab 将其切到前台。 | 待验证 |

用脚本的 `d` 关闭已按住右 Shift 的指针连接，预期本机输入无修饰键残留。再运行一次，选 `s` 发送 `disconnect` 结束会话，预期同样释放，重连时需要重新配对。中键和两个侧键可用 `b` 补测。状态均为待验证。

## 8. Windows 实机复核键位开关和高权限窗口

### 关闭键位映射后重新连接

在原 Windows 主机窗口按 Ctrl+C，再在同一个普通 PowerShell 运行。

```powershell
Set-Location (Join-Path $env:USERPROFILE 'src\koko')
$kokoKeyPath = Join-Path $env:USERPROFILE '.koko\host.key'
.\target\debug\koko-host.exe $kokoKeyPath '0.0.0.0:27688' 'KoKo Windows' --no-cross-system-shortcuts
```

在 Mac 重新连接，单按 ⌘ 后释放。预期出现 Windows「开始」菜单，用 Escape 关闭。关闭 Mac 远控窗口，按第 7 节为 Mac 终端重新配对并运行 Python 块，键鼠提示输入 `m`。预期 Mac 来源左 Command 打开「开始」，Mac 来源 Control 可复制，Windows 来源 Ctrl 可粘贴。完成后按 Ctrl+C 停止 Windows 主机，按第 5 节重启以恢复默认映射。状态待验证。

### 高完整性进程的输入拦截

从另一个普通 Windows PowerShell 运行下方命令。只有测试目标窗口以管理员权限启动。按 Windows 本地 UAC 提示批准后置前新窗口。操作时保持 KoKo 主机原有权限。

```powershell
Start-Process powershell.exe -Verb RunAs
```

在 Mac 远控窗口向空白管理员 PowerShell 单击并输入一个字母，避免回车。管理员窗口预期没有收到输入，普通的 KoKo 主机终端预期出现 `Windows input blocked` 并说明完整性更高或无法查询。需要记录明确的协议回复时，关闭 Mac 远控窗口，按第 7 节为 Mac 终端重新配对并运行脚本，在管理员测试提示处输入 `a`，预期看到 `error input_failed`。最后在 Windows 本地关闭管理员窗口，将普通记事本置前，确认 Mac 可继续输入。状态待验证。 [Microsoft SendInput 与 UIPI](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)

## 9. Windows 检查 Ctrl+C、EOF 和指定 PID 的强制退出

再开一个普通 PowerShell，用于查询本轮 host 与 daemon，下面的命令只接受由 host 控制台给出的 PID。请在本轮 host 仍在运行时执行。

```powershell
$kokoBin = Join-Path $env:USERPROFILE 'src\koko\target\debug'
$kokoHostExe = Join-Path $kokoBin 'koko-host.exe'
$kokoDaemonExe = Join-Path $kokoBin 'koko-daemon.exe'
$kokoHostPid = [int](Read-Host '请输入本轮 Windows host 控制台显示的 PID')
$kokoHostInfo = Get-CimInstance Win32_Process -Filter "ProcessId = $kokoHostPid"
if ($kokoHostInfo.ExecutablePath -ne $kokoHostExe) {
    throw 'PID 与本次构建的 koko-host.exe 不匹配。请重新核对。'
}
$kokoDaemon = Get-CimInstance Win32_Process -Filter "ParentProcessId = $kokoHostPid AND Name = 'koko-daemon.exe'" |
    Where-Object ExecutablePath -eq $kokoDaemonExe
if (-not $kokoDaemon) { throw '未找到本轮 host 的同目录 daemon' }
$kokoHostInfo, $kokoDaemon | Format-Table Name, ProcessId, ParentProcessId
```

回到原 host 窗口按 `Ctrl+C`，预期 Mac 连接结束，原 PowerShell 恢复提示符。再回查询窗口执行下列命令，预期没有输出。可用 Mac 先按住一个修饰键或正在拖动，再通过 Windows 本地按 Ctrl+C 停止主机，随后核对普通记事本输入与鼠标拖动。

```powershell
Get-CimInstance Win32_Process -Filter "ProcessId = $kokoHostPid OR ProcessId = $($kokoDaemon.ProcessId)" |
    Format-Table Name, ProcessId, ParentProcessId
```

在普通 PowerShell 用空行管道启动一次，输入流随即结束。预期进程退出并恢复提示符。命令输出会再次显示新的 host PID，检查它及本次 daemon 已结束。

```powershell
Set-Location (Join-Path $env:USERPROFILE 'src\koko')
$kokoKeyPath = Join-Path $env:USERPROFILE '.koko\host.key'
'' | .\target\debug\koko-host.exe $kokoKeyPath '0.0.0.0:27688' 'KoKo Windows'
Get-NetUDPEndpoint -LocalPort 27688 -ErrorAction SilentlyContinue
```

重新按第 5 节启动一次，确认 Mac 可以重新连接。仅用这次专门测试的实例做强制退出。查询窗口重跑本节的 PID 查询块，输入新 PID，再执行下面这段。它再次核对路径与启动时间，预期只终止选中的 host，该实例的 daemon 随后退出。最后再次启动，检查端口和 Mac 连接可用。

```powershell
$kokoCurrent = Get-CimInstance Win32_Process -Filter "ProcessId = $kokoHostPid"
if ($kokoCurrent.ExecutablePath -ne $kokoHostExe -or $kokoCurrent.CreationDate -ne $kokoHostInfo.CreationDate) {
    throw '当前 PID 已发生变化，请重新查询本轮进程。'
}
Stop-Process -Id $kokoHostPid -Force
Start-Sleep -Seconds 3
Get-CimInstance Win32_Process -Filter "ProcessId = $kokoHostPid OR ProcessId = $($kokoDaemon.ProcessId)" |
    Format-Table Name, ProcessId, ParentProcessId
```

## 10. 记录实机结果

| 记录项 | 本次实测 |
| --- | --- |
| Windows 版本、Build、x64 CPU、GPU 和驱动 | 待提供 |
| Mac 型号、macOS 版本、KoKo 版本及构建提交 | 待提供 |
| Windows KoKo 提交、网络与防火墙配置文件 | 待提供 |
| 两屏型号、分辨率、100% / 150% 缩放、排列和主屏标记 | 待提供 |
| 发现、拒绝与批准、封面、双屏、热插拔和截图 | 待验证 |
| 键鼠、中文与 emoji、Windows 本机复制粘贴、完整性拦截与按键释放 | 待验证 |
| Caps Lock 与左右修饰键、默认和关闭映射、中键和侧键、滚动方向 | 待验证 |
| 旧 ID 键鼠请求与释放、Pointer closed 与会话断开、Ctrl+C 释放 | 待验证 |
| Ctrl+C、EOF、指定 host 强退后 daemon 清理和重启 | 待验证 |
| 挂起 daemon 后的宽限期、启动期间强制退出、已有 Job 和含空格的路径 | 待验证，按 [1.1 验收说明](./windows-1.1-acceptance.md) 分别记录 |

测试结束在 Windows 主机窗口按 Ctrl+C。Mac 的本地结果目录可以通过 Finder 查看或移到废纸篓。保留 `host.key` 供下次保持同一设备身份。共享实测结果时，提交版本、操作、预期、实际与第一条错误即可，截图请先遮住个人内容、连接码或邀请。密钥与临时配对资料留在本机。
