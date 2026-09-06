import Foundation

public struct RunDetailViewModel: Equatable, Sendable {
    public var manifest: RunManifest
    public var sidebarRow: SidebarRowViewModel
    public var subsystems: SubsystemsViewModel
    public var messages: [MessageLineViewModel]

    public init(manifest: RunManifest, serialIndex: SerialIndex, catalog: [BootStage]) {
        let states = StateMachine.evaluate(catalog: catalog, index: serialIndex)
        self.manifest = manifest
        self.sidebarRow = SidebarViewModel.row(for: manifest)
        self.subsystems = SubsystemsViewModel(manifest: manifest, states: states)
        self.messages = MessageFilter.rows(for: serialIndex)
    }

    public static func load(manifest: RunManifest, store: RunStore) throws -> RunDetailViewModel {
        let serialIndex = try scanSerials(manifest: manifest, store: store)
        let catalog = try StageCatalog.load(for: manifest.arch)
        return RunDetailViewModel(manifest: manifest, serialIndex: serialIndex, catalog: catalog)
    }

    public static func scanSerials(manifest: RunManifest, store: RunStore) throws -> SerialIndex {
        guard !manifest.serials.isEmpty else {
            return try MarkerScanner().scan(data: Data())
        }

        var data = Data()
        for serial in manifest.serials {
            let url = serialURL(serial, manifest: manifest, store: store)
            guard FileManager.default.fileExists(atPath: url.path) else {
                throw RunShowError.serialMissing(url)
            }
            if !data.isEmpty, data.last != 0x0A {
                data.append(0x0A)
            }
            data.append(try Data(contentsOf: url))
        }
        return try MarkerScanner().scan(data: data)
    }

    private static func serialURL(_ serial: SerialRef, manifest: RunManifest, store: RunStore) -> URL {
        if serial.path.hasPrefix("/") {
            return URL(fileURLWithPath: serial.path)
        }
        return store.runDirectory(id: manifest.id).appendingPathComponent(serial.path)
    }
}
