import Foundation

/// Registers a run with Vigil, the operator's dashboard, through scripts/vigil-record.sh, which does nothing when Vigil
/// is not installed. This process is the run's runner: Vigil shows the run as running while it lives.
enum VigilRegistration {
    static func start(root: URL, platform: String, mode: String, suite: String?, serial: String, userSerial: String?,
                      profile: String?, id: String, commit: String) -> String? {
        let output = run(root: root, arguments: ["start", platform, mode, suite ?? "", serial, userSerial ?? "", profile ?? "", id, commit])
        let id = output.trimmingCharacters(in: .whitespacesAndNewlines)
        return id.isEmpty ? nil : id
    }

    static func finish(root: URL, id: String?, exitStatus: Int) {
        guard let id else { return }
        _ = run(root: root, arguments: ["finish", id, String(exitStatus)])
    }

    private static func run(root: URL, arguments: [String]) -> String {
        let script = root.appendingPathComponent("scripts/vigil-record.sh")
        guard FileManager.default.isExecutableFile(atPath: script.path) else { return "" }
        let process = Process()
        process.executableURL = script
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return "" }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return String(decoding: data, as: UTF8.self)
    }
}
