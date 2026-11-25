# Windows 1.1 显示器、截图与生命周期实机验收

[返回文档目录](./README.md) · [Windows 兼容开发方案](./windows-plan.md) · [1.1 和 1.2 从安装到实机验收](./windows-1.2-acceptance.md)

**状态：待验证。** 本页由实机操作者填写。CI 的 WIC 合成像素测试、进程 Job 测试和行协议测试分别检查编码、子进程和协议，实机结果见下表。

| 记录项 | 实测配置 |
| --- | --- |
| Windows 版本、Build、架构、CPU | 待提供 |
| GPU 型号及驱动版本、本地或远程桌面会话、普通或管理员账户 | 待提供 |
| 每块屏的型号、像素分辨率、物理像素矩形、排列、缩放、主屏标记 | 待提供 |
| Mac 型号、macOS 版本、KoKo 版本及构建提交 | 待提供 |
| Windows 构建提交、网络连接方式、Windows 防火墙网络配置文件 | 待提供 |

## 启动被控端

在已解锁的 Windows 10 21H2（Build 19044）或更新版本上运行。主屏设为 100%，副屏设为 150%，先将副屏放在主屏左边。建议打开无隐私信息的测试图案，方便区分两块屏及判断裁剪位置。使用两个分别可辨认的壁纸检查设备封面。

在装好 MSVC C++ 构建工具、Windows SDK 和 Rust stable 的 Windows PowerShell 中，从仓库根目录构建并启动。目录名可含空格，两个 exe 需放在同一目录。

```powershell
cargo build --locked --release --bins -p koko-host-windows -p koko-daemon
if ($LASTEXITCODE -ne 0) { throw 'KoKo 构建失败' }
$kokoData = Join-Path $env:USERPROFILE '.koko'
New-Item -ItemType Directory -Force -Path $kokoData | Out-Null
$kokoKeyPath = Join-Path $kokoData 'host.key'
.\target\release\koko-host.exe $kokoKeyPath '0.0.0.0:27688' 'KoKo Windows'
```

记录 host 启动时显示的 PID。确认控制台出现监听地址、设备 ID 和临时验证码。如果系统显示防火墙提示，只在用于验收的可信网络上授权。每次出现配对请求时，核对对方名称、设备、类型与权限范围，再输入该行给出的 `approve <编号>` 或 `decline <编号>`。临时码和直连邀请由 daemon 按现有验证码流程授权。反馈材料须涂去连接码、邀请、私钥和屏幕上的个人数据。

## 控制端与画面

| 步骤 | 操作与预期 | 实测记录 |
| --- | --- | --- |
| 1. 启动与发现 | **Windows** 记录监听地址和 host/daemon PID，确认只有一个 host 控制台。**Mac** 打开 KoKo 并刷新局域网列表，找到正确的 Windows 设备卡片。 | 待验证 |
| 2. 本地确认 | **Mac** 选中 Windows 卡片并点「进入桌面」。**Windows** 核对请求后先输入 `decline <编号>`，**Mac** 应无法打开画面。再次从 Mac 发起，Windows 尝试旧编号，旧编号应无效。再用当前编号 `approve <编号>`，Mac 应看到该设备画面。 | 待验证 |
| 3. 临时授权 | **Mac** 结束会话，从「连接伙伴」使用当前短码或直连邀请连接。确认显示同一 Windows 设备。 | 待验证 |
| 4. 封面 | **Windows** 为主屏与副屏设置明显不同的壁纸并展开一个测试窗口。**Mac** 确认卡片封面对应主屏壁纸，测试窗口不会出现在封面上。再用纯色背景复核。 | 待验证 |
| 5. 双屏 | **Mac** 在远控窗口切换两块显示器，记录名称、列表顺序和标注的像素尺寸。主屏排首位，画面各自对应 Windows 图案，100% / 150% 时边缘和比例正常。**Windows** 将副屏移至主屏上方，Mac 刷新显示器列表并复测。 | 待验证 |
| 6. 截图 | **Mac** 执行下方协议检查。**Windows** 将光标停在易辨认位置。PNG/JPEG 均可解码，长边请求小于原图时按比例缩小，更大时保持原尺寸。相同 PNG 请求只应在 `cursor:true` 时显示 Windows 光标；无参数 `screenshot` 返回主屏 PNG 且显示光标。 | 待验证 |
| 7. 热插拔 | **Windows** 断开副屏并改变主屏分辨率。**Mac** 刷新显示器列表，用旧 ID 发一次 `capture`，确认 `display_unavailable`。再插回副屏，核对同一设备路径时 ID 稳定，新截图对应当前物理像素尺寸。 | 待验证 |
| 8. 只读 | 在仅含 1.1 的构建上，**Mac** 验证截图可用，测试窗口不响应远程键鼠。Agent 获授权后可查看和截图，Agent 输入仍无法执行。 | 待验证 |

用 Mac 终端补测 PNG、JPEG、光标、主屏截图与壁纸。先退出 Mac 的远控会话。填入 Windows 控制台显示的设备 ID 本机执行，配对提示出现后在 Windows 控制台用当前编号同意。运行前确认安装路径与本机应用一致。

```bash
agent=/Applications/Koko.app/Contents/Helpers/koko-agent
testdir=$(mktemp -d)
printf 'Windows 设备 ID（9 位数字）: '
read -r device
"$agent" pair "$testdir/controller.key" "$device" --name '1.1 screenshot check' > "$testdir/pair.json"
```

以下步骤使用 Python 3 标准库。仅在临时目录保存现场测试截图，按需用「预览」打开。脚本在客户端退出前发送 `disconnect`。热插拔测试时也可用同一 `remote-stream` 入口单独提交之前记录的 ID。

```bash
python3 - "$agent" "$testdir" <<'PY'
import base64
import json
import pathlib
import subprocess
import sys

agent = sys.argv[1]
work = pathlib.Path(sys.argv[2])
pair = json.loads((work / "pair.json").read_text())
with subprocess.Popen(
    [agent, "remote-stream", str(work / "controller.key"), pair["host_id"], pair["address"]],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
) as client:
    def request(action, **args):
        client.stdin.write(json.dumps({"action": action, **args}) + "\n")
        client.stdin.flush()
        return json.loads(client.stdout.readline())

    def save(filename, reply):
        data = reply.pop("image", reply.pop("png", None))
        print(filename, reply)
        if data is not None:
            (work / filename).write_bytes(base64.b64decode(data))

    displays = request("displays")
    print(displays)
    assert displays["status"] == "displays", displays
    for display in displays["displays"]:
        identifier = display["id"]
        for fmt, cursor, edge in [("png", True, 640), ("png", False, 640), ("jpeg", False, 4096)]:
            save(f"{identifier}-{edge}-{cursor}.{fmt}", request(
                "capture", display=identifier, edge=edge, quality=80, format=fmt, cursor=cursor))
    save("primary.png", request("screenshot"))
    save("wallpaper.jpeg", request("wallpaper"))
    print(request("disconnect"))
    client.stdin.close()
print(work)
PY
```

## 子进程清理

在 Windows 上记录对应进程 PID。涉及强制终止时只对本次验收的 host 操作。每轮都检查对应 daemon 与后代是否仍在，以及重启后是否重新监听成功。

| 场景 | 操作与预期 | 实测记录 |
| --- | --- | --- |
| 正常退出 | **Windows** 在 host 控制台按 Ctrl+C，确认已结束远控、无残留进程。另测标准输入 EOF（如从无后续数据的管道启动）并确认退出。 | 待验证 |
| 宽限期耗尽 | **Windows** 用能暂停本次 daemon 的调试器保持 daemon 挂起，再在 host 控制台按 Ctrl+C。超过约 3 秒宽限期后 Job 应终止 daemon。 | 待验证 |
| 宿主异常与强制退出 | **Windows** 分别记录已启动后的 host 异常退出、启动期间强制终止、配对后强制终止。可从另一个 PowerShell 用 `Stop-Process -Id <本次 host PID> -Force` 验证强制终止。若未复现真正的异常退出，单独保留待验证。 | 待验证 |
| 已有 Job | **Windows** 从已有 Job 的启动器下启动，建立连接并重复退出验证。若上层 Job 禁止嵌套，记录原始 Win32 错误并确认无挂起进程。 | 待验证 |
| 安装路径 | **Windows** 把两个 exe 移到同一个含空格的目录，从该路径启动，并重复一次发现、截图和退出。 | 待验证 |
