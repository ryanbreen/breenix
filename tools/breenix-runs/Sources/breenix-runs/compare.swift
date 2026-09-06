import BreenixRuns
import Foundation

struct CompareArguments {
    var lhsSelector: String
    var rhsSelector: String
}

func parseCompare(_ args: ArraySlice<String>) throws -> CompareArguments {
    let selectors = Array(args)
    guard selectors.count == 2 else {
        throw CLIError(description: "compare requires <run-id-a> <run-id-b>")
    }
    for selector in selectors {
        guard !selector.hasPrefix("--") else {
            throw CLIError(description: "unknown compare flag \(selector)")
        }
    }
    return CompareArguments(lhsSelector: selectors[0], rhsSelector: selectors[1])
}

func runCompare(_ arguments: CompareArguments, store: RunStore) throws {
    let lhs = try loadManifest(selector: arguments.lhsSelector, store: store)
    let rhs = try loadManifest(selector: arguments.rhsSelector, store: store)
    let result = try RunDiff.compare(lhs: lhs, rhs: rhs, store: store)
    print(RunDiff.render(result))
}
