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
    func testLauncherForwardsOperatorOverride() throws {
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), targetGuard: nil)
        let plan = try launcher.plan(options: BeastLaunchOptions(sha: String(repeating: "a", count: 40)))
        XCTAssertTrue(plan.runGate.arguments.last!.contains(" BREENIX_ALLOW_X86_QEMU=1 "))
    }
}
