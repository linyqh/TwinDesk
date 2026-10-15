// swift-tools-version: 6.0
//
// The macOS app is built with SwiftPM so it is reproducible from the command
// line; scripts/bundle-app.sh wraps the executable into a signed .app with
// the Info.plist and entitlements from Support/. Open this folder in Xcode
// for editing and debugging.
import PackageDescription

let package = Package(
    name: "TwinDesk",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "TwinDesk", targets: ["OpenShareMouse"]),
    ],
    targets: [
        // Static Rust core produced by scripts/build-rust.sh.
        .binaryTarget(
            name: "OSMCoreFFI",
            path: "Frameworks/OSMCoreFFI.xcframework"
        ),
        // Generated UniFFI bindings over the Rust core.
        .target(
            name: "OSMCore",
            dependencies: ["OSMCoreFFI"],
            path: "Sources/OSMCore",
            swiftSettings: [
                // Generated code predates Swift 6 strict concurrency.
                .swiftLanguageMode(.v5),
            ],
            linkerSettings: [
                .linkedFramework("CoreGraphics"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("Carbon"),
            ]
        ),
        .executableTarget(
            name: "OpenShareMouse",
            dependencies: ["OSMCore"],
            path: "Sources/OpenShareMouse",
            resources: [.copy("Resources/TwinDesk.png")]
        ),
        .testTarget(
            name: "OpenShareMouseTests",
            dependencies: ["OSMCore", "OpenShareMouse"],
            path: "Tests/OpenShareMouseTests"
        ),
    ]
)
