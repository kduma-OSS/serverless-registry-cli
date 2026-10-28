// swift-tools-version: 5.8
import PackageDescription

let package = Package(
    name: "DemoKit",
    platforms: [.macOS(.v12)],
    products: [
        .library(name: "DemoKit", targets: ["DemoKit"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-log.git", from: "1.5.0"),
    ],
    targets: [
        .target(name: "DemoKit", dependencies: [.product(name: "Logging", package: "swift-log")]),
    ]
)
