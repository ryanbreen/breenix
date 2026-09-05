import Foundation
@testable import BreenixRuns
import XCTest

final class HostFactsTests: XCTestCase {
    func testSampleParsesFixtureStringsWithoutRealProcesses() throws {
        let repoRoot = URL(fileURLWithPath: "/repo", isDirectory: true)
        let runner = FixtureProcessRunner(outputs: [
            "/usr/bin/pgrep -c qemu-system-aarch64": ProcessResult(stdout: Data("2\n".utf8), exitCode: 0),
            "/usr/bin/pgrep -c qemu-system-x86_64": ProcessResult(stdout: Data("3\n".utf8), exitCode: 0),
            "/usr/sbin/sysctl -n vm.loadavg": ProcessResult(stdout: Data("{ 1.23 2.34 3.45 }\n".utf8), exitCode: 0),
            "/bin/ps -o time= -p 4242": ProcessResult(stdout: Data("01:02.34\n".utf8), exitCode: 0),
            "/usr/bin/pmset -g therm": ProcessResult(stdout: Data("CPU_Speed_Limit     = 100\nScheduler_Limit     = 100\n".utf8), exitCode: 0),
            "/usr/sbin/sysctl hw.model hw.memsize": ProcessResult(stdout: Data("hw.model: Mac16,1\nhw.memsize: 34359738368\n".utf8), exitCode: 0),
            "/usr/bin/env qemu-system-aarch64 --version": ProcessResult(stdout: Data("QEMU emulator version 10.0.2\nCopyright\n".utf8), exitCode: 0),
            "/usr/bin/git rev-parse HEAD @ /repo": ProcessResult(stdout: Data("7a19f550abcdef\n".utf8), exitCode: 0),
            "/usr/bin/git status --porcelain @ /repo": ProcessResult(stdout: Data(" M tools/breenix-runs/README.md\n".utf8), exitCode: 0)
        ])

        let sample = try HostFacts.sample(
            runner: runner,
            repoRoot: repoRoot,
            wallTime: Date(timeIntervalSince1970: 1_788_632_000),
            qemuPID: 4242
        )

        XCTAssertEqual(sample.qemuPeersAarch64, 2)
        XCTAssertEqual(sample.qemuPeersX86_64, 3)
        XCTAssertEqual(sample.loadavg1, 1.23)
        XCTAssertEqual(sample.loadavg5, 2.34)
        XCTAssertEqual(sample.loadavg15, 3.45)
        XCTAssertNotNil(sample.qemuCPUSeconds)
        XCTAssertEqual(sample.qemuCPUSeconds!, 62.34, accuracy: 0.001)
        XCTAssertEqual(sample.thermalPressure, "CPU_Speed_Limit     = 100\nScheduler_Limit     = 100")
        XCTAssertEqual(sample.hostModel, "Mac16,1")
        XCTAssertEqual(sample.physMem, 34_359_738_368)
        XCTAssertEqual(sample.qemuVersion, "QEMU emulator version 10.0.2")
        XCTAssertEqual(sample.gitSHA, "7a19f550abcdef")
        XCTAssertEqual(sample.gitDirty, true)
        XCTAssertNil(sample.clockRatio)
    }

    func testProcessTimeParserHandlesHourMinuteSecondShape() {
        let seconds = HostFacts.parseProcessTime("1:02:03\n")
        XCTAssertNotNil(seconds)
        XCTAssertEqual(seconds!, 3_723, accuracy: 0.001)
    }

    func testProcessTimeParserHandlesMinuteSecondFractionShape() {
        let seconds = HostFacts.parseProcessTime("02:03.45\n")
        XCTAssertNotNil(seconds)
        XCTAssertEqual(seconds!, 123.45, accuracy: 0.001)
    }

    func testThermalPressureIsNilWhenUnavailable() throws {
        let runner = FixtureProcessRunner(outputs: [
            "/usr/bin/pgrep -c qemu-system-aarch64": ProcessResult(stdout: Data("0\n".utf8), exitCode: 1),
            "/usr/bin/pgrep -c qemu-system-x86_64": ProcessResult(stdout: Data("0\n".utf8), exitCode: 1),
            "/usr/sbin/sysctl -n vm.loadavg": ProcessResult(stdout: Data("{ 0.10 0.20 0.30 }\n".utf8), exitCode: 0),
            "/usr/bin/pmset -g therm": ProcessResult(stdout: Data(), stderr: Data("unsupported\n".utf8), exitCode: 1),
            "/usr/sbin/sysctl hw.model hw.memsize": ProcessResult(stdout: Data("hw.model: Mac16,1\nhw.memsize: 17179869184\n".utf8), exitCode: 0),
            "/usr/bin/env qemu-system-aarch64 --version": ProcessResult(stdout: Data("QEMU emulator version 10.0.2\n".utf8), exitCode: 0),
            "/usr/bin/git rev-parse HEAD @ /repo": ProcessResult(stdout: Data("abc123\n".utf8), exitCode: 0),
            "/usr/bin/git status --porcelain @ /repo": ProcessResult(stdout: Data(), exitCode: 0)
        ])

        let sample = try HostFacts.sample(runner: runner, repoRoot: URL(fileURLWithPath: "/repo", isDirectory: true))
        XCTAssertNil(sample.thermalPressure)
        XCTAssertEqual(sample.gitDirty, false)
    }
}

private struct FixtureProcessRunner: ProcessRunner {
    var outputs: [String: ProcessResult]

    func run(_ request: ProcessRequest, outputHandler: ((Data) -> Void)?) throws -> ProcessResult {
        let key = Self.key(for: request)
        guard let result = outputs[key] else {
            XCTFail("No fixture output for \(key)")
            return ProcessResult(exitCode: 127)
        }
        if request.combineOutput {
            outputHandler?(result.stdout)
        }
        return result
    }

    private static func key(for request: ProcessRequest) -> String {
        var key = ([request.executable] + request.arguments).joined(separator: " ")
        if let path = request.workingDirectory?.path {
            key += " @ \(path)"
        }
        return key
    }
}
