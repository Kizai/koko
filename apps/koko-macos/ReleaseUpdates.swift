import Combine
import Foundation

private struct ReleaseVersion: Comparable {
    let major: Int
    let minor: Int
    let patch: Int
    var string: String { "\(major).\(minor).\(patch)" }

    init?(_ string: String) {
        let parts = string.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3, let major = Int(parts[0]), let minor = Int(parts[1]), let patch = Int(parts[2]),
              major >= 0, minor >= 0, patch >= 0, "\(major).\(minor).\(patch)" == string else { return nil }
        self.major = major
        self.minor = minor
        self.patch = patch
    }

    static func < (lhs: Self, rhs: Self) -> Bool {
        (lhs.major, lhs.minor, lhs.patch) < (rhs.major, rhs.minor, rhs.patch)
    }
}

@MainActor
final class ReleaseUpdates: ObservableObject {
    struct Release: Equatable, Identifiable {
        let version: String
        var id: String { version }
        var pageURL: URL { Self.repository.appendingPathComponent("releases/tag/v\(version)") }
        var downloadURL: URL { Self.repository.appendingPathComponent("releases/download/v\(version)/Koko-\(version).dmg") }

        fileprivate static let repository = URL(string: "https://github.com/Kizai/koko")!
        fileprivate init(_ number: ReleaseVersion) { version = number.string }
    }

    enum State: Equatable {
        case idle, checking, upToDate, available(Release), failed(String)
    }

    let installedVersion: String?
    @Published private(set) var state: State = .idle
    @Published private(set) var automaticNotice: Release?

    private let defaults: UserDefaults
    private let load: (URLRequest) async throws -> (Data, URLResponse)
    private var pending: Task<Void, Never>?
    private var manualCheck = false

    init(installedVersion: String? = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
         defaults: UserDefaults = .standard,
         load: @escaping (URLRequest) async throws -> (Data, URLResponse) = { try await URLSession.shared.data(for: $0) }) {
        self.installedVersion = installedVersion
        self.defaults = defaults
        self.load = load
    }

    func check(manual: Bool) async {
        if let pending {
            if manual { manualCheck = true }
            await pending.value
            return
        }
        guard let installedVersion, let current = ReleaseVersion(installedVersion) else {
            state = .failed("无法读取当前应用的版本号。")
            return
        }
        if !manual, defaults.string(forKey: "releaseCheckedBuild") == installedVersion,
           let last = defaults.object(forKey: "releaseCheckedAt") as? Date,
           (0..<(24 * 60 * 60)).contains(Date().timeIntervalSince(last)) {
            if let saved = defaults.string(forKey: "releaseAvailableVersion"),
               let version = ReleaseVersion(saved), version > current {
                present(Release(version), automatically: true)
            } else if state == .idle, defaults.bool(forKey: "releaseCheckSucceeded") {
                state = .upToDate
            }
            return
        }

        defaults.set(installedVersion, forKey: "releaseCheckedBuild")
        defaults.set(Date(), forKey: "releaseCheckedAt")
        defaults.removeObject(forKey: "releaseAvailableVersion")
        defaults.set(false, forKey: "releaseCheckSucceeded")
        manualCheck = manual
        state = .checking
        let task = Task {
            defer { pending = nil }
            do {
                if let release = try await newestRelease(after: current) {
                    defaults.set(release.version, forKey: "releaseAvailableVersion")
                    present(release, automatically: !manualCheck)
                } else {
                    automaticNotice = nil
                    state = .upToDate
                }
                defaults.set(true, forKey: "releaseCheckSucceeded")
            } catch {
                state = .failed(error.localizedDescription)
            }
        }
        pending = task
        await task.value
    }

    func dismissAutomaticNotice() {
        if let automaticNotice { defaults.set(automaticNotice.version, forKey: "releaseDismissedVersion") }
        automaticNotice = nil
    }

    private func present(_ release: Release, automatically: Bool) {
        state = .available(release)
        if automatically {
            automaticNotice = defaults.string(forKey: "releaseDismissedVersion") == release.version ? nil : release
        } else if automaticNotice != release {
            automaticNotice = nil
        }
    }

    private struct GitHubRelease: Decodable {
        let tagName: String
        let htmlUrl: String
        let draft: Bool
        let prerelease: Bool
        let assets: [Asset]

        struct Asset: Decodable {
            let name: String
            let state: String
            let browserDownloadUrl: String
        }
    }

    private struct CheckError: LocalizedError {
        let errorDescription: String?
        init(_ description: String) { errorDescription = description }
    }

    private func newestRelease(after current: ReleaseVersion) async throws -> Release? {
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/Kizai/koko/releases?per_page=100")!)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, status) = try await fetch(request)
        if status == 403 || status == 429 {
            // KoKo's 0.x builds follow previews. The public feed includes those but does not identify them.
            guard current.major == 0 else { throw CheckError("GitHub 暂时限制了检查次数，请稍后再试。") }
            return try await newestFeedRelease(after: current)
        }
        guard status == 200 else { throw CheckError("GitHub 返回了 HTTP \(status)，请稍后再试。") }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let releases = try? decoder.decode([GitHubRelease].self, from: data) else {
            throw CheckError("无法读取 GitHub 发布信息，请稍后再试。")
        }

        let newest = releases.compactMap { candidate -> ReleaseVersion? in
            guard !candidate.draft, !candidate.prerelease || current.major == 0,
                  candidate.tagName.hasPrefix("v"), let version = ReleaseVersion(String(candidate.tagName.dropFirst())),
                  version > current else { return nil }
            let release = Release(version)
            guard candidate.htmlUrl == release.pageURL.absoluteString,
                  candidate.assets.contains(where: {
                      $0.state == "uploaded" && $0.name == release.downloadURL.lastPathComponent
                      && $0.browserDownloadUrl == release.downloadURL.absoluteString
                  }) else { return nil }
            return version
        }.max()
        return newest.map(Release.init)
    }

    private func newestFeedRelease(after current: ReleaseVersion) async throws -> Release? {
        let (data, status) = try await fetch(URLRequest(url: Release.repository.appendingPathComponent("releases.atom")))
        guard status == 200 else { throw CheckError("无法读取 GitHub 发布订阅，HTTP \(status)。") }
        let feed = ReleaseFeed()
        let parser = XMLParser(data: data)
        parser.delegate = feed
        guard parser.parse(), feed.isFeed else { throw CheckError("无法读取 GitHub 发布订阅，请稍后再试。") }

        for version in feed.versions.sorted(by: >) where version > current {
            let release = Release(version)
            var request = URLRequest(url: release.downloadURL)
            request.httpMethod = "HEAD"
            let (_, status) = try await fetch(request)
            if status == 200 { return release }
            if status != 404 { throw CheckError("无法确认 Mac 安装包，HTTP \(status)。") }
        }
        return nil
    }

    private func fetch(_ request: URLRequest) async throws -> (Data, Int) {
        var request = request
        request.timeoutInterval = 15
        request.cachePolicy = .reloadIgnoringLocalCacheData
        do {
            let (data, response) = try await load(request)
            guard let response = response as? HTTPURLResponse else { throw CheckError("GitHub 返回了无效响应。") }
            return (data, response.statusCode)
        } catch is URLError {
            throw CheckError("连接 GitHub 失败，请检查网络后重试。")
        }
    }
}

private final class ReleaseFeed: NSObject, XMLParserDelegate {
    var versions: [ReleaseVersion] = []
    var isFeed = false
    private var path: [String] = []

    func parser(_ parser: XMLParser, didStartElement element: String, namespaceURI: String?,
                qualifiedName: String?, attributes: [String: String]) {
        path.append(element)
        if path == ["feed"] { isFeed = true }
        guard path == ["feed", "entry", "link"], attributes["rel"] == "alternate",
              let href = attributes["href"], let tag = URL(string: href)?.lastPathComponent,
              tag.hasPrefix("v"), let number = ReleaseVersion(String(tag.dropFirst())),
              href == ReleaseUpdates.Release(number).pageURL.absoluteString else { return }
        versions.append(number)
    }

    func parser(_ parser: XMLParser, didEndElement element: String, namespaceURI: String?, qualifiedName: String?) {
        path.removeLast()
    }
}
