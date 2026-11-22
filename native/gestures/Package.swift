// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "TapGestures",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "TapGestures", type: .dynamic, targets: ["TapGestures"])
    ],
    targets: [
        .target(
            name: "TapGestures",
            path: "Sources/TapGestures",
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("QuartzCore"),
            ]
        ),
        .testTarget(
            name: "TapGesturesTests",
            dependencies: ["TapGestures"],
            path: "Tests/TapGesturesTests"
        )
    ]
)
