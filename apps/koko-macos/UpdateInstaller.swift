import AppKit
import Sparkle

/// Where an in-app update stands, for KoKo's own update window.
enum InstallPhase: Equatable {
    case idle
    case preparing
    case downloading(received: Int64, total: Int64?)
    case extracting(Double)
    case installing
    case failed(String)

    /// One bar for the whole update: download, then unpacking, then installing.
    var fraction: Double? {
        switch self {
        case .idle, .failed: nil
        case .preparing: 0
        case .downloading(let received, let total):
            total.map { $0 > 0 ? min(1, Double(received) / Double($0)) * 0.85 : 0 } ?? 0
        case .extracting(let progress): 0.85 + min(1, progress) * 0.1
        case .installing: 0.96
        }
    }

    var title: String {
        switch self {
        case .idle: ""
        case .preparing: "正在准备…"
        case .downloading(let received, let total):
            if let total {
                "正在下载 \(Self.size(received)) / \(Self.size(total))"
            } else {
                "正在下载 \(Self.size(received))"
            }
        case .extracting: "正在校验并解压…"
        case .installing: "安装中，完成后自动重新启动…"
        case .failed(let message): message
        }
    }

    private static func size(_ bytes: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
    }
}

/// Runs Sparkle with KoKo's own interface. The person confirms in the update window, so this
/// installs the chosen version without further prompts and relaunches when it is ready.
@MainActor
final class UpdateInstaller: NSObject, SPUUpdaterDelegate {
    private let driver = KokoUserDriver()
    private lazy var updater = SPUUpdater(hostBundle: .main, applicationBundle: .main,
                                          userDriver: driver, delegate: self)
    private var started = false
    private var feed: URL?

    init(report: @escaping (InstallPhase) -> Void) {
        super.init()
        driver.report = report
    }

    func install(version: String, feedURL: URL) throws {
        if !updater.sessionInProgress {
            feed = feedURL
            driver.requestedVersion = version
        }
        if !started {
            try updater.start()
            started = true
        }
        if updater.canCheckForUpdates { updater.checkForUpdates() }
    }

    func cancel() {
        driver.cancel()
    }

    func feedURLString(for updater: SPUUpdater) -> String? { feed?.absoluteString }
}

@MainActor
private final class KokoUserDriver: NSObject, SPUUserDriver {
    var requestedVersion: String?
    var report: (InstallPhase) -> Void = { _ in }
    private var cancelDownload: (() -> Void)?
    private var expected: Int64?
    private var received: Int64 = 0

    func cancel() {
        cancelDownload?()
        cancelDownload = nil
        report(.idle)
    }

    func show(_ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void) {
        // KoKo checks GitHub itself; Sparkle only installs what the person chose.
        reply(SUUpdatePermissionResponse(automaticUpdateChecks: false, sendSystemProfile: false))
    }

    func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) {
        report(.preparing)
    }

    func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState,
                         reply: @escaping (SPUUserUpdateChoice) -> Void) {
        guard appcastItem.versionString == requestedVersion, !appcastItem.isInformationOnlyUpdate else {
            reply(.dismiss)
            report(.failed("更新源里的版本与所选版本不一致，请稍后再试。"))
            return
        }
        reply(.install)
    }

    func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {}

    func showUpdateReleaseNotesFailedToDownloadWithError(_ error: any Error) {}

    func showUpdateNotFoundWithError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        report(.failed("没有找到可安装的更新包。"))
        acknowledgement()
    }

    func showUpdaterError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        cancelDownload = nil
        report(.failed("更新失败：" + error.localizedDescription))
        acknowledgement()
    }

    func showDownloadInitiated(cancellation: @escaping () -> Void) {
        cancelDownload = cancellation
        expected = nil
        received = 0
        report(.downloading(received: 0, total: nil))
    }

    func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {
        expected = expectedContentLength > 0 ? Int64(expectedContentLength) : nil
        report(.downloading(received: received, total: expected))
    }

    func showDownloadDidReceiveData(ofLength length: UInt64) {
        received += Int64(length)
        report(.downloading(received: received, total: expected))
    }

    func showDownloadDidStartExtractingUpdate() {
        cancelDownload = nil
        report(.extracting(0))
    }

    func showExtractionReceivedProgress(_ progress: Double) {
        report(.extracting(progress))
    }

    func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) {
        report(.installing)
        reply(.install)
    }

    func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool,
                              retryTerminatingApplication: @escaping () -> Void) {
        report(.installing)
    }

    func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        acknowledgement()
    }

    func dismissUpdateInstallation() {
        cancelDownload = nil
    }
}
