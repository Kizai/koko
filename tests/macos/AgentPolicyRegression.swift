import AppKit

extension DesktopRegression {
    @MainActor static func checkAgentPolicy() throws {
        guard shortcutName(code: 12, flags: 0x10) == "⌘Q", shortcutName(code: 12, flags: 0x10 | 0x01) == "⌘Q",
              shortcutName(code: 51, flags: 0x12) == "⇧⌘⌫", shortcutName(code: 12, flags: 0) == nil,
              shortcutName(code: 8, flags: 0x10) == nil
        else { throw Failure("高危快捷键应只匹配退出、注销、锁屏、删除和强制退出") }

        let partial = try JSONDecoder().decode(AgentPolicy.self, from: Data(#"{"keywords":["Wipe"]}"#.utf8))
        guard partial.keywords == ["Wipe"], partial.shortcuts, partial.commandApps.contains("com.apple.Terminal"),
              partial.matchingKeyword(in: ["Wipe Disk…"]) == "Wipe", partial.matchingKeyword(in: ["Delete"]) == nil
        else { throw Failure("策略文件缺少的项应保留默认值，关键词不区分大小写") }
        guard AgentPolicy().matchingKeyword(in: ["移到废纸篓", "永久删除"]) == "删除"
        else { throw Failure("默认关键词应覆盖删除类按钮") }

        // The test binary has no bundle identifier, so an empty entry stands for "this app".
        var policy = AgentPolicy()
        policy.commandApps = [""]
        policy.keywords = []
        let current = NSRunningApplication.current
        let typing = RemoteAction(action: "text", text: "rm -rf ~\n", display: CGMainDisplayID())
        guard policy.check(typing, frontmost: current)?.rule == "command:" else {
            throw Failure("在终端类应用里输入文字应需要确认")
        }
        let quit = RemoteAction(action: "pointer", display: CGMainDisplayID(),
                                events: [RemotePointerEvent(kind: .keyDown, code: 12, flags: 0x10, repeat: false)])
        guard policy.check(quit, frontmost: current)?.rule == "shortcut:⌘Q" else {
            throw Failure("⌘Q 应需要确认")
        }
        policy.commandApps = []
        let move = RemoteAction(action: "pointer", display: CGMainDisplayID(),
                                events: [RemotePointerEvent(kind: .move, x: 10, y: 10)])
        guard policy.check(move, frontmost: current) == nil, policy.check(typing, frontmost: current) == nil else {
            throw Failure("普通移动和普通应用里的输入不应需要确认")
        }
    }
}
