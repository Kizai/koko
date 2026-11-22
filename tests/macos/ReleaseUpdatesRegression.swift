import Foundation

extension DesktopRegression {
    @MainActor static func checkReleaseUpdates() async throws {
        let suite = "koko.tests.updates." + UUID().uuidString
        guard let defaults = UserDefaults(suiteName: suite) else { throw Failure("无法隔离更新测试偏好") }
        defer { defaults.removePersistentDomain(forName: suite) }
        let repository = "https://github.com/Kizai/koko"
        func assetRelease(_ version: String, _ preview: Bool = true, draft: Bool = false,
                          uploaded: Bool = true, canonical: Bool = true) -> [String: Any] {
            let filename = "Koko-\(version).dmg"
            return ["tag_name": "v\(version)", "prerelease": preview, "draft": draft,
                    "html_url": "\(repository)/releases/tag/v\(version)",
                    "assets": [["name": filename, "state": uploaded ? "uploaded" : "open",
                                "browser_download_url": canonical ? "\(repository)/releases/download/v\(version)/\(filename)" : "https://example.org/\(filename)"]]]
        }
        let records = [assetRelease("0.1.9"), assetRelease("0.1.11"), assetRelease("0.1.10"),
                       assetRelease("0.2.0", draft: true), assetRelease("0.3.0", uploaded: false),
                       assetRelease("0.4.0", canonical: false), assetRelease("0.1.011"), assetRelease("beta")]
        var payload = try JSONSerialization.data(withJSONObject: records)
        var apiStatus = 200
        var requests: [URLRequest] = []
        let feed = """
        <feed xmlns="http://www.w3.org/2005/Atom">
        <entry><link rel="alternate" href="\(repository)/releases/tag/v0.1.14" /></entry>
        <entry><link rel="alternate" href="https://github.com/other/koko/releases/tag/v9.9.9" /></entry>
        <entry><link rel="alternate" href="\(repository)/releases/tag/v0.1.10" /></entry>
        </feed>
        """
        var gate: CheckedContinuation<Void, Never>?
        var pause = true
        let load: (URLRequest) async throws -> (Data, URLResponse) = { request in
            requests.append(request)
            guard let url = request.url else { throw Failure("无请求地址") }
            var status = apiStatus
            var data = payload
            if pause {
                pause = false
                await withCheckedContinuation { gate = $0 }
            }
            switch url.host {
            case "api.github.com":
                guard url.path == "/repos/Kizai/koko/releases", request.httpMethod == "GET" else {
                    throw Failure("必须使用发布列表以发现内测版")
                }
            case "github.com" where url.path == "/Kizai/koko/releases.atom":
                status = 200
                data = Data(feed.utf8)
            case "github.com" where request.httpMethod == "HEAD":
                status = url.lastPathComponent == "Koko-0.1.10.dmg" ? 200 : 404
                data = Data()
            default: throw Failure("意外的网络请求")
            }
            return (data, HTTPURLResponse(url: url, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }

        let checker = ReleaseUpdates(installedVersion: "0.1.8", defaults: defaults, load: load)
        let first = Task { await checker.check(manual: false) }
        while gate == nil { await Task.yield() }
        Task { gate?.resume() }
        await checker.check(manual: true)
        await first.value
        guard requests.count == 1, case .available(let release) = checker.state, release.version == "0.1.11",
              release.downloadURL.absoluteString == "\(repository)/releases/download/v0.1.11/Koko-0.1.11.dmg",
              checker.automaticNotice == nil else { throw Failure("并发手动和自动检查应共用请求并得到最新可用安装包") }

        let restored = ReleaseUpdates(installedVersion: "0.1.8", defaults: defaults, load: load)
        await restored.check(manual: false)
        guard requests.count == 1, restored.automaticNotice == release else { throw Failure("24 小时内重启应恢复可用更新") }
        restored.dismissAutomaticNotice()
        let dismissed = ReleaseUpdates(installedVersion: "0.1.8", defaults: defaults, load: load)
        await dismissed.check(manual: false)
        await dismissed.check(manual: true)
        guard requests.count == 2, dismissed.automaticNotice == nil, dismissed.state == .available(release) else {
            throw Failure("已忽略的版本仍应支持手动检查")
        }

        defaults.set(Date(timeIntervalSinceNow: -25 * 3600), forKey: "releaseCheckedAt")
        payload = try JSONSerialization.data(withJSONObject: [assetRelease("0.1.12")])
        await dismissed.check(manual: false)
        guard requests.count == 3, dismissed.automaticNotice?.version == "0.1.12" else {
            throw Failure("超过一天应重新检查并提醒更新的版本")
        }
        apiStatus = 403
        await dismissed.check(manual: true)
        guard case .available(let fallback) = dismissed.state, fallback.version == "0.1.10",
              requests.suffix(2).compactMap({ $0.url?.lastPathComponent }) == ["Koko-0.1.14.dmg", "Koko-0.1.10.dmg"] else {
            throw Failure("限流时应通过订阅跳过没有安装包的版本")
        }
        let stable = ReleaseUpdates(installedVersion: "1.0.0", defaults: defaults, load: load)
        await stable.check(manual: true)
        guard case .failed = stable.state, requests.last?.url?.host == "api.github.com" else {
            throw Failure("正式版遇到限流时应提示重试")
        }
        apiStatus = 200
        payload = try JSONSerialization.data(withJSONObject: [assetRelease("1.2.0"), assetRelease("1.1.0", false)])
        await stable.check(manual: true)
        guard case .available(let stableRelease) = stable.state, stableRelease.version == "1.1.0" else {
            throw Failure("正式版只应接受正式发布的安装包")
        }
        payload = try JSONSerialization.data(withJSONObject: records)
        await stable.check(manual: true)
        guard stable.state == .upToDate else { throw Failure("已安装较新版本时不能提示降级") }
        payload = Data("invalid".utf8)
        await stable.check(manual: true)
        guard case .failed = stable.state else { throw Failure("无效的发布数据应允许重试") }
        let count = requests.count
        let unversioned = ReleaseUpdates(installedVersion: nil, defaults: defaults, load: load)
        await unversioned.check(manual: true)
        guard case .failed = unversioned.state, requests.count == count else { throw Failure("版本号缺失时不应查询发布列表") }

        let offline = ReleaseUpdates(installedVersion: "0.1.8", defaults: defaults, load: { _ in throw URLError(.notConnectedToInternet) })
        await offline.check(manual: true)
        guard case .failed = offline.state, offline.automaticNotice == nil else { throw Failure("网络断开时应返回失败状态") }
    }

    @MainActor static func checkLiveReleaseUpdates(installedVersion: String) async throws {
        let name = "koko.tests.updates.live." + UUID().uuidString
        guard let defaults = UserDefaults(suiteName: name) else { throw Failure("无法隔离更新测试偏好") }
        defer { defaults.removePersistentDomain(forName: name) }
        let updates = ReleaseUpdates(installedVersion: installedVersion, defaults: defaults)
        await updates.check(manual: true)
        switch updates.state {
        case .available(let release): print("GitHub 实时更新检查：\(installedVersion) → \(release.version)，\(release.downloadURL)")
        case .upToDate: print("GitHub 实时更新检查：\(installedVersion) 已是最新可用版本。")
        case .failed(let message): throw Failure(message)
        default: throw Failure("GitHub 实时更新检查尚未完成")
        }
    }
}
