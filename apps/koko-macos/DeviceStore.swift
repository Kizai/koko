import AppKit
import Foundation
import Security

enum VerificationCodeMode: String, CaseIterable, Identifiable {
    case temporary
    case custom
    case both

    var id: Self { self }

    var title: String {
        switch self {
        case .temporary: "仅使用临时验证码"
        case .custom: "仅使用自定义验证码"
        case .both: "同时使用两种验证码"
        }
    }

    var hint: String {
        switch self {
        case .temporary: "每次远控结束刷新"
        case .custom: "使用固定自定义验证码"
        case .both: "临时与自定义验证码均有效"
        }
    }
}

func formatDeviceID(_ raw: String) -> String {
    let digits = raw.filter(\.isWholeNumber)
    guard digits.count == 9 else { return raw }
    let a = digits.prefix(3)
    let b = digits.dropFirst(3).prefix(3)
    let c = digits.suffix(3)
    return "\(a) \(b) \(c)"
}

enum KeychainCode {
    private static let service = "io.github.kizai.koko.custom-code"
    private static let account = "host"

    static func load() -> String {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data,
              let code = String(data: data, encoding: .utf8) else { return "" }
        return code
    }

    static func save(_ code: String) {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
        SecItemDelete(query as CFDictionary)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return }
        var item = query
        item[kSecValueData] = data
        SecItemAdd(item as CFDictionary, nil)
    }
}

struct DeviceRecord: Codable, Identifiable, Equatable {
    let id: String
    var deviceID: String
    var name: String
    var os: String?
    var addresses: [String]
    var lastConnectedAt: Date
}

enum DeviceHistoryStore {
    private static var baseDirectory: URL {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support", isDirectory: true)
        return appSupport.appendingPathComponent("Koko", isDirectory: true)
    }

    private static var devicesURL: URL {
        baseDirectory.appendingPathComponent("devices.json")
    }

    private static var coversDirectory: URL {
        baseDirectory.appendingPathComponent("covers", isDirectory: true)
    }

    static func loadRecords() -> [DeviceRecord] {
        guard let data = try? Data(contentsOf: devicesURL),
              let records = try? JSONDecoder().decode([DeviceRecord].self, from: data) else {
            return []
        }
        return records.sorted { $0.lastConnectedAt > $1.lastConnectedAt }
    }

    static func saveRecords(_ records: [DeviceRecord]) {
        do {
            try FileManager.default.createDirectory(at: baseDirectory, withIntermediateDirectories: true)
            let data = try JSONEncoder().encode(records)
            try data.write(to: devicesURL, options: .atomic)
        } catch {
            // Non-fatal: history persistence is best-effort.
        }
    }

    static func coverURL(for hostID: String) -> URL {
        let safe = hostID.filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }
        return coversDirectory.appendingPathComponent("\(safe).jpg")
    }

    static func loadCovers(for ids: [String]) -> [String: NSImage] {
        var result: [String: NSImage] = [:]
        for id in ids {
            if let image = NSImage(contentsOf: coverURL(for: id)) {
                result[id] = image
            }
        }
        return result
    }

    static func saveCover(_ data: Data, for hostID: String) -> NSImage? {
        guard let image = NSImage(data: data) else { return nil }
        do {
            try FileManager.default.createDirectory(at: coversDirectory, withIntermediateDirectories: true)
            try data.write(to: coverURL(for: hostID), options: .atomic)
        } catch {
            // Ignore disk write failure and still return the in-memory image.
        }
        return image
    }

    static func removeCover(for hostID: String) {
        try? FileManager.default.removeItem(at: coverURL(for: hostID))
    }
}
