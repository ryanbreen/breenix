import Foundation

public struct BootStage: Codable, Equatable, Sendable {
    public var name: String
    public var marker: String
    public var failureMeaning: String
    public var checkHint: String

    public init(name: String, marker: String, failureMeaning: String, checkHint: String) {
        self.name = name
        self.marker = marker
        self.failureMeaning = failureMeaning
        self.checkHint = checkHint
    }
}

public struct StageCatalogFile: Codable, Equatable, Sendable {
    public var schemaVersion: Int
    public var arch: Arch
    public var stages: [BootStage]

    public init(schemaVersion: Int, arch: Arch, stages: [BootStage]) {
        self.schemaVersion = schemaVersion
        self.arch = arch
        self.stages = stages
    }
}

public enum StageCatalogError: Error, Equatable, CustomStringConvertible {
    case missing(URL)
    case wrongSchema(Int)
    case wrongArch(expected: Arch, actual: Arch)
    case empty(Arch)

    public var description: String {
        switch self {
        case .missing(let url):
            return "boot stage catalog is missing at \(url.path)"
        case .wrongSchema(let version):
            return "unsupported boot stage catalog schemaVersion \(version)"
        case .wrongArch(let expected, let actual):
            return "boot stage catalog arch mismatch: expected \(expected.rawValue), got \(actual.rawValue)"
        case .empty(let arch):
            return "boot stage catalog for \(arch.rawValue) is empty"
        }
    }
}

public enum StageCatalog {
    public static func load(for arch: Arch) throws -> [BootStage] {
        guard let url = catalogURL(for: arch) else {
            throw StageCatalogError.missing(expectedCatalogURL(for: arch))
        }

        let file = try RunStore.decoder.decode(StageCatalogFile.self, from: Data(contentsOf: url))
        guard file.schemaVersion == 1 else {
            throw StageCatalogError.wrongSchema(file.schemaVersion)
        }
        guard file.arch == arch else {
            throw StageCatalogError.wrongArch(expected: arch, actual: file.arch)
        }
        guard !file.stages.isEmpty else {
            throw StageCatalogError.empty(arch)
        }
        return file.stages
    }

    public static func catalogURL(for arch: Arch) -> URL? {
        resourceBundle.url(
            forResource: "boot-stages-\(arch.rawValue)",
            withExtension: "json"
        )
    }

    private static func expectedCatalogURL(for arch: Arch) -> URL {
        resourceBundle.bundleURL.appendingPathComponent("boot-stages-\(arch.rawValue).json")
    }

    private static var resourceBundle: Bundle {
        Bundle.module
    }
}
