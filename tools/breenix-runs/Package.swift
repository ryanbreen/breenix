// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "breenix-runs",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .library(name: "BreenixRuns", targets: ["BreenixRuns"]),
        .executable(name: "breenix-runs", targets: ["breenix-runs"]),
        .executable(name: "BreenixRunInspector", targets: ["BreenixRunInspector"])
    ],
    targets: [
        .target(
            name: "BreenixRuns",
            resources: [.copy("Resources")]
        ),
        .executableTarget(
            name: "breenix-runs",
            dependencies: ["BreenixRuns"]
        ),
        .executableTarget(
            name: "BreenixRunInspector",
            dependencies: ["BreenixRuns"]
        ),
        .testTarget(
            name: "BreenixRunsTests",
            dependencies: ["BreenixRuns"]
        )
    ],
    swiftLanguageModes: [.v6]
)
