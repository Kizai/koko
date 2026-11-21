import Foundation

@main
struct KokoDesktop {
    static func main() async {
        do {
            try await run(Array(CommandLine.arguments.dropFirst()))
        } catch {
            let code = (error as? DesktopError)?.rawValue
                ?? (error as? ScreenCaptureError)?.rawValue
                ?? ScreenCaptureError.captureFailed.rawValue
            FileHandle.standardError.write(Data("\(code)\n".utf8))
            exit(1)
        }
    }

    private static func run(_ args: [String]) async throws {
        switch args.first {
        case "screen-permission":
            print(hasScreenCapturePermission(request: args.last == "request") ? "granted" : "required")
        case "input-permission":
            print(hasInputPermission(request: args.last == "request") ? "granted" : "required")
        case "screenshot":
            try FileHandle.standardOutput.write(contentsOf: await captureScreen())
        case "click":
            guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
            guard args.count == 3, let x = Double(args[1]), let y = Double(args[2]),
                  (0..<10_000).contains(x), (0..<10_000).contains(y)
            else { throw DesktopError.invalidCommand }
            try click(x, y)
        case "key":
            guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
            guard args.count == 2 else { throw DesktopError.invalidCommand }
            try postRemoteKey(args[1])
        case "text":
            guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
            let text = String(decoding: FileHandle.standardInput.readDataToEndOfFile(), as: UTF8.self)
            try postRemoteText(text)
        default:
            throw DesktopError.invalidCommand
        }
    }
}
