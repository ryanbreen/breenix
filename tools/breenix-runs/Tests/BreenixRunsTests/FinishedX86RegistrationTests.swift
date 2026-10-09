import Foundation
@testable import BreenixRuns
import XCTest

final class FinishedX86RegistrationTests: XCTestCase {
    func testEveryFinishedSuiteIsFiledOnceWithOwnSerialsAndTimes() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let boot = root.appendingPathComponent("breenix_gate_1")
        try FileManager.default.createDirectory(at: boot, withIntermediateDirectories: true)
        try Data("""
        [{"suite":"files-io","verdict":"PASS","started":10,"ended":20},
         {"suite":"directories","verdict":"FAIL","started":20,"ended":30},
         {"suite":"processes","verdict":"NOT-RUN","started":30,"ended":30}]
        """.utf8).write(to: boot.appendingPathComponent("suite-results.json"))
        let manifest = RunManifest(id: "finished", startedAt: Date(), endedAt: Date(), arch: .x86_64,
            profile: "gate", launcher: .beastSSH, kernel: KernelIdentity(gitSHA: String(repeating: "a", count: 40)),
            host: nil, verdict: .fail("later suite"), verdictSource: .gateScript(command: [], exitCode: 1),
            serials: [], captures: [], command: [], env: ["BREENIX_QEMU_PROFILE": "q35"], tags: [], notes: nil)
        let runner = Recorder()
        try FinishedX86Registration.file(script: URL(fileURLWithPath: "/record.sh"), manifest: manifest, runDirectory: root, runner: runner)
        XCTAssertEqual(runner.calls.count, 2, "finished runs must be filed; unstarted suites must not be filed")
        for (index, suite) in ["files-io", "directories"].enumerated() {
            let call = runner.calls[index]
            XCTAssertEqual(call.arguments[0], "record")
            XCTAssertEqual(call.arguments[3], suite)
            XCTAssertTrue(call.arguments[4].hasSuffix("suite-\(suite)/serial_kernel.log"))
            XCTAssertTrue(call.arguments[5].hasSuffix("suite-\(suite)/serial_user.log"))
            XCTAssertEqual(call.arguments[6], "q35")
            XCTAssertEqual(call.arguments[7], index == 0 ? "finished" : "finished-breenix_gate_1-directories")
            XCTAssertEqual(call.arguments[9], index == 0 ? "10.0" : "20.0")
            XCTAssertEqual(call.arguments[10], index == 0 ? "20.0" : "30.0")
            XCTAssertEqual(call.arguments[11], index == 0 ? "0" : "1")
        }
    }

    func testNoBootDirectoriesUsesManifestStatusExactlyOnce() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        for status in [130, 1, 255] {
            let manifest = RunManifest(id: "empty", startedAt: Date(), endedAt: Date(), arch: .x86_64,
                profile: "gate", launcher: .beastSSH, kernel: KernelIdentity(gitSHA: String(repeating: "a", count: 40)),
                host: nil, verdict: .fail("no boot"), verdictSource: .gateScript(command: [], exitCode: status),
                serials: [], captures: [], command: [], env: [:], tags: [], notes: nil)
            let runner = Recorder()
            try FinishedX86Registration.file(script: URL(fileURLWithPath: "/record.sh"), manifest: manifest, runDirectory: root, runner: runner)
            XCTAssertEqual(runner.calls.count, 1)
            XCTAssertEqual(runner.calls[0].arguments[11], String(status))
            XCTAssertTrue(runner.calls[0].arguments[4].hasSuffix("serial_kernel.txt"))
        }
    }

    private final class Recorder: ProcessRunner {
        var calls: [ProcessRequest] = []
        func run(_ request: ProcessRequest, outputHandler: (@Sendable (Data) -> Void)?) throws -> ProcessResult {
            calls.append(request)
            return ProcessResult(exitCode: 0)
        }
    }
}
