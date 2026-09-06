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
        let serialFacts = BootFactsParser.parse(serialLines: serialIndex.lines)
            .map { record in
                var record = record
                record.sourceFile = "serial.txt"
                return record
            }
        let gateStdoutFacts = BootFactsParser.parse(text: gateStdoutText)
            .map { record in
                var record = record
                record.sourceFile = "gate-stdout.txt"
                return record
            }

        return TracesViewModel(
            hostFacts: serialFacts + gateStdoutFacts,
            bxcap: BXCAPDecoder.decode(serialLines: serialIndex.lines),
            fatalRegs: FatalRegsDecoder.decode(serialLines: serialIndex.lines)
        )
    }
}
