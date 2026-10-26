// swift-tools-version: 6.0
// AzooKeyKanaKanjiConverter 7d5dd99fd7f4d1251ff94ae6c75642e4b36f327b, vendored.
//
// The sources under Sources/ are upstream's, unmodified. Only this manifest
// differs from upstream:
//
// - EfficientNGram is built with SwiftyMarisa on Windows too. Upstream
//   excludes it there and compiles a mock instead (an initializer that loads
//   nothing and a uniform distribution), so a personalization model never
//   changes the Zenzai ranking. The only reason SwiftyMarisa did not build on
//   Windows was a `typedef unsigned long size_t;` in cmarisawrapper.h, which
//   redefines size_t on LLP64; see azooKey/AzooKeyKanaKanjiConverter#355.
// - SwiftyMarisa is pinned to the revision with that typedef removed and with
//   explicit lengths for [Int8] keys and queries. Before the latter, strlen
//   ran past the end of the key arrays, which the n-gram keys always hit, so
//   probabilities could differ from run to run on every platform.
// - EfficientNGram is a product, so tests can train and load a model.
// - Targets this repository does not build are left out: the default
//   dictionary module (the server ships the dictionaries itself), CliTool,
//   upstream's tests and the LLAMA_MOCK variant.
//
// The directory must stay named AzooKeyKanaKanjiConverter: SwiftPM derives a
// path dependency's identity from it, and the server refers to the products
// through that identity.

import PackageDescription

var swiftSettings: [SwiftSetting] = [
    .enableUpcomingFeature("ExistentialAny"),
]

var dependencies: [Package.Dependency] = [
    .package(url: "https://github.com/apple/swift-algorithms", from: "1.0.0"),
    .package(url: "https://github.com/apple/swift-collections", from: "1.0.0"),
    .package(url: "https://github.com/ensan-hcl/swift-tokenizers", branch: "feat/minimum"),
]

var efficientNGramDependencies: [Target.Dependency] = [.product(name: "Transformers", package: "swift-tokenizers")]
#if !os(Linux) || !canImport(Android)
// SwiftyMarisa is not available on Android, where EfficientNGram falls back to
// upstream's mock.
dependencies.append(.package(url: "https://github.com/ensan-hcl/SwiftyMarisa", revision: "84ff8af5c189a5c72d8cb41f3e488ea08b43bcaa"))
efficientNGramDependencies.append("SwiftyMarisa")
swiftSettings.append(.interoperabilityMode(.Cxx))
#endif

var targets: [Target] = [
    .target(
        name: "SwiftUtils",
        dependencies: [
            .product(name: "Algorithms", package: "swift-algorithms")
        ],
        resources: [],
        swiftSettings: swiftSettings
    ),
    .target(
        name: "EfficientNGram",
        dependencies: efficientNGramDependencies,
        resources: [.copy("tokenizer")],
        swiftSettings: swiftSettings
    ),
]

#if os(Windows) || os(Linux)
targets.append(contentsOf: [
    .systemLibrary(
        name: "llama.cpp"
    ),
    .target(
        name: "KanaKanjiConverterModule",
        dependencies: [
            "SwiftUtils",
            "llama.cpp",
            "EfficientNGram",
            .product(name: "Collections", package: "swift-collections"),
        ],
        swiftSettings: swiftSettings
    ),
])
#else
targets.append(contentsOf: [
    .binaryTarget(
        name: "llama.cpp",
        url: "https://github.com/fkunn1326/llama.cpp/releases/download/b4844/llama-b4844-xcframework.zip",
        // this can be computed `swift package compute-checksum llama-b4844-xcframework.zip`
        checksum: "40bd1e58e727511649e13a6de9eb577ea8be78fe4183c2e1b382b12054849f05"
    ),
    .target(
        name: "KanaKanjiConverterModule",
        dependencies: [
            "SwiftUtils",
            "EfficientNGram",
            "llama.cpp",
            .product(name: "Collections", package: "swift-collections"),
        ],
        swiftSettings: swiftSettings
    ),
])
#endif

let package = Package(
    name: "AzooKeyKanakanjiConverter",
    platforms: [.iOS(.v16), .macOS(.v13)],
    products: [
        .library(
            name: "SwiftUtils",
            targets: ["SwiftUtils"]
        ),
        .library(
            name: "KanaKanjiConverterModule",
            targets: ["KanaKanjiConverterModule"]
        ),
        .library(
            name: "EfficientNGram",
            targets: ["EfficientNGram"]
        ),
    ],
    dependencies: dependencies,
    targets: targets
)
