import Foundation

/// Files harvested finished boots before their manifest becomes visible to importers.
public enum FinishedX86Registration {
    public static func file(script: URL, manifest: RunManifest, runDirectory: URL, runner: ProcessRunner) throws {
        let boots = try FileManager.default.contentsOfDirectory(at: runDirectory, includingPropertiesForKeys: nil)
            .filter { $0.lastPathComponent.hasPrefix("breenix_gate_") }.sorted { $0.path < $1.path }
        var filed = 0
        func recordID(_ suffix: String) -> String { filed == 0 ? manifest.id : manifest.id + "-" + suffix }
        for boot in boots {
            let resultFile = boot.appendingPathComponent("suite-results.json")
            if FileManager.default.fileExists(atPath: resultFile.path) {
                let rows = try JSONDecoder().decode([SuiteResult].self, from: Data(contentsOf: resultFile))
                for row in rows where row.verdict != "NOT-RUN" {
                    let directory = boot.appendingPathComponent("suite-" + row.suite)
                    try record(script: script, manifest: manifest, directory: directory,
                               id: recordID(boot.lastPathComponent + "-" + row.suite),
                               suite: row.suite, started: row.started, ended: row.ended,
                               status: row.verdict == "PASS" ? 0 : 1, runner: runner)
                    filed += 1
                }
                if rows.allSatisfy({ $0.verdict == "NOT-RUN" }) {
                    let times = try JSONDecoder().decode(BootTimes.self, from: Data(contentsOf: boot.appendingPathComponent("boot-times.json")))
                    try record(script: script, manifest: manifest, directory: boot, id: recordID(boot.lastPathComponent),
                               suite: rows.first?.suite, started: times.started, ended: times.ended, status: 1, runner: runner)
                    filed += 1
                }
            } else {
                let user = boot.appendingPathComponent("serial_user.log")
                let kernel = boot.appendingPathComponent("serial_kernel.log")
                guard FileManager.default.fileExists(atPath: user.path), FileManager.default.fileExists(atPath: kernel.path) else { continue }
                let timing = boot.appendingPathComponent("boot-times.json")
                let times = try JSONDecoder().decode(BootTimes.self, from: Data(contentsOf: timing))
                try record(script: script, manifest: manifest, directory: boot,
                           id: recordID(boot.lastPathComponent), suite: nil,
                           started: times.started, ended: times.ended,
                           status: manifest.verdict.isFailure ? 1 : 0, runner: runner)
                filed += 1
            }
        }
    }

    private static func record(script: URL, manifest: RunManifest, directory: URL, id: String,
                               suite: String?, started: Double, ended: Double, status: Int, runner: ProcessRunner) throws {
        let result = try runner.run(ProcessRequest(executable: script.path, arguments: [
            "record", "beast", suite == nil ? "tests" : "suite", suite ?? "",
            directory.appendingPathComponent("serial_kernel.log").path,
            directory.appendingPathComponent("serial_user.log").path,
            manifest.env["BREENIX_QEMU_PROFILE"] ?? "default", id, manifest.kernel.gitSHA ?? "",
            String(started), String(ended), String(status)
        ]))
        if result.exitCode != 0 {
            throw NSError(domain: "Vigil finished boot registration", code: Int(result.exitCode),
                          userInfo: [NSLocalizedDescriptionKey: result.stderrString])
        }
    }

    private struct SuiteResult: Decodable {
        var suite: String
        var verdict: String
        var started: Double
        var ended: Double
    }
    private struct BootTimes: Decodable {
        var started: Double
        var ended: Double
    }
}
