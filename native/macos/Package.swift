// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "AgaveStudio",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "AgaveStudio", targets: ["AgaveStudio"])
    ],
    targets: [
        .executableTarget(
            name: "AgaveStudio",
            path: "Sources/AgaveStudio"
        )
    ]
)
