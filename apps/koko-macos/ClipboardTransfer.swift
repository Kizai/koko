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
        let text = board.string(forType: .string).flatMap { string in
            !string.isEmpty && string.utf8.count <= maxClipboardText && !string.contains("\0") ? string : nil
        }
        // "Copy Image" in a browser adds the image's address as text; the picture is what was meant.
        if let text, !(text.hasPrefix("http") && !text.contains(where: \.isWhitespace)) {
            return ClipboardItems(text: text)
        }
        // A screenshot taken to the clipboard or an image copied from a browser has no file, so it
        // is saved as one and sent like a copied file.
        if let png = pngData(from: board) {
            let folder = FileManager.default.temporaryDirectory
                .appendingPathComponent("KoKo-Clipboard", isDirectory: true)
                .appendingPathComponent(UUID().uuidString, isDirectory: true)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            let stamp = DateFormatter()
            stamp.dateFormat = "yyyy-MM-dd HH.mm.ss"
            let file = folder.appendingPathComponent("图片 \(stamp.string(from: Date())).png")
            try png.write(to: file)
            return ClipboardItems(paths: [file.path])
        }
        if let text { return ClipboardItems(text: text) }
        throw ClipboardError.unsupported
    }

    @MainActor
    private static func pngData(from board: NSPasteboard) -> Data? {
        if let png = board.data(forType: .png) { return png }
        guard let tiff = board.data(forType: .tiff) else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }

    /// Image types published next to a received image file, so it pastes as a picture in editors
    /// and chats while Finder still pastes the file.
    private static func imageRepresentations(of path: String) -> [(NSPasteboard.PasteboardType, Data)] {
        let url = URL(fileURLWithPath: path)
        guard ["png", "jpg", "jpeg", "heic", "tif", "tiff", "gif", "bmp", "webp"].contains(url.pathExtension.lowercased()),
              let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size <= 64 * 1024 * 1024,
              let data = try? Data(contentsOf: url), let image = NSBitmapImageRep(data: data)
        else { return [] }
        let png = url.pathExtension.lowercased() == "png" ? data : image.representation(using: .png, properties: [:])
        return [png.map { (.png, $0) }, image.tiffRepresentation.map { (.tiff, $0) }].compactMap { $0 }
    }

    /// Replaces the pasteboard with the received text or files.
    @MainActor
    func publish(on board: NSPasteboard) throws {
        board.clearContents()
        let published: Bool
        if let text {
            published = board.setString(text, forType: .string)
        } else if paths.count == 1, case let images = Self.imageRepresentations(of: paths[0]), !images.isEmpty {
            let item = NSPasteboardItem()
            item.setString(URL(fileURLWithPath: paths[0]).absoluteString, forType: .fileURL)
            for (type, data) in images { item.setData(data, forType: type) }
            published = board.writeObjects([item])
        } else {
            published = board.writeObjects(paths.map { URL(fileURLWithPath: $0) as NSURL })
        }
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
