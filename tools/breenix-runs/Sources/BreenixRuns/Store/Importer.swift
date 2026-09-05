import Foundation

public enum ImporterError: Error, Equatable, CustomStringConvertible {
    case unreadablePath(String)

    public var description: String {
        switch self {
        case .unreadablePath(let path):
            return "path does not exist or is not readable: \(path)"
        }
    }
}

public struct ImportedRun: Equatable {
    public var id: String
    public var sourcePath: String

    public init(id: String, sourcePath: String) {
        self.id = id
        self.sourcePath = sourcePath
    }
}

public struct ImportSkip: Equatable {
    public var path: String
    public var reason: String

    public init(path: String, reason: String) {
        self.path = path
        self.reason = reason
    }
}

public struct ImportPathResult: Equatable {
    public var sourcePath: String
    public var imported: [ImportedRun]
    public var skipped: [ImportSkip]

    public init(sourcePath: String, imported: [ImportedRun] = [], skipped: [ImportSkip] = []) {
        self.sourcePath = sourcePath
        self.imported = imported
        self.skipped = skipped
    }
}

public struct Importer {
    private struct GateInfo {
        var arch: Arch
        var profile: String
    }

    private struct SerialSource {
        var url: URL
        var destinationName: String
        var stream: SerialStream
    }

    private struct CaptureSource {
        var url: URL
        var destinationName: String
    }

    private struct PreservedFailureEntry {
        var sourceURL: URL
        var serials: [SerialSource]
        var captures: [CaptureSource]
        var startedAt: Date
        var arch: Arch
        var profile: String
        var verdict: Verdict
    }

    private let store: RunStore
    private let fileManager: FileManager
    private let scanner: MarkerScanner

    public init(store: RunStore, fileManager: FileManager = .default, scanner: MarkerScanner = MarkerScanner()) {
        self.store = store
        self.fileManager = fileManager
        self.scanner = scanner
    }

    public func importPath(_ url: URL) throws -> ImportPathResult {
        let sourceURL = url.standardizedFileURL
        guard fileManager.fileExists(atPath: sourceURL.path),
              fileManager.isReadableFile(atPath: sourceURL.path) else {
            throw ImporterError.unreadablePath(sourceURL.path)
        }

        var isDirectory: ObjCBool = false
        _ = fileManager.fileExists(atPath: sourceURL.path, isDirectory: &isDirectory)
        if !isDirectory.boolValue {
            return try importSingleFile(sourceURL)
        }

        if isPreservedFailureContainer(sourceURL) {
            return try importPreservedFailures(in: sourceURL, sourcePath: sourceURL.path)
        }

        if isProdFailureRunDirectory(sourceURL) {
            var result = ImportPathResult(sourcePath: sourceURL.path)
            try importProdFailureRun(sourceURL, into: &result)
            return result
        }

        if let gateInfo = gateInfo(for: sourceURL) {
            var result = ImportPathResult(sourcePath: sourceURL.path)
            try importGateIteration(sourceURL, info: gateInfo, into: &result)
            return result
        }

        if sourceURL.lastPathComponent == "breenix_aarch64_testing_profile" {
            var result = ImportPathResult(sourcePath: sourceURL.path)
            for child in try childDirectories(of: sourceURL) where isIntegerName(child.lastPathComponent) {
                try importGateIteration(child, info: GateInfo(arch: .aarch64, profile: "testing"), into: &result)
            }
            return result
        }

        var treeResult = ImportPathResult(sourcePath: sourceURL.path)
        let children = try childDirectories(of: sourceURL)
        let preservedFailures = try preservedFailureEntries(in: children, into: &treeResult)
        let preservedFailuresBySerialData = try indexByPrimarySerialData(preservedFailures)
        var consumedPreservedFailures: Set<Int> = []

        for child in children {
            if isPreservedFailureContainer(child) {
                continue
            } else if child.lastPathComponent == "breenix_aarch64_testing_profile" {
                for iteration in try childDirectories(of: child) where isIntegerName(iteration.lastPathComponent) {
                    try importGateIteration(
                        iteration,
                        info: GateInfo(arch: .aarch64, profile: "testing"),
                        preservedFailures: preservedFailures,
                        preservedFailuresBySerialData: preservedFailuresBySerialData,
                        consumedPreservedFailures: &consumedPreservedFailures,
                        into: &treeResult
                    )
                }
            } else if let gateInfo = gateInfo(for: child) {
                try importGateIteration(
                    child,
                    info: gateInfo,
                    preservedFailures: preservedFailures,
                    preservedFailuresBySerialData: preservedFailuresBySerialData,
                    consumedPreservedFailures: &consumedPreservedFailures,
                    into: &treeResult
                )
            }
        }

        for (index, entry) in preservedFailures.enumerated() where !consumedPreservedFailures.contains(index) {
            try importPreservedFailure(entry, into: &treeResult)
        }

        if !treeResult.imported.isEmpty || !treeResult.skipped.isEmpty {
            return treeResult
        }

        return try importLooseSerialDirectory(sourceURL)
    }

    private func importSingleFile(_ url: URL) throws -> ImportPathResult {
        if isFlatPreservedFailureSerial(url), isPreservedFailureContainer(url.deletingLastPathComponent()) {
            var result = ImportPathResult(sourcePath: url.path)
            try importFlatPreservedFailure(url, profile: profileForFailureContainer(url.deletingLastPathComponent()), into: &result)
            return result
        }

        var result = ImportPathResult(sourcePath: url.path)
        try importLooseSerial(url, into: &result)
        return result
    }

    private func importGateIteration(_ directory: URL, info: GateInfo, into result: inout ImportPathResult) throws {
        let serials = try serialSources(in: directory)
        guard !serials.isEmpty else {
            return
        }

        let startedAt = modificationDate(of: serials[0].url) ?? Date()
        try importRun(
            sourceURL: directory,
            serials: serials,
            captures: try captureSources(in: directory),
            startedAt: startedAt,
            arch: info.arch,
            profile: info.profile,
            verdict: .unknown,
            into: &result
        )
    }

    private func importGateIteration(
        _ directory: URL,
        info: GateInfo,
        preservedFailures: [PreservedFailureEntry],
        preservedFailuresBySerialData: [Data: [Int]],
        consumedPreservedFailures: inout Set<Int>,
        into result: inout ImportPathResult
    ) throws {
        let serials = try serialSources(in: directory)
        guard !serials.isEmpty else {
            return
        }

        let firstSerialData = try Data(contentsOf: serials[0].url)
        if let matchingIndexes = preservedFailuresBySerialData[firstSerialData],
           let matchingIndex = matchingIndexes.first(where: { !consumedPreservedFailures.contains($0) }) {
            consumedPreservedFailures.insert(matchingIndex)
            let matchedFailure = preservedFailures[matchingIndex]
            try importPreservedFailure(
                matchedFailure,
                captures: mergedCaptures(base: matchedFailure.captures, additional: try captureSources(in: directory)),
                preloadedFirstSerialData: firstSerialData,
                into: &result
            )
            return
        }

        try importRun(
            sourceURL: directory,
            serials: serials,
            captures: try captureSources(in: directory),
            startedAt: modificationDate(of: serials[0].url) ?? Date(),
            arch: info.arch,
            profile: info.profile,
            verdict: .unknown,
            preloadedFirstSerialData: firstSerialData,
            into: &result
        )
    }

    private func importPreservedFailures(in directory: URL, sourcePath: String) throws -> ImportPathResult {
        var result = ImportPathResult(sourcePath: sourcePath)
        for entry in try preservedFailureEntries(in: directory, into: &result) {
            try importPreservedFailure(entry, into: &result)
        }

        return result
    }

    private func importFlatPreservedFailure(_ serialURL: URL, profile: String, into result: inout ImportPathResult) throws {
        guard let entry = flatPreservedFailureEntry(serialURL, profile: profile, into: &result) else { return }
        try importPreservedFailure(entry, into: &result)
    }

    private func importProdFailureRun(_ directory: URL, into result: inout ImportPathResult) throws {
        guard let entry = prodFailureRunEntry(directory, into: &result) else { return }
        try importPreservedFailure(entry, into: &result)
    }

    private func preservedFailureEntries(in directories: [URL], into result: inout ImportPathResult) throws -> [PreservedFailureEntry] {
        var entries: [PreservedFailureEntry] = []
        for directory in directories where isPreservedFailureContainer(directory) {
            entries.append(contentsOf: try preservedFailureEntries(in: directory, into: &result))
        }
        return entries
    }

    private func preservedFailureEntries(in directory: URL, into result: inout ImportPathResult) throws -> [PreservedFailureEntry] {
        let profile = profileForFailureContainer(directory)
        var entries: [PreservedFailureEntry] = []

        for child in try directoryContents(of: directory) {
            if isProdFailureRunDirectory(child) {
                if let entry = prodFailureRunEntry(child, into: &result) {
                    entries.append(entry)
                }
            } else if isFlatPreservedFailureSerial(child) {
                if let entry = flatPreservedFailureEntry(child, profile: profile, into: &result) {
                    entries.append(entry)
                }
            }
        }

        return entries
    }

    private func flatPreservedFailureEntry(
        _ serialURL: URL,
        profile: String,
        into result: inout ImportPathResult
    ) -> PreservedFailureEntry? {
        guard let startedAt = timestampFromFlatFailureName(serialURL.lastPathComponent) else {
            result.skipped.append(ImportSkip(path: serialURL.path, reason: "timestamp undeterminable"))
            return nil
        }

        let sidecar = serialURL.deletingPathExtension().appendingPathExtension("facts.txt")
        let captures = fileManager.fileExists(atPath: sidecar.path)
            ? [CaptureSource(url: sidecar, destinationName: sidecar.lastPathComponent)]
            : []
        return PreservedFailureEntry(
            sourceURL: serialURL,
            serials: [SerialSource(url: serialURL, destinationName: serialURL.lastPathComponent, stream: .single)],
            captures: captures,
            startedAt: startedAt,
            arch: .aarch64,
            profile: profile,
            verdict: .fail("imported")
        )
    }

    private func prodFailureRunEntry(_ directory: URL, into result: inout ImportPathResult) -> PreservedFailureEntry? {
        guard let startedAt = parseTimestamp(directory.lastPathComponent) else {
            result.skipped.append(ImportSkip(path: directory.path, reason: "timestamp undeterminable"))
            return nil
        }

        let serialURL = directory.appendingPathComponent("serial.txt")
        guard fileManager.fileExists(atPath: serialURL.path) else {
            return nil
        }

        var captures: [CaptureSource] = []
        let facts = directory.appendingPathComponent("gate_boot_facts.txt")
        if fileManager.fileExists(atPath: facts.path) {
            captures.append(CaptureSource(url: facts, destinationName: facts.lastPathComponent))
        }

        return PreservedFailureEntry(
            sourceURL: directory,
            serials: [SerialSource(url: serialURL, destinationName: "serial.txt", stream: .single)],
            captures: captures,
            startedAt: startedAt,
            arch: .aarch64,
            profile: "prod",
            verdict: .fail("imported")
        )
    }

    private func importPreservedFailure(
        _ entry: PreservedFailureEntry,
        captures: [CaptureSource]? = nil,
        preloadedFirstSerialData: Data? = nil,
        into result: inout ImportPathResult
    ) throws {
        try importRun(
            sourceURL: entry.sourceURL,
            serials: entry.serials,
            captures: captures ?? entry.captures,
            startedAt: entry.startedAt,
            arch: entry.arch,
            profile: entry.profile,
            verdict: entry.verdict,
            preloadedFirstSerialData: preloadedFirstSerialData,
            into: &result
        )
    }

    private func indexByPrimarySerialData(_ entries: [PreservedFailureEntry]) throws -> [Data: [Int]] {
        var index: [Data: [Int]] = [:]
        for (entryIndex, entry) in entries.enumerated() {
            guard let primarySerial = entry.serials.first else {
                continue
            }
            index[try Data(contentsOf: primarySerial.url), default: []].append(entryIndex)
        }
        return index
    }

    private func mergedCaptures(base: [CaptureSource], additional: [CaptureSource]) -> [CaptureSource] {
        var destinationNames = Set(base.map(\.destinationName))
        var captures = base
        for capture in additional where !destinationNames.contains(capture.destinationName) {
            destinationNames.insert(capture.destinationName)
            captures.append(capture)
        }
        return captures
    }

    private func importLooseSerialDirectory(_ directory: URL) throws -> ImportPathResult {
        var result = ImportPathResult(sourcePath: directory.path)
        guard let enumerator = fileManager.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles]
        ) else {
            return result
        }

        var serials: [URL] = []
        for case let url as URL in enumerator {
            let values = try url.resourceValues(forKeys: [.isRegularFileKey])
            guard values.isRegularFile == true, url.pathExtension == "txt" else {
                continue
            }
            serials.append(url)
        }

        for serial in serials.sorted(by: urlPathLessThan) {
            try importLooseSerial(serial, into: &result)
        }
        return result
    }

    private func importLooseSerial(_ url: URL, into result: inout ImportPathResult) throws {
        let data: Data
        let index: SerialIndex
        do {
            data = try Data(contentsOf: url)
            index = try scanner.scan(data: data)
        } catch {
            result.skipped.append(ImportSkip(path: url.path, reason: "unreadable"))
            return
        }

        guard let arch = inferLooseArch(fileName: url.lastPathComponent, index: index) else {
            result.skipped.append(ImportSkip(path: url.path, reason: "arch undeterminable"))
            return
        }

        try importRun(
            sourceURL: url,
            serials: [SerialSource(url: url, destinationName: url.lastPathComponent, stream: stream(for: url.lastPathComponent))],
            captures: [],
            startedAt: modificationDate(of: url) ?? Date(),
            arch: arch,
            profile: inferLooseProfile(index: index),
            verdict: .unknown,
            preloadedFirstSerialData: data,
            preloadedFirstSerialIndex: index,
            into: &result
        )
    }

    private func importRun(
        sourceURL: URL,
        serials: [SerialSource],
        captures: [CaptureSource],
        startedAt: Date,
        arch: Arch,
        profile: String,
        verdict: Verdict,
        preloadedFirstSerialData: Data? = nil,
        preloadedFirstSerialIndex: SerialIndex? = nil,
        into result: inout ImportPathResult
    ) throws {
        guard let firstSerial = serials.first else {
            return
        }
        let firstSerialData = try preloadedFirstSerialData ?? Data(contentsOf: firstSerial.url)
        let id = RunManifest.makeImportedID(serialData: firstSerialData, sourcePath: sourceURL.standardizedFileURL.path)
        let runDirectory = try store.createRunDirectory(id: id)

        var serialRefs: [SerialRef] = []
        for serial in serials {
            let destination = runDirectory.appendingPathComponent(serial.destinationName)
            let data = serial.url == firstSerial.url ? firstSerialData : try Data(contentsOf: serial.url)
            try store.writeAtomically(data: data, to: destination)
            serialRefs.append(SerialRef(
                name: serial.destinationName,
                path: serial.destinationName,
                bytes: data.count,
                stream: serial.stream
            ))
        }

        var captureRefs: [CaptureRef] = []
        for capture in captures {
            let data = try Data(contentsOf: capture.url)
            let destination = runDirectory.appendingPathComponent(capture.destinationName)
            try store.writeAtomically(data: data, to: destination)
            captureRefs.append(CaptureRef(name: capture.destinationName, path: capture.destinationName, bytes: data.count))
        }

        let firstIndex = try preloadedFirstSerialIndex ?? scanner.scan(data: firstSerialData)
        let manifest = RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: nil,
            arch: arch,
            profile: profile,
            launcher: .imported,
            kernel: KernelIdentity(buildID: extractBuildID(from: firstIndex)),
            host: nil,
            verdict: verdict,
            verdictSource: .imported,
            serials: serialRefs,
            captures: captureRefs,
            command: [],
            env: [:],
            tags: ["imported"],
            notes: nil
        )

        try store.writeManifest(manifest)
        result.imported.append(ImportedRun(id: id, sourcePath: sourceURL.path))
    }

    private func inferLooseArch(fileName: String, index: SerialIndex) -> Arch? {
        if index.hits.contains(where: { $0.family == .bootBannerAarch64 }) {
            return .aarch64
        }
        if fileName.contains("serial_kernel") || fileName.contains("serial_user") {
            return .x86_64
        }
        if index.hits.contains(where: { $0.family == .kernelLogX86 }) {
            return .x86_64
        }
        return nil
    }

    private func inferLooseProfile(index: SerialIndex) -> String {
        let strictFamilies: Set<MarkerFamily> = [
            .oracleFutexHandoff,
            .oracleFcntlPM,
            .oracleIRQHold,
            .censusStrand,
            .testBootTests
        ]
        return index.hits.contains(where: { strictFamilies.contains($0.family) }) ? "strict" : "unknown"
    }

    private func extractBuildID(from index: SerialIndex) -> String? {
        index.hits.first {
            $0.family == .bootBannerAarch64 && $0.fields["buildID"]?.stringValue != nil
        }?.fields["buildID"]?.stringValue
    }

    private func gateInfo(for directory: URL) -> GateInfo? {
        let name = directory.lastPathComponent
        if hasIntegerSuffix(name, after: "breenix_aarch64_strict_") {
            return GateInfo(arch: .aarch64, profile: "strict")
        }
        if name == "breenix_aarch64_prod_profile" {
            return GateInfo(arch: .aarch64, profile: "prod")
        }
        if hasIntegerSuffix(name, after: "breenix_x86_boot_tests_") {
            return GateInfo(arch: .x86_64, profile: "boot-tests")
        }
        if hasIntegerSuffix(name, after: "breenix_gate_") {
            return GateInfo(arch: .x86_64, profile: "gate")
        }
        if isIntegerName(name), directory.deletingLastPathComponent().lastPathComponent == "breenix_aarch64_testing_profile" {
            return GateInfo(arch: .aarch64, profile: "testing")
        }
        return nil
    }

    private func serialSources(in directory: URL) throws -> [SerialSource] {
        let sources = try directoryContents(of: directory).filter { url in
            url.lastPathComponent.hasPrefix("serial")
                && (url.pathExtension == "txt" || url.pathExtension == "log")
                && isRegularFile(url)
        }
        return sources.sorted(by: urlPathLessThan).map {
            SerialSource(url: $0, destinationName: $0.lastPathComponent, stream: stream(for: $0.lastPathComponent))
        }
    }

    private func captureSources(in directory: URL) throws -> [CaptureSource] {
        try directoryContents(of: directory).filter { url in
            isRegularFile(url)
                && !url.lastPathComponent.hasPrefix("serial")
                && ["txt", "log"].contains(url.pathExtension)
        }.sorted(by: urlPathLessThan).map {
            CaptureSource(url: $0, destinationName: $0.lastPathComponent)
        }
    }

    private func stream(for fileName: String) -> SerialStream {
        if fileName.contains("serial_user") {
            return .com1
        }
        if fileName.contains("serial_kernel") {
            return .com2
        }
        return .single
    }

    private func isPreservedFailureContainer(_ directory: URL) -> Bool {
        directory.lastPathComponent == "breenix_aarch64_strict_failures"
            || directory.lastPathComponent == "breenix_prod_profile_failures"
            || directory.lastPathComponent == "breenix_testing_profile_failures"
    }

    private func profileForFailureContainer(_ directory: URL) -> String {
        switch directory.lastPathComponent {
        case "breenix_prod_profile_failures":
            return "prod"
        case "breenix_testing_profile_failures":
            return "testing"
        default:
            return "strict"
        }
    }

    private func isFlatPreservedFailureSerial(_ url: URL) -> Bool {
        timestampFromFlatFailureName(url.lastPathComponent) != nil
            && !url.lastPathComponent.hasSuffix(".facts.txt")
    }

    private func isProdFailureRunDirectory(_ directory: URL) -> Bool {
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: directory.path, isDirectory: &isDirectory),
              isDirectory.boolValue,
              directory.deletingLastPathComponent().lastPathComponent == "breenix_prod_profile_failures",
              parseTimestamp(directory.lastPathComponent) != nil else {
            return false
        }
        return true
    }

    private func timestampFromFlatFailureName(_ name: String) -> Date? {
        guard name.hasSuffix(".txt"), !name.hasSuffix(".facts.txt") else {
            return nil
        }
        let stem = String(name.dropLast(4))
        let parts = stem.split(separator: "-")
        guard parts.count == 2,
              parts[1].hasPrefix("boot") else {
            return nil
        }
        return parseTimestamp(String(parts[0]))
    }

    private func parseTimestamp(_ text: String) -> Date? {
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyyMMdd'T'HHmmss'Z'"
        return formatter.date(from: text)
    }

    private func hasIntegerSuffix(_ name: String, after prefix: String) -> Bool {
        guard name.hasPrefix(prefix) else {
            return false
        }
        return isIntegerName(String(name.dropFirst(prefix.count)))
    }

    private func isIntegerName(_ name: String) -> Bool {
        !name.isEmpty && name.allSatisfy(\.isNumber)
    }

    private func childDirectories(of directory: URL) throws -> [URL] {
        try directoryContents(of: directory).filter { url in
            var isDirectory: ObjCBool = false
            return fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
        }.sorted(by: urlPathLessThan)
    }

    private func directoryContents(of directory: URL) throws -> [URL] {
        try fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey, .contentModificationDateKey],
            options: [.skipsHiddenFiles]
        )
    }

    private func isRegularFile(_ url: URL) -> Bool {
        (try? url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true
    }

    private func modificationDate(of url: URL) -> Date? {
        try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
    }

    private func urlPathLessThan(_ lhs: URL, _ rhs: URL) -> Bool {
        lhs.path.localizedStandardCompare(rhs.path) == .orderedAscending
    }
}
