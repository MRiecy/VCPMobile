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
        // Cargo.lock does not constrain the Swift package resolver. Keep the
        // published Swift sources aligned with Tauri's swift-rs 1.0.9 dependency.
        .package(
            name: "SwiftRs",
            url: "https://github.com/Brendonovich/swift-rs",
            .exact("1.0.9")
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
