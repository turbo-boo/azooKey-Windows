// swift-tools-version: 6.0
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

// The converter's modules are built with C++ interop (SwiftyMarisa wraps
// marisa-trie), and Swift refuses to import such a module into a compilation
// that does not enable it as well.
let swiftSettings: [SwiftSetting] = [.interoperabilityMode(.Cxx)]

let package = Package(
    name: "azookey-server",
    products: [
        // Products define the executables and libraries a package produces, making them visible to other packages.
        .library(
            name: "azookey-server",
            type: .dynamic,
            targets: ["azookey-server"]
        ),
        .library(name: "ffi", targets: ["azookey-server"])
    ],
    dependencies: [
        // Vendored so EfficientNGram (personalization) is real on Windows;
        // see AzooKeyKanaKanjiConverter/Package.swift.
        .package(path: "AzooKeyKanaKanjiConverter")
    ],
    targets: [
        // Targets are the basic building blocks of a package, defining a module or a test suite.
        // Targets can depend on other targets in this package and products from dependencies.
        .target(name: "ffi"),
        .target(
            name: "azookey-server",
            dependencies: [
                .product(name: "KanaKanjiConverterModule", package: "azookeykanakanjiconverter"),
                "ffi"
            ],
            swiftSettings: swiftSettings
        ),
        .testTarget(
            name: "azookey-serverTests",
            dependencies: [
                "azookey-server",
                .product(name: "KanaKanjiConverterModule", package: "azookeykanakanjiconverter")
            ],
            swiftSettings: swiftSettings
        ),
        .testTarget(
            name: "PersonalizationModelTests",
            dependencies: [
                .product(name: "EfficientNGram", package: "azookeykanakanjiconverter")
            ],
            swiftSettings: swiftSettings
        ),
    ]
)
