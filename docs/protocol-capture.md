# macOS 协议样例采集

[返回文档目录](./README.md)

本清单是 Windows 方案 0.6 尚待执行的双 Mac 实机工作。当前只交付[格式说明](./architecture.md#desktop-request-与回复-json)，没有录制或编造回复、音视频样例，也没有把源码推导的数据当成 golden fixtures。

## 环境与记录

两台 Mac 分别作为被控端和人类控制端，使用已知提交的 Koko.app。在被控端运行 `./scripts/build-macos.sh --allow-ad-hoc` 构建测试应用，启动共享并授予屏幕录制、辅助功能权限，完成真实配对。裸 `serve` / `serve-pair` 不提供完整应用桥接；采集目标应是应用以 `--app-desktop` 启动的 daemon。

每次记录提交、应用和 macOS 版本、显示器、采集时间、编码器及请求选项。使用专用测试窗口、音频和剪贴板内容，不包含身份私钥、邀请或验证码。原始数据先保存在本机，不直接提交完整屏幕或私人 UI 树。

## Desktop 行协议

在临时测试构建中，对 `LocalConnection.swift` 的 host-daemon 管道增加本地 trace：在 `Desktop request: ` 分支解码前保存完整原始请求行，在 `command.send` 写入 `desktop-reply` 前保存实际编码后的回复行（含最终一个 LF）。只记录这两类行并保留请求编号。本次文档 PR 不修改 Swift；此处的临时 trace 由操作者在采集时执行。CLI 返回值已经经过 Rust 解码和再次序列化，不能替代原始本机回复。

逐项触发以下动作，保存对应请求和回复：

- 显示器枚举、主屏 PNG screenshot、指定显示器 JPEG/PNG capture、wallpaper、前台测试应用 ui_tree。
- 音量读取、受控设置再恢复原值；保留当前 `audio_volume` 实际缺字段的回复及 CLI 错误，不补写 level/muted。
- 测试窗口中的 pointer move/down/up、精确触控板及普通滚轮、左右修饰键 down/up、release，以及命名 key 和中英文/emoji text。
- 文本、单文件、含空文件夹的目录复制/粘贴，覆盖 clipboard_read、clipboard_target、clipboard_publish。文件粘贴才会请求目标目录。
- video 与独立 audio 握手；可用测试 Agent 收集一条安全、明确需要审批的动作及批准后的重试。

对照 architecture 中全部 15 种 action 核对覆盖；能力标记、可选字段、省略值和错误必须保留真实形态。特别区分本机 displays 的 outbox 与远端 control。

## 二进制媒体

先关闭 GUI 的视频订阅，避免占用唯一视频槽位。在人类控制端设置下列变量为已配对会话的真实值：`KOKO_AGENT`（辅助进程路径）、`KOKO_CONTROLLER_KEY`、`KOKO_HOST_ID`、`KOKO_HOST_ADDRESS`。不要把密钥内容复制进采集材料。

```sh
umask 077
mkdir -p capture
printf '%s\n' '{"action":"displays"}' |
  "$KOKO_AGENT" remote-stream "$KOKO_CONTROLLER_KEY" "$KOKO_HOST_ID" "$KOKO_HOST_ADDRESS" \
  > capture/displays.cli.jsonl

# 从真实 displays 回复选择显示器，设置 KOKO_DISPLAY_ID。
capture_media() {
  request=$(printf '{"action":"video","display":%s,"edge":1280,"quality":70,"fps":30,"audio":true,"feedback":%s}' \
    "$KOKO_DISPLAY_ID" "$1")
  sleep 20 |
    "$KOKO_AGENT" remote-video "$KOKO_CONTROLLER_KEY" "$KOKO_HOST_ID" "$KOKO_HOST_ADDRESS" "$request" \
    > "capture/media-feedback-$1.bin" 2> "capture/media-feedback-$1.stderr"
}
capture_media false
capture_media true
```

以上 JSON 只是采集命令的请求输入，不是输出样例。采集期间播放测试声音并移动窗口。`sleep` 管道保持 stdin 打开而不发送反馈；辅助进程自行确认类型 9。直接使用 `</dev/null` 会因 EOF 关闭辅助进程及连接。stdout 必须按二进制原样保存，stderr 单独保存。

依次读取四字节大端长度，确认 1–8 MiB，再精确读取相应字节，首字节为类型。验证存在类型 5、6：类型 5 固定头是 14 字节，余下 cookie 非空；类型 6 前四字节序号在同一编码器内连续，然后是 AAC 包。无反馈时应有类型 2，有反馈时应有 9、10；类型 4 是辅助进程插入的链路状态。保留完整记录后再裁切必要片段，记录裁切位置，不手工重构载荷。

当前主机不发送类型 7。若确需该历史样例，另行使用已确认可运行且发送该记录的历史构建（发送源码见 `9a20fec`，后在 `ff0cd1a` 移除），记录独立的版本和兼容会话；不能用当前帧加上手写时间戳替代。历史音频语义也须与当前类型 5/6 分开标注。

## 交回材料与后续测试

交回脱敏后的原始 JSONL、完整媒体记录片段、分离的 stderr、元数据和每个文件的 SHA-256（`shasum -a 256`）。脱敏仅改文本标识并附变更说明，不改字段集合、类型、可选字段存在性或二进制载荷；不宜公开的图像/音频应重新用测试内容采集。

收到真实材料后，再为 displays、ui_tree 和音视频格式增加 koko-proto 样例测试，并用于 0B 的字节级比对。当前阶段 0.6 的样例采集和测试仍待完成；阶段 0B 不因此提前开始。
