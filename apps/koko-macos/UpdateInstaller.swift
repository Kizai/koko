import AppKit
import Sparkle

@MainActor
final class UpdateInstaller: NSObject, SPUUpdaterDelegate {
    private let driver = InstallUserDriver(hostBundle: .main, delegate: nil)
    private lazy var updater = SPUUpdater(hostBundle: .main, applicationBundle: .main,
                                          userDriver: driver, delegate: self)
    private var started = false
    private var feed: URL?

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

    func feedURLString(for updater: SPUUpdater) -> String? { feed?.absoluteString }
}

@MainActor
private final class InstallUserDriver: SPUStandardUserDriver {
    var requestedVersion: String?

    override func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState,
                                  reply: @escaping (SPUUserUpdateChoice) -> Void) {
        // The KoKo button already confirmed this particular download. Keep Sparkle's
        // own install/relaunch confirmation when an update is already staged.
        if state.userInitiated, state.stage == .notDownloaded,
           appcastItem.versionString == requestedVersion, !appcastItem.isInformationOnlyUpdate {
            super.dismissUpdateInstallation()
            reply(.install)
        } else {
            super.showUpdateFound(with: appcastItem, state: state, reply: reply)
        }
    }
}
