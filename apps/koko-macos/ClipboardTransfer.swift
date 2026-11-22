import AppKit

struct ClipboardPacket: Codable {
    enum Kind: String, Codable { case text, file, data, end }
    let kind: Kind
    var name: String?
    var size: UInt64?
    var bytes: Data?
}

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

actor ClipboardSource {
    static let chunk = 64 * 1024
    private var files: ArraySlice<URL> = []
    private var text: Data?
    private var file: FileHandle?
    private var remaining: UInt64 = 0

    private init(files: [URL] = [], text: Data? = nil) {
        self.files = files[...]
        self.text = text
    }

    @MainActor static func read(from board: NSPasteboard) async throws -> ClipboardSource {
        try await snapshot(named: board.name, changeCount: board.changeCount)
    }

    private static func snapshot(named name: NSPasteboard.Name, changeCount: Int) async throws -> ClipboardSource {
        let board = NSPasteboard(name: name)
        guard board.changeCount == changeCount else { throw ClipboardError.unchanged }
        let source: ClipboardSource
        if board.types?.contains(.fileURL) == true {
            let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
            guard !urls.isEmpty else { throw ClipboardError.unsupported }
            source = ClipboardSource(files: urls)
        } else if let string = board.string(forType: .string), string.utf8.count <= Self.chunk {
            source = ClipboardSource(text: Data(string.utf8))
        } else {
            throw ClipboardError.unsupported
        }
        guard board.changeCount == changeCount else { throw ClipboardError.unchanged }
        return source
    }

    func next() throws -> ClipboardPacket {
        if let text {
            self.text = nil
            return ClipboardPacket(kind: .text, bytes: text)
        }
        if remaining > 0, let file {
            guard let bytes = try file.read(upToCount: min(Self.chunk, Int(clamping: remaining))), !bytes.isEmpty
            else { throw ClipboardError.failed }
            remaining -= UInt64(bytes.count)
            if remaining == 0 { try file.close(); self.file = nil }
            return ClipboardPacket(kind: .data, bytes: bytes)
        }
        guard let url = files.popFirst() else { return ClipboardPacket(kind: .end) }
        let values = try url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
        guard values.isRegularFile == true, values.isSymbolicLink != true else { throw ClipboardError.unsupported }
        let handle = try FileHandle(forReadingFrom: url)
        remaining = try handle.seekToEnd()
        try handle.seek(toOffset: 0)
        if remaining > 0 { file = handle } else { try handle.close() }
        return ClipboardPacket(kind: .file, name: url.lastPathComponent, size: remaining)
    }
}

private enum ReceivedClipboard: Sendable {
    case text(String)
    case files([URL])
}

@MainActor
final class ClipboardDestination {
    private let storage = ClipboardStorage()
    private var published = false
    private var cancelled = false

    func accept(_ packet: ClipboardPacket, on board: NSPasteboard, whileActive: () -> Bool) async throws {
        guard !cancelled else { throw CancellationError() }
        guard let selection = try await storage.receive(packet) else { return }
        guard !cancelled, whileActive() else { throw ClipboardError.unchanged }
        board.clearContents()
        switch selection {
        case .text(let text): published = board.setString(text, forType: .string)
        case .files(let urls): published = board.writeObjects(urls as [NSURL])
        }
        guard published else { throw ClipboardError.failed }
    }

    func discard() {
        cancelled = true
        if !published { Task { await storage.discard() } }
    }
}

private actor ClipboardStorage {
    private var staging: URL?
    private var urls: [URL] = []
    private var text: String?
    private var file: FileHandle?
    private var remaining: UInt64 = 0

    func discard() {
        try? file?.close()
        file = nil
        if let staging { try? FileManager.default.removeItem(at: staging); self.staging = nil }
    }

    func receive(_ packet: ClipboardPacket) throws -> ReceivedClipboard? {
        switch packet.kind {
        case .text:
            guard urls.isEmpty, text == nil, let bytes = packet.bytes, bytes.count <= ClipboardSource.chunk,
                  let value = String(data: bytes, encoding: .utf8) else { throw ClipboardError.invalid }
            text = value
        case .file:
            guard remaining == 0, text == nil, let name = packet.name, !name.isEmpty,
                  name != ".", name != "..", !name.contains("/"), !name.contains("\0"),
                  let size = packet.size else { throw ClipboardError.invalid }
            let manager = FileManager.default
            if staging == nil {
                let root = UserDefaults.standard.string(forKey: "downloadDirectory") ?? defaultDownloadDirectory
                let directory = URL(fileURLWithPath: root, isDirectory: true)
                    .appendingPathComponent("KoKo-" + UUID().uuidString, isDirectory: true)
                try manager.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
                staging = directory
            }
            guard let staging else { throw ClipboardError.failed }
            // Separate containers also preserve two selected files with the same basename.
            let parent = staging.appendingPathComponent(String(urls.count), isDirectory: true)
            try manager.createDirectory(at: parent, withIntermediateDirectories: false)
            let url = parent.appendingPathComponent(name)
            try Data().write(to: url, options: .withoutOverwriting)
            if size > 0 { file = try FileHandle(forWritingTo: url) }
            urls.append(url)
            remaining = size
        case .data:
            guard let file, let bytes = packet.bytes, !bytes.isEmpty, bytes.count <= ClipboardSource.chunk,
                  UInt64(bytes.count) <= remaining else { throw ClipboardError.invalid }
            try file.write(contentsOf: bytes)
            remaining -= UInt64(bytes.count)
            if remaining == 0 { try file.close(); self.file = nil }
        case .end:
            guard remaining == 0, text != nil || !urls.isEmpty else { throw ClipboardError.invalid }
            if let text { return .text(text) }
            return .files(urls)
        }
        return nil
    }
}

@MainActor
final class HostClipboard {
    let id: String
    var source: ClipboardSource?
    var destination: ClipboardDestination?
    var lastActivity = ContinuousClock.now
    init(id: String) { self.id = id }
}
