import BreenixRuns
import Foundation

struct ImportArguments {
    var paths: [String]
}

func parseImport(_ args: ArraySlice<String>) throws -> ImportArguments {
    let paths = Array(args)
    guard !paths.isEmpty else {
        throw CLIError(description: "import requires at least one path")
    }
    for path in paths {
        guard !path.hasPrefix("--") else {
            throw CLIError(description: "unknown import flag \(path)")
        }
    }
    return ImportArguments(paths: paths)
}

func runImport(_ arguments: ImportArguments, store: RunStore) throws {
    let importer = Importer(store: store)
    for path in arguments.paths {
        let url = URL(fileURLWithPath: path)
        let result = try importer.importPath(url)
        print("Import: \(result.sourcePath)")
        print("Imported: \(result.imported.count)")
        for run in result.imported {
            print("  \(run.id)")
        }
        print("Skipped: \(result.skipped.count)")
        for skip in result.skipped {
            print("  \(skip.path): \(skip.reason)")
        }
    }
}
