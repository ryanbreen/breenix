import Foundation
import XCTest
@testable import BreenixRuns

final class X86QemuPolicyTests: XCTestCase {
    func testRequiresExactOperatorOverride() throws {
        for environment in [[:], ["BREENIX_ALLOW_X86_QEMU": "0"], ["BREENIX_ALLOW_X86_QEMU": "true"],
                            ["BREENIX_CATCHUP": "1"]] {
            XCTAssertThrowsError(try X86QemuPolicy.requireOperatorOverride(environment: environment)) { error in
                XCTAssertEqual(String(describing: error), X86QemuPolicy.refusal)
            }
        }
        XCTAssertNoThrow(try X86QemuPolicy.requireOperatorOverride(environment: ["BREENIX_ALLOW_X86_QEMU": "1"]))
    }
    func testLauncherRefusesWithoutOperatorOverride() throws {
        let runner = RefusalProcessRunner()
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), runner: runner,
                                     targetGuard: nil, x86QemuEnvironment: [:])
        let options = BeastLaunchOptions(sha: String(repeating: "a", count: 40))
        XCTAssertFalse(try launcher.plan(options: options).runGate.arguments.last!.contains("BREENIX_ALLOW_X86_QEMU"))
        XCTAssertThrowsError(try launcher.runX86(options: options)) { error in
            XCTAssertTrue(error is X86QemuPolicy)
        }
        XCTAssertTrue(runner.calls.isEmpty)
    }
    func testLauncherForwardsOperatorOverride() throws {
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), targetGuard: nil, x86QemuEnvironment: ["BREENIX_ALLOW_X86_QEMU": "1"])
        let plan = try launcher.plan(options: BeastLaunchOptions(sha: String(repeating: "a", count: 40)))
        XCTAssertTrue(plan.runGate.arguments.last!.contains(" BREENIX_ALLOW_X86_QEMU=1 "))
    }
}

private final class RefusalProcessRunner: ProcessRunner {
    var calls: [ProcessRequest] = []
    func run(_ request: ProcessRequest, outputHandler: ((Data) -> Void)?) throws -> ProcessResult {
        calls.append(request)
        return ProcessResult(exitCode: 0)
    }
}
