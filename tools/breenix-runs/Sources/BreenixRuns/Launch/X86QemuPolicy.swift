import Foundation

public struct X86QemuPolicy: Error, CustomStringConvertible {
    public var description: String { Self.refusal }
    public static let refusal = "x86 QEMU execution is retired in breenix-runs under S23; use the x86-metal boot loop (scripts/metal/metal-run). BREENIX_ALLOW_X86_QEMU=1 permits only --dry-run planning."

    public static func requireOperatorOverride(environment: [String: String] = ProcessInfo.processInfo.environment) throws {
        guard environment["BREENIX_ALLOW_X86_QEMU"] == "1" else { throw X86QemuPolicy() }
    }
}
