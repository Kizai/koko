import AppKit

let logoURL = URL(fileURLWithPath: CommandLine.arguments[1])
let packageURL = URL(fileURLWithPath: CommandLine.arguments[2])
guard let logo = NSImage(contentsOf: logoURL) else {
    throw CocoaError(.fileReadCorruptFile)
}

guard NSWorkspace.shared.setIcon(logo, forFile: packageURL.path, options: []) else {
    throw CocoaError(.fileWriteUnknown)
}
try (packageURL as NSURL).setResourceValue(true, forKey: .hasHiddenExtensionKey)
