import Foundation

public struct X86QemuPolicy: Error, CustomStringConvertible {
    public var description: String { Self.refusal }
    public static let refusal = "x86 QEMU refused by the 2026-10-18 bare-metal-only policy (S23); use the R240 metal boot loop (scripts/metal/metal-run); operator override: BREENIX_ALLOW_X86_QEMU=1."

    public static func requireOperatorOverride(environment: [String: String] = ProcessInfo.processInfo.environment) throws {
        guard environment["BREENIX_ALLOW_X86_QEMU"] == "1" else { throw X86QemuPolicy() }
    }
}
