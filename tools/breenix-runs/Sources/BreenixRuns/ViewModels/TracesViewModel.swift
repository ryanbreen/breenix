import Foundation

public struct TracesViewModel: Equatable, Sendable {
    public var hostFacts: [BootFactsRecord]
    public var bxcap: BXCAPDecodeResult
    public var fatalRegs: [FatalRegsRecord]

    public init(hostFacts: [BootFactsRecord], bxcap: BXCAPDecodeResult, fatalRegs: [FatalRegsRecord]) {
        self.hostFacts = hostFacts
        self.bxcap = bxcap
        self.fatalRegs = fatalRegs
    }

    public static func build(serialIndex: SerialIndex, gateStdoutText: String) -> TracesViewModel {
        let serialText = mergedText(from: serialIndex)
        let bootFactsText = [serialText, gateStdoutText]
            .filter { !$0.isEmpty }
            .joined(separator: "\n")

        return TracesViewModel(
            hostFacts: BootFactsParser.parse(text: bootFactsText),
            bxcap: BXCAPDecoder.decode(serialLines: serialIndex.lines),
            fatalRegs: FatalRegsDecoder.decode(serialLines: serialIndex.lines)
        )
    }

    private static func mergedText(from index: SerialIndex) -> String {
        index.lines.map(\.text).joined(separator: "\n")
    }
}
