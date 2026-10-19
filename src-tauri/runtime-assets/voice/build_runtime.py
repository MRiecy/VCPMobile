#!/usr/bin/env python3
"""Rebuild the CPU-only Silero runtime. Never substitutes a full ORT runtime."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent
PLUGIN_IOS = ROOT.parents[1] / "plugins/vcp-mobile/ios"
LOCK = json.loads((ROOT / "runtime.lock.json").read_text(encoding="utf-8"))
MODEL = ROOT / "assets/voice/silero_vad.ort"
OPS = ROOT / "required_operators_and_types.config"
COMMON = [
    "--minimal_build", "--disable_ml_ops", "--disable_rtti",
    "--enable_reduced_operator_type_support", "--enable_lto",
    "--no_kleidiai", "--no_sve", "--skip_tests", "--skip_submodule_sync",
]
DEFINES = [
    "onnxruntime_BUILD_UNIT_TESTS=OFF", "onnxruntime_DISABLE_FLOAT8_TYPES=ON",
    "onnxruntime_DISABLE_FLOAT4_TYPES=ON", "onnxruntime_DISABLE_SPARSE_TENSORS=ON",
    "onnxruntime_DISABLE_OPTIONAL_TYPE=ON",
    # build.py omits the ON flag for --no_*; explicit OFF also clears a reused CMake cache.
    "onnxruntime_USE_KLEIDIAI=OFF", "onnxruntime_USE_SVE=OFF",
]


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*args: object, cwd: Path | None = None, env: dict | None = None) -> None:
    subprocess.run([str(x) for x in args], cwd=cwd, env=env, check=True)


def download(url: str, target: Path, expected: str) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True)
    if not target.exists():
        temporary = target.with_suffix(target.suffix + ".download")
        with urllib.request.urlopen(url, timeout=120) as source, temporary.open("wb") as out:
            shutil.copyfileobj(source, out)
        if sha(temporary) != expected:
            raise RuntimeError(f"SHA-256 mismatch: {url}")
        temporary.replace(target)
    if sha(target) != expected:
        raise RuntimeError(f"SHA-256 mismatch in cache: {target}")
    return target


def source(cache: Path) -> Path:
    path = cache / "onnxruntime"
    if not path.exists():
        # Full checkout fetches hundreds of MB of unrelated model fixtures.
        run("git", "clone", "--sparse", "--filter=blob:none", "--depth=1", "--branch",
            "v" + LOCK["ortVersion"], "https://github.com/microsoft/onnxruntime.git", path)
        run("git", "-C", path, "sparse-checkout", "set", "cmake", "include", "objectivec",
            "java", "onnxruntime/core", "onnxruntime/contrib_ops", "onnxruntime/lora",
            "tools/ci_build", "tools/python", "orttraining/orttraining/training_ops")
    commit = subprocess.check_output(["git", "-C", str(path), "rev-parse", "HEAD"], text=True).strip()
    if commit != LOCK["ortCommit"]:
        raise RuntimeError(f"ORT checkout must be {LOCK['ortCommit']}, found {commit}")
    return path


def prepare(cache: Path, regenerate: bool = False) -> Path:
    original = download(
        f"https://raw.githubusercontent.com/snakers4/silero-vad/{LOCK['sileroCommit']}/src/silero_vad/data/silero_vad_16k_op15.onnx",
        cache / "silero_vad_16k_op15.onnx", LOCK["modelSha256"],
    )
    if regenerate or not MODEL.exists() or not OPS.exists():
        import onnxruntime as ort
        if ort.__version__ != LOCK["ortVersion"]:
            raise RuntimeError("Install the pinned requirements-build.txt in a build-only venv")
        converted = cache / "converted"
        run(sys.executable, "-m", "onnxruntime.tools.convert_onnx_models_to_ort", original,
            "--output_dir", converted, "--optimization_style", "Fixed",
            "--enable_type_reduction", "--target_platform", "arm")
        candidate = converted / "silero_vad_16k_op15.ort"
        if "ortModelSha256" in LOCK and sha(candidate) != LOCK["ortModelSha256"]:
            # ORT graph serialization may reorder equivalent data between runs.
            # Keep the checked-in, validated artifact intact; never silently repin it.
            raise RuntimeError(f"Converted bytes differ from the locked artifact. Candidate retained at {candidate}; "
                               "review and verify it before explicitly updating the asset and lock.")
        MODEL.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(candidate, MODEL)
        config = (converted / "silero_vad_16k_op15.required_operators_and_types.config").read_text()
        OPS.write_text("# Silero v6.2.2, ORT 1.24.2, Fixed ARM optimizations; generated with type reduction.\n"
                       + "\n".join(line for line in config.splitlines() if not line.startswith("#")) + "\n",
                       encoding="utf-8", newline="\n")
    for key, path in [("ortModelSha256", MODEL), ("operatorsSha256", OPS)]:
        if key in LOCK and sha(path) != LOCK[key]:
            raise RuntimeError(f"Locked {key} mismatch: {path}")
    return original


def copy_apple_inputs(src: Path) -> None:
    # Tauri links the SwiftPM static product through Rust. App resources are
    # embedded by the G1 Xcode project inputs and loaded from Bundle.main.
    bindings = PLUGIN_IOS / "OrtObjC"
    (bindings / "include").mkdir(parents=True, exist_ok=True)
    # Official inference bindings. Optional provider methods report unavailable
    # because the custom C runtime contains only the CPU provider.
    for item in (src / "objectivec").iterdir():
        if item.suffix in (".h", ".mm") and not any(
            word in item.name for word in ("training", "checkpoint")
        ):
            shutil.copyfile(item, bindings / item.name)
    for item in (src / "objectivec/include").glob("*.h"):
        if not any(word in item.name for word in ("training", "checkpoint")):
            shutil.copyfile(item, bindings / "include" / item.name)
    shutil.copyfile(src / "LICENSE", bindings / "LICENSE")


def build_android_jni(src: Path, build: Path, tool_bin: Path) -> Path:
    """Compile the same-version JNI with only CPU symbols enabled."""
    jni_build = build / "jni"
    headers, classes = jni_build / "headers", jni_build / "classes"
    headers.mkdir(parents=True, exist_ok=True)
    classes.mkdir(parents=True, exist_ok=True)
    java_sources = sorted((src / "java/src/main/java").rglob("*.java"))
    java_sources += sorted((src / "java/src/main/android").rglob("*.java"))
    run("javac", "--release", "8", "-h", headers, "-d", classes, *java_sources)
    native = src / "java/src/main/native"
    c_sources = [path for path in sorted(native.glob("*.c")) if "TrainingSession" not in path.name]
    compiler = tool_bin / ("clang.exe" if platform.system() == "Windows" else "clang")
    output = jni_build / "libonnxruntime4j_jni.so"
    run(compiler, "--target=aarch64-linux-android26", "-std=c11", "-Os", "-flto", "-fPIC",
        "-fvisibility=hidden", "-ffunction-sections", "-fdata-sections", "-shared",
        "-Wl,--no-undefined", "-Wl,--gc-sections", "-Wl,-z,max-page-size=16384",
        "-Wl,-soname,libonnxruntime4j_jni.so", "-I" + str(src / "include"),
        "-I" + str(src / "include/onnxruntime/core/session"),
        "-I" + str(headers), "-I" + str(native), "-I" + str(build / "MinSizeRel"), *c_sources,
        "-L" + str(build / "MinSizeRel"), "-lonnxruntime", "-ldl", "-llog", "-lm", "-o", output)
    return output


def verify_android_elf(tool_bin: Path, library: Path, jni: Path) -> dict:
    readelf = tool_bin / ("llvm-readelf.exe" if platform.system() == "Windows" else "llvm-readelf")
    alignments, dependencies = {}, {}
    for path in (library, jni):
        headers = subprocess.check_output([str(readelf), "--file-header", "--program-headers", "--wide", str(path)], text=True)
        values = [int(line.split()[-1], 16) for line in headers.splitlines() if line.strip().startswith("LOAD ")]
        if "AArch64" not in headers or not values or any(value < 16384 for value in values):
            raise RuntimeError(f"Expected arm64 ELF with 16KB-compatible LOAD alignment: {path}")
        alignments[path.name] = values
        dynamic = subprocess.check_output([str(readelf), "--dynamic", "--wide", str(path)], text=True)
        dependencies[path.name] = re.findall(r"Shared library: \[([^]]+)\]", dynamic)
    core_symbols = subprocess.check_output([str(readelf), "--dyn-syms", "--wide", str(library)], text=True)
    jni_symbols = subprocess.check_output([str(readelf), "--dyn-syms", "--wide", str(jni)], text=True)
    required = sorted(set(re.findall(r"\bUND\s+(Ort\w+)", jni_symbols)))
    for symbol in required:
        if not re.search(r"\b" + symbol + r"(?:@|\s|$)", core_symbols):
            raise RuntimeError(f"JNI references a symbol absent from the trimmed core: {symbol}")
    return {"loadAlignments": alignments, "neededLibraries": dependencies, "jniRequiredOrtSymbols": required}


def android(args: argparse.Namespace, cache: Path, src: Path) -> None:
    sdk = Path(args.android_sdk or os.environ.get("ANDROID_HOME", ""))
    ndk = Path(args.android_ndk or sdk / "ndk" / LOCK["androidNdk"])
    if not (ndk / "source.properties").exists():
        raise RuntimeError("Pass --android-sdk and install the NDK version in runtime.lock.json")
    if LOCK["androidNdk"] not in (ndk / "source.properties").read_text():
        raise RuntimeError("The supplied NDK differs from runtime.lock.json")
    build = args.build_dir.resolve() if args.build_dir else cache / "android-build"
    environment = os.environ.copy()
    if args.cmake_bin:
        environment["PATH"] = str(Path(args.cmake_bin).resolve()) + os.pathsep + environment.get("PATH", "")
    run(sys.executable, src / "tools/ci_build/build.py", "--build_dir", build,
        "--config", "MinSizeRel", "--update", "--build", "--parallel", args.jobs,
        "--android", "--android_abi", "arm64-v8a", "--android_api", LOCK["androidApi"],
        "--android_sdk_path", sdk, "--android_ndk_path", ndk, "--cmake_generator", "Ninja",
        "--build_shared_lib", *COMMON, "--include_ops_by_config", OPS,
        "--cmake_extra_defines", *DEFINES, env=environment)
    library = build / "MinSizeRel/libonnxruntime.so"
    tag = "windows-x86_64" if platform.system() == "Windows" else "darwin-x86_64" if platform.system() == "Darwin" else "linux-x86_64"
    strip = ndk / f"toolchains/llvm/prebuilt/{tag}/bin/llvm-strip"
    run(str(strip) + (".exe" if platform.system() == "Windows" else ""), "--strip-unneeded", library)
    jni = build_android_jni(src, build, strip.parent)
    run(str(strip) + (".exe" if platform.system() == "Windows" else ""), "--strip-unneeded", jni)
    elf_report = verify_android_elf(strip.parent, library, jni)
    official = download(
        f"https://repo.maven.apache.org/maven2/com/microsoft/onnxruntime/onnxruntime-android/{LOCK['ortVersion']}/onnxruntime-android-{LOCK['ortVersion']}.aar",
        cache / "official-bindings.aar", LOCK["officialAarSha256"],
    )
    target = ROOT / f"android/onnxruntime-silero-{LOCK['ortVersion']}.aar"
    target.parent.mkdir(parents=True, exist_ok=True)
    candidate = target.with_suffix(".aar.building")
    # Only Java bytecode is reused. The official JNI links NNAPI and cannot be
    # paired with a CPU-only core, so both shipped native libraries are rebuilt.
    # Keep the last complete runtime usable if packaging is interrupted.
    with zipfile.ZipFile(official) as inp, zipfile.ZipFile(candidate, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as out:
        entries: dict[str, bytes] = {}
        for item in inp.infolist():
            if item.filename in ("AndroidManifest.xml", "classes.jar", "proguard.txt", "R.txt") or item.filename.startswith("META-INF/"):
                entries[item.filename] = inp.read(item)
        entries["AndroidManifest.xml"] = re.sub(
            rb'(android:minSdkVersion=")\d+(")',
            lambda match: match[1] + str(LOCK["androidApi"]).encode() + match[2],
            entries["AndroidManifest.xml"], count=1,
        )
        entries["proguard.txt"] = (ROOT / "android/silero-consumer-rules.pro").read_bytes()
        entries["jni/arm64-v8a/libonnxruntime4j_jni.so"] = jni.read_bytes()
        entries["jni/arm64-v8a/libonnxruntime.so"] = library.read_bytes()
        entries["META-INF/ORT-LICENSE"] = (src / "LICENSE").read_bytes()
        entries["META-INF/ORT-ThirdPartyNotices.txt"] = (src / "ThirdPartyNotices.txt").read_bytes()
        for name, content in sorted(entries.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            out.writestr(info, content, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    candidate.replace(target)
    report = {"ortVersion": LOCK["ortVersion"], "ortCommit": LOCK["ortCommit"],
              "modelBytes": MODEL.stat().st_size, "modelSha256": sha(MODEL), "operatorsSha256": sha(OPS),
              "libraryBytes": library.stat().st_size,
              "librarySha256": sha(library), "aarBytes": target.stat().st_size, "aarSha256": sha(target),
              "jniBytes": jni.stat().st_size, "jniSha256": sha(jni), "elf": elf_report,
              "buildFlags": COMMON + DEFINES, "androidApi": LOCK["androidApi"], "ndk": LOCK["androidNdk"]}
    (ROOT / "android/build-report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


def ios(args: argparse.Namespace, cache: Path, src: Path) -> None:
    if platform.system() != "Darwin":
        raise RuntimeError("iOS XCFramework must be built with Xcode on macOS; full runtime fallback is prohibited")
    build = args.build_dir.resolve() if args.build_dir else cache / "ios-build"
    build.mkdir(parents=True, exist_ok=True)
    settings = build / "settings.json"
    settings.write_text(json.dumps({
        "build_osx_archs": {"iphoneos": ["arm64"]},
        "build_params": {
            "base": ["--parallel", str(args.jobs), "--use_xcode", "--build_apple_framework", *COMMON,
                     "--cmake_extra_defines", *DEFINES],
            "iphoneos": ["--ios", "--apple_deploy_target=" + LOCK["iosDeploymentTarget"]],
        },
    }, indent=2), encoding="utf-8")
    run(sys.executable, src / "tools/ci_build/github/apple/build_apple_framework.py",
        "--build_dir", build, "--config", "MinSizeRel", "--include_ops_by_config", OPS, settings)
    target = PLUGIN_IOS / "Runtime/onnxruntime.xcframework"
    shutil.copytree(build / "framework_out/onnxruntime.xcframework", target, dirs_exist_ok=True)
    # The official Objective-C bridge tests __has_include, not a USE_COREML flag.
    # Upstream get_c_cxx_api_headers exports enabled EP headers only. Assert the
    # actual artifact too, including when a prior local framework was reused.
    if any(target.rglob("coreml_provider_factory.h")):
        raise RuntimeError("CPU-only framework must not expose CoreML headers to the Objective-C bridge")
    framework_library = target / "ios-arm64/onnxruntime.framework/onnxruntime"
    archive = PLUGIN_IOS / "Runtime/libonnxruntime.a"
    # SwiftPM does not merge binaryTarget archives into a static product; Cargo
    # explicitly links this thin archive in plugin build.rs. The upstream lipo
    # assembly can wrap even a single architecture in a universal container.
    archs = subprocess.check_output(["xcrun", "lipo", "-archs", str(framework_library)], text=True).strip()
    if archs != "arm64":
        raise RuntimeError(f"Unexpected Apple runtime architectures: {archs}")
    if framework_library.read_bytes()[:8] == b"!<arch>\n":
        shutil.copyfile(framework_library, archive)
    else:
        run("xcrun", "lipo", framework_library, "-thin", "arm64", "-output", archive)
    if archive.read_bytes()[:8] != b"!<arch>\n":
        raise RuntimeError("Rust must link a thin static archive, not a Mach-O dylib/fat container")
    report = {"ortVersion": LOCK["ortVersion"], "ortCommit": LOCK["ortCommit"],
              "target": "iphoneos-arm64", "deploymentTarget": LOCK["iosDeploymentTarget"],
              "modelSha256": sha(MODEL), "operatorsSha256": sha(OPS),
              "libraryBytes": archive.stat().st_size, "librarySha256": sha(archive),
              "xcframeworkBytes": sum(path.stat().st_size for path in target.rglob("*") if path.is_file()),
              "buildFlags": COMMON + DEFINES, "rustLinkLibraries": ["onnxruntime", "c++", "objc", "Foundation"]}
    (ROOT / "ios").mkdir(parents=True, exist_ok=True)
    (ROOT / "ios/build-report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    copy_apple_inputs(src)
    print(json.dumps(report, indent=2))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=["prepare", "android", "ios", "apple-inputs"])
    parser.add_argument("--cache-dir", type=Path, default=Path(tempfile.gettempdir()) / "vcpmobile-silero-runtime")
    parser.add_argument("--regenerate-model", action="store_true")
    parser.add_argument("--source-dir", type=Path, help="Existing checkout of the exact locked ORT commit")
    parser.add_argument("--build-dir", type=Path, help="Reuse an existing platform build directory")
    parser.add_argument("--android-sdk")
    parser.add_argument("--android-ndk")
    parser.add_argument("--cmake-bin", help="Directory containing cmake, ctest and ninja")
    parser.add_argument("--jobs", type=int, default=min(8, os.cpu_count() or 1))
    args = parser.parse_args()
    cache = args.cache_dir.resolve()
    cache.mkdir(parents=True, exist_ok=True)
    prepare(cache, args.regenerate_model)
    if args.target == "prepare":
        return
    src = args.source_dir.resolve() if args.source_dir else source(cache)
    if subprocess.check_output(["git", "-C", str(src), "rev-parse", "HEAD"], text=True).strip() != LOCK["ortCommit"]:
        raise RuntimeError("ORT source commit differs from runtime.lock.json")
    run("git", "-C", src, "diff", "--quiet", "HEAD", "--")
    if args.target == "android":
        android(args, cache, src)
    elif args.target == "ios":
        ios(args, cache, src)
    else:
        copy_apple_inputs(src)


if __name__ == "__main__":
    main()
