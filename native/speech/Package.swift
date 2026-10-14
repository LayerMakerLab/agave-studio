// swift-tools-version:5.10
// agave-speech: on-device speech to text with NVIDIA Parakeet TDT 0.6B v3 (CoreML) through FluidAudio.
import PackageDescription

let package = Package(
    name: "agave-speech",
    platforms: [.macOS(.v14)],
    dependencies: [
        // Apache-2.0. Pinned: the model files Agave downloads are pinned to what this version loads.
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.17.7"),
    ],
    targets: [
        .executableTarget(
            name: "agave-speech",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio")],
            path: "Sources"
        ),
    ]
)
