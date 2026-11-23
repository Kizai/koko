import AppKit

enum ClipboardError: String, Error {
    case unsupported = "clipboard_unsupported"
    case invalid = "clipboard_invalid"
    case unchanged = "clipboard_unchanged"
    case failed = "clipboard_failed"
    case permission = "clipboard_file_permission"
    case timeout = "clipboard_timeout"

    static func from(_ error: Error) -> Self {
        if let clipboard = error as? Self { return clipboard }
        var cause: NSError? = error as NSError
        while let current = cause {
            if current.domain == NSCocoaErrorDomain,
               [NSFileReadNoPermissionError, NSFileWriteNoPermissionError].contains(current.code) { return .permission }
            if current.domain == NSPOSIXErrorDomain, [Int(EACCES), Int(EPERM)].contains(current.code) { return .permission }
            cause = current.userInfo[NSUnderlyingErrorKey] as? NSError
        }
        return .failed
    }
}

/// Text up to this size travels with the request; files and folders of any size are streamed.
let maxClipboardText = 1024 * 1024

/// What a pasteboard holds, in the form the Rust transfer expects: text, or absolute paths of
/// files and folders. The bytes themselves are read and written by `koko-agent` and the daemon.
struct ClipboardItems: Codable, Equatable {
    var text: String?
    var paths: [String] = []

    @MainActor
    static func read(from board: NSPasteboard) throws -> ClipboardItems {
        if board.types?.contains(.fileURL) == true {
            let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
            guard !urls.isEmpty else { throw ClipboardError.unsupported }
            return ClipboardItems(paths: urls.map(\.path))
        }
        if let string = board.string(forType: .string), !string.isEmpty, string.utf8.count <= maxClipboardText,
           !string.contains("\0") {
            return ClipboardItems(text: string)
        }
        throw ClipboardError.unsupported
    }

    /// Replaces the pasteboard with the received text or files.
    @MainActor
    func publish(on board: NSPasteboard) throws {
        board.clearContents()
        let published = if let text { board.setString(text, forType: .string) }
            else { board.writeObjects(paths.map { URL(fileURLWithPath: $0) as NSURL }) }
        guard published else { throw ClipboardError.failed }
    }
}

/// A new private folder under the chosen download location for one received transfer.
func makeClipboardFolder() throws -> URL {
    let root = UserDefaults.standard.string(forKey: "downloadDirectory") ?? defaultDownloadDirectory
    let folder = URL(fileURLWithPath: root, isDirectory: true).appendingPathComponent("KoKo-" + UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    return folder
}

/// One line of `koko-agent clipboard-pull` or `clipboard-push` output.
struct ClipboardProgressLine: Decodable {
    var progress: Int64?
    var total: Int64?
    var status: String?
    var code: String?
    var text: String?
    var paths: [String]?
}
