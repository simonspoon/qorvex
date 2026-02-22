// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "qorvex-streamer",
    platforms: [
        .macOS(.v13) // ScreenCaptureKit requires macOS 12.3+; .v13 is nearest available SPM enum
    ],
    targets: [
        .executableTarget(
            name: "qorvex-streamer",
            path: "Sources"
        )
    ]
)
