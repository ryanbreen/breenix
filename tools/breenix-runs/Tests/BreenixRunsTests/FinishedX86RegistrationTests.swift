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
            XCTAssertEqual(call.arguments[safe: 0], "record")
            XCTAssertEqual(call.arguments[safe: 3], suite)
            XCTAssertTrue(call.arguments[safe: 4]?.hasSuffix("suite-\(suite)/serial_kernel.log") == true)
            XCTAssertTrue(call.arguments[safe: 5]?.hasSuffix("suite-\(suite)/serial_user.log") == true)
            XCTAssertEqual(call.arguments[safe: 6], "q35")
            XCTAssertEqual(call.arguments[safe: 7], index == 0 ? "finished" : "finished-breenix_gate_1-directories")
            XCTAssertEqual(call.arguments[safe: 9], index == 0 ? "10.0" : "20.0")
            XCTAssertEqual(call.arguments[safe: 10], index == 0 ? "20.0" : "30.0")
            XCTAssertEqual(call.arguments[safe: 11], index == 0 ? "0" : "1")
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
            XCTAssertEqual(try XCTUnwrap(runner.calls.first).arguments[safe: 11], String(status))
            XCTAssertTrue(try XCTUnwrap(runner.calls.first).arguments[safe: 4]?.hasSuffix("serial_kernel.txt") == true)
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
