import AppKit
import SwiftUI

/// KoKo's update window: what changed, then one progress bar through download, unpacking and
/// installing, ending in a relaunch.
struct UpdateWindow: View {
    @ObservedObject var updates: ReleaseUpdates
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Group {
            if let release = updates.offered {
                content(release)
                    .task(id: release.version) { await updates.loadNotes(for: release) }
            } else {
                Text("当前没有可用更新。")
                    .foregroundStyle(.secondary)
                    .frame(width: 520, height: 160)
            }
        }
    }

    private func content(_ release: ReleaseUpdates.Release) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 16) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable()
                    .frame(width: 64, height: 64)
                VStack(alignment: .leading, spacing: 4) {
                    Text("有新版本可用").font(.title2.weight(.semibold))
                    Text("KoKo \(release.version)" + (updates.installedVersion.map { "（当前 \($0)）" } ?? ""))
                        .foregroundStyle(.secondary)
                }
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    if let notes = updates.notes[release.version] {
                        ReleaseNotesView(markdown: notes)
                    } else {
                        Text("正在读取更新说明…").foregroundStyle(.secondary)
                        Link("在 GitHub 查看更新说明", destination: release.pageURL)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
            }
            .frame(height: 220)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))
            footer(release)
        }
        .padding(24)
        .frame(width: 560)
    }

    @ViewBuilder
    private func footer(_ release: ReleaseUpdates.Release) -> some View {
        let phase = updates.installPhase
        if let fraction = phase.fraction {
            VStack(alignment: .leading, spacing: 8) {
                ProgressView(value: fraction)
                HStack {
                    Text(phase.title).foregroundStyle(.secondary)
                    Spacer()
                    Text("\(Int(fraction * 100))%").monospacedDigit().foregroundStyle(.secondary)
                }
                .font(.callout)
                if case .downloading = phase {
                    HStack {
                        Spacer()
                        Button("取消", action: updates.cancelInstall)
                    }
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 10) {
                if case .failed(let message) = phase {
                    Text(message).foregroundStyle(.red).textSelection(.enabled)
                }
                Text("更新完成后 KoKo 会自动重新启动，正在进行的远程会话会断开。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                HStack {
                    Spacer()
                    Button("稍后") { dismiss() }
                        .keyboardShortcut(.cancelAction)
                    Button(phase == .idle ? "立即更新" : "重试") { updates.install(release) }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.borderedProminent)
                }
            }
        }
    }
}

/// Renders release notes written in Markdown: headings, bullets and inline styles, line by line.
private struct ReleaseNotesView: View {
    let markdown: String

    var body: some View {
        ForEach(Array(markdown.components(separatedBy: .newlines).enumerated()), id: \.offset) { _, line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#") {
                Text(inline(trimmed.drop(while: { $0 == "#" }).trimmingCharacters(in: .whitespaces)))
                    .font(.headline)
                    .padding(.top, 6)
            } else if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text("•")
                    Text(inline(String(trimmed.dropFirst(2))))
                }
            } else if !trimmed.isEmpty {
                Text(inline(trimmed))
            }
        }
        .textSelection(.enabled)
    }

    private func inline(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text)) ?? AttributedString(text)
    }
}
