// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "DemoKit",
    platforms: [.macOS(.v12)],
    products: [
        .library(name: "DemoKit", targets: ["DemoKit"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-log.git", from: "1.5.0"),
        .package(id: "acme.networking", from: "2.0.0"),
    ],
    targets: [
        .target(name: "DemoKit", dependencies: [.product(name: "Logging", package: "swift-log")]),
        .testTarget(name: "DemoKitTests", dependencies: ["DemoKit"]),
    ]
)
