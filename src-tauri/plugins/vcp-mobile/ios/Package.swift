// swift-tools-version:5.5
import PackageDescription

let package = Package(
    name: "tauri-plugin-vcp-mobile",
    platforms: [.iOS("26.0")],
    products: [
        .library(
            name: "tauri-plugin-vcp-mobile",
            type: .static,
            targets: ["tauri-plugin-vcp-mobile"]
        )
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
        // Cargo.lock does not constrain the Swift package resolver. Rust swift-rs
        // 1.0.8 keeps the Swift sources from the published SwiftPM 1.0.7 tag.
        .package(
            name: "SwiftRs",
            url: "https://github.com/Brendonovich/swift-rs",
            .exact("1.0.7")
        )
    ],
    targets: [
        .target(
            name: "tauri-plugin-vcp-mobile",
            dependencies: [.byName(name: "Tauri"), .byName(name: "SwiftRs"), "onnxruntime_objc"],
            path: "Sources"
        ),
        // Produced by runtime-assets/voice/build_runtime.py ios before SwiftPM resolution.
        // Deliberately local: a missing custom runtime must not pull the full ORT package.
        .binaryTarget(name: "onnxruntime", path: "Runtime/onnxruntime.xcframework"),
        .target(
            name: "onnxruntime_objc",
            dependencies: ["onnxruntime"],
            path: "OrtObjC",
            exclude: ["LICENSE"],
            publicHeadersPath: "include",
            cxxSettings: [.define("SPM_BUILD")]
        )
    ],
    cxxLanguageStandard: .cxx17
)
