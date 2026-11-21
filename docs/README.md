# Koko 文档

| 想了解什么 | 从这里开始 |
| --- | --- |
| 项目目标、本地开发命令和路线图 | [简体中文](./overview.zh-CN.md) · [English](./overview.md) |
| 安装 macOS 测试包、选择角色和验证局域网连接 | [macOS 双机测试](./macos.md) |
| 本机系统权限、远端设备授权和截图验收 | [macOS 权限设计](./macos-permissions.md) |
| 模块职责、设备授权和通信协议 | [架构设计](./architecture.md) |
| 当前要交付什么、按什么顺序验收 | [Phase 1 实现计划](./implementation.md) |
| 如何设计、实现和评审改动 | [开发规则](./development.md) |

## 临时笔记

将需要到期删除的调研或实验记录放在 `docs/notes/` 下。给需要清理的 Markdown 笔记在第一行加上实际到期日，格式如下。

```markdown
<!-- expires-on: 2026-12-31 -->
```

GitHub Actions 在默认分支每天北京时间 03:17 检查。到期日当天起，带有效标记且已纳入 Git 的笔记会被删除，删除记录保存在 Git 历史中。把需要长期保留的结论移到上面的正式文档中。

在仓库根目录执行以下命令可预览将清理的笔记。

```bash
python3 .github/scripts/cleanup_expired_docs.py
```
