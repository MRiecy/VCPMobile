#!/usr/bin/env python3
"""Run manual iOS stage diagnostics using the G1 device archive build chain."""
import argparse
from collections import deque
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import html
import io
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import subprocess
import sys
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[2]
REPO = ROOT.parent
IOS = ROOT / "ios"
APPLE = ROOT / "gen" / "apple"
INPUTS = json.loads((IOS / "g1-inputs.json").read_text(encoding="utf-8"))


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def evidence_dir():
    return Path(os.environ["RUNNER_TEMP"]) / "vcp-ios-g1"


def prepare():
    evidence = evidence_dir()
    evidence.mkdir(parents=True, exist_ok=True)
    manifest = {
        "purpose": "stage-diagnostic", "status": "prepared",
        "phase": os.environ.get("IOS_DIAGNOSTIC_PHASE", "phase-3"),
        "profile": os.environ.get("IOS_DIAGNOSTIC_PROFILE", "debug"),
        "target": "aarch64-apple-ios", "inputs": INPUTS,
        "developerDir": INPUTS["developerDir"],
        "commit": run(["git", "rev-parse", "HEAD"]),
        "ref": os.environ.get("GITHUB_REF"),
        "runId": os.environ.get("GITHUB_RUN_ID"),
        "runAttempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "imageOS": os.environ.get("ImageOS"), "imageVersion": os.environ.get("ImageVersion"),
        "availableXcodes": sorted(str(path) for path in Path("/Applications").glob("Xcode*.app")),
        "stages": [],
    }
    write_json(evidence / "ios-build-manifest.json", manifest)
    return manifest


@contextmanager
def stage(evidence, manifest, name):
    record = {"name": name, "status": "running", "startedAt": datetime.now(timezone.utc).isoformat()}
    manifest["stages"].append(record)
    manifest["activeStage"] = name
    write_json(evidence / "ios-build-manifest.json", manifest)
    print(f"::group::{name}", flush=True)
    try:
        yield
        record["status"] = "passed"
    except Exception as error:
        record.update({"status": "failed", "error": str(error)})
        manifest["failedStage"] = name
        raise
    finally:
        record["finishedAt"] = datetime.now(timezone.utc).isoformat()
        write_json(evidence / "ios-build-manifest.json", manifest)
        print("::endgroup::", flush=True)


def collect_generated_inputs(evidence):
    """Keep generated project inputs on failure, without copying build products."""
    for directory, children, files in os.walk(APPLE):
        children[:] = [name for name in children if name not in {"build", "target", ".build", "Externals", "Assets.xcassets"}]
        for name in files:
            source = Path(directory) / name
            if source.suffix not in {".yml", ".json", ".pbxproj", ".plist", ".xcprivacy", ".xcconfig", ".entitlements", ".xcscheme"} and name != "Package.resolved":
                continue
            destination = evidence / "generated-project" / source.relative_to(APPLE)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)


def summarize():
    evidence = evidence_dir()
    path = evidence / "ios-build-manifest.json"
    manifest = json.loads(path.read_text(encoding="utf-8")) if path.is_file() else prepare()
    steps = json.loads(os.environ.get("IOS_DIAGNOSTIC_STEPS", "{}"))
    outcomes = {name: value.get("outcome", "unknown") for name, value in steps.items()}
    manifest["workflowSteps"] = outcomes
    # Setup failures and killed build processes must not remain "prepared"/"running".
    if any(outcome in {"failure", "cancelled"} for outcome in outcomes.values()):
        manifest["status"] = "failed"
        manifest.setdefault("failedStage", next(name for name, outcome in outcomes.items() if outcome in {"failure", "cancelled"}))
    elif manifest["status"] not in {"built", "failed"}:
        manifest["status"] = "incomplete"
    for record in manifest["stages"]:
        if record["status"] == "running":
            record["status"] = "interrupted"
            manifest["failedStage"] = record["name"]
    try:
        collect_generated_inputs(evidence)
    except OSError as error:
        manifest.setdefault("diagnosticErrors", []).append(f"Generated project collection: {error}")
    write_json(path, manifest)
    lines = [
        "# iOS stage diagnostics", "",
        f"- Stage: `{manifest['phase']}`; profile: `{manifest['profile']}`; target: `{manifest['target']}`",
        f"- Commit: `{manifest['commit']}`",
        f"- Result: **{manifest['status']}**; failed step: `{manifest.get('failedStage', 'none')}`",
        f"- Xcode: `{manifest['developerDir']}`; SDK: `{manifest.get('sdkVersion', 'not reached')}`", "",
        "This run provides compilation/link/archive evidence only. Stage acceptance, native behavior and G2 user packaging remain separate.", "",
        "| Build stage | Result |", "|---|---|",
        *[f"| {record['name']} | {record['status']} |" for record in manifest["stages"]], "",
        "| Workflow step | Result |", "|---|---|",
        *[f"| {name} | {outcome} |" for name, outcome in outcomes.items()], "",
    ]
    if manifest.get("error"):
        lines += ["<pre>" + html.escape(manifest["error"]) + "</pre>", ""]
    if manifest.get("diagnosticErrors"):
        lines += ["Diagnostic collection warnings:", "<pre>" + html.escape("\n".join(manifest["diagnosticErrors"])) + "</pre>", ""]
    if manifest["status"] != "built":
        logs = sorted(evidence.glob("*.log"), key=lambda item: item.stat().st_mtime)
        if logs:
            with logs[-1].open(encoding="utf-8", errors="replace") as stream:
                tail = "".join(deque(stream, maxlen=60))[-16000:]
            lines += [f"Last log excerpt (`{logs[-1].name}`); full logs are in the artifact:", "<pre>" + html.escape(tail) + "</pre>", ""]
    report = "\n".join(lines)
    (evidence / "summary.md").write_text(report, encoding="utf-8")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as stream:
            stream.write(report)


def icon_entries():
    entries = []
    for idiom, sizes in {
        "iphone": {"20x20": [2, 3], "29x29": [2, 3], "40x40": [2, 3], "60x60": [2, 3]},
        "ipad": {"20x20": [1, 2], "29x29": [1, 2], "40x40": [1, 2], "76x76": [1, 2], "83.5x83.5": [2]},
        "ios-marketing": {"1024x1024": [1]},
    }.items():
        for size, scales in sizes.items():
            for scale in scales:
                filename = "AppIcon-512@2x.png" if idiom == "ios-marketing" else f"AppIcon-{size}@{scale}x.png"
                entries.append({"idiom": idiom, "size": size, "scale": f"{scale}x", "filename": filename})
    return entries


def overlay(project_dir=APPLE):
    """Emit an additive XcodeGen spec; never rewrite the CLI-owned project.yml."""
    project_dir = Path(project_dir).resolve()
    require(project_dir.is_relative_to(APPLE.resolve()), "Overlay destination must stay in gen/apple")
    spec = project_dir / "project.yml"
    require(spec.is_file(), "Run tauri ios init before applying the overlay")
    source = spec.read_text(encoding="utf-8")
    match = re.search(r"(?m)^name:\s*([\w-]+)\s*$", source)
    require(match is not None, "CLI project name format changed")
    name = match[1]
    privacy = project_dir / "VCPResources" / "PrivacyInfo.xcprivacy"
    privacy.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(IOS / "PrivacyInfo.xcprivacy", privacy)
    icon_dir = project_dir / "Assets.xcassets" / "AppIcon.appiconset"
    icon_dir.mkdir(parents=True, exist_ok=True)
    entries = icon_entries()
    for filename in {entry["filename"] for entry in entries}:
        shutil.copyfile(ROOT / "icons" / "ios" / filename, icon_dir / filename)
    write_json(icon_dir / "Contents.json", {"images": entries, "info": {"version": 1, "author": "xcode"}})
    output = project_dir / "project.vcp.json"
    write_json(output, {
        "include": ["project.yml"],
        "targets": {f"{name}_iOS": {
            "sources": [{"path": "VCPResources/PrivacyInfo.xcprivacy", "buildPhase": "resources"}],
            "settings": {"base": {
                "TARGETED_DEVICE_FAMILY": "1,2",
                "ASSETCATALOG_COMPILER_APPICON_NAME": "AppIcon", "CODE_SIGNING_ALLOWED": "NO",
            }, "configs": {"debug": {"DEBUG_INFORMATION_FORMAT": "dwarf-with-dsym", "STRIP_INSTALLED_PRODUCT": "NO"}}},
        }},
    })
    return output


def run(command, *, env=None, log=None, cwd=REPO):
    print("+ " + shlex.join(str(item) for item in command), flush=True)
    if log is None:
        result = subprocess.run(command, cwd=cwd, env=env, text=True, encoding="utf-8", errors="replace", stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        require(result.returncode == 0, f"Command failed ({result.returncode}): {result.stdout.strip()}")
        return result.stdout.strip()
    with log.open("w", encoding="utf-8") as stream:
        stream.write("+ " + shlex.join(str(item) for item in command) + "\n")
        stream.flush()
        with subprocess.Popen(command, cwd=cwd, env=env, text=True, encoding="utf-8", errors="replace", stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
            for line in process.stdout:
                stream.write(line)
                stream.flush()
                print(line, end="", flush=True)
            code = process.wait()
    require(code == 0, f"Command failed ({code}); see {log.name}")
    return ""


def install_xcodegen(destination):
    package = INPUTS["xcodegen"]
    payload = urllib.request.urlopen(package["url"], timeout=120).read()
    require(hashlib.sha256(payload).hexdigest() == package["sha256"], "XcodeGen download SHA-256 mismatch")
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        for entry in archive.infolist():
            target = (destination / entry.filename).resolve()
            require(target.is_relative_to(destination.resolve()), "XcodeGen archive path escapes destination")
            require((entry.external_attr >> 16) & 0o170000 != 0o120000, "Unexpected XcodeGen archive symlink")
        archive.extractall(destination)
    binary = destination / "xcodegen" / "bin" / "xcodegen"
    binary.chmod(0o755)
    return binary


def collect_archives(evidence, manifest, env):
    """Preserve actual archives and native diagnostics without a product allowlist."""
    archives = sorted(APPLE.rglob("*.xcarchive"))
    require(archives, "Build completed without a device archive to collect")
    manifest["archives"] = []
    for index, archive in enumerate(archives, 1):
        archive_zip = evidence / f"{manifest['phase']}-{manifest['profile']}-{index}-{archive.name}.zip"
        run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(archive), str(archive_zip)], env=env)
        manifest["archives"].append({"file": archive_zip.name, "sha256": digest(archive_zip)})
        for app in sorted((archive / "Products" / "Applications").glob("*.app")):
            details = evidence / "bundles" / str(index) / app.name
            details.mkdir(parents=True, exist_ok=True)
            try:
                (details / "files.txt").write_text("\n".join(sorted(str(p.relative_to(app)) for p in app.rglob("*"))) + "\n", encoding="utf-8")
                shutil.copyfile(app / "Info.plist", details / "Info.plist")
                privacy = app / "PrivacyInfo.xcprivacy"
                if privacy.is_file():
                    shutil.copyfile(privacy, details / privacy.name)
                info = plistlib.loads((app / "Info.plist").read_bytes())
                binary = app / info["CFBundleExecutable"]
                for name, command in [
                    ("architectures", ["lipo", "-archs"]),
                    ("symbols", ["nm", "-U", "-j"]),
                    ("load-commands", ["otool", "-l"]),
                ]:
                    try:
                        output = details / f"{name}.txt"
                        with output.open("w", encoding="utf-8") as stream:
                            result = subprocess.run(["xcrun", *command, str(binary)], env=env, text=True, stdout=stream, stderr=subprocess.STDOUT)
                        if result.returncode:
                            manifest.setdefault("diagnosticErrors", []).append(f"{app.name}/{name}: exit {result.returncode}; see {output.relative_to(evidence)}")
                    except (OSError, subprocess.SubprocessError) as error:
                        manifest.setdefault("diagnosticErrors", []).append(f"{app.name}/{name}: {error}")
            except (OSError, KeyError, ValueError) as error:
                manifest.setdefault("diagnosticErrors", []).append(f"{app.name}: {error}")


def build():
    evidence = evidence_dir()
    path = evidence / "ios-build-manifest.json"
    manifest = json.loads(path.read_text(encoding="utf-8")) if path.is_file() else prepare()
    manifest["status"] = "running"
    try:
        with stage(evidence, manifest, "toolchain"):
            require(sys.platform == "darwin" and os.environ.get("GITHUB_ACTIONS") == "true", "Apple builds are restricted to manual GitHub Actions")
            require(manifest["profile"] in {"debug", "release"}, "Unknown archive profile")
            require(manifest["phase"] in {f"phase-{index}" for index in range(7)}, "Unknown diagnostic phase")
            env = os.environ.copy()
            env["DEVELOPER_DIR"] = manifest["developerDir"]
            # Tauri filters DEVELOPER_DIR from xcodebuild's environment; bind the
            # disposable runner's system selection before probing or building.
            run(["sudo", "-n", "xcode-select", "--switch", manifest["developerDir"]], env=env, log=evidence / "xcode-select.log")
            manifest["xcode"] = run(["xcodebuild", "-version"], env=env)
            manifest["sdkVersion"] = run(["xcrun", "--sdk", "iphoneos", "--show-sdk-version"], env=env)
            manifest["macOS"] = run(["sw_vers"], env=env)
            for tool in ["node", "pnpm"]:
                manifest[tool] = run([tool, "--version"], env=env)
            manifest["rust"] = run(["rustc", "--version"], env=env)
            manifest["tauriCli"] = run(["pnpm", "exec", "tauri", "--version"], env=env)
            ios_config = json.loads((ROOT / "tauri.ios.conf.json").read_text(encoding="utf-8"))
            env.update({"IPHONEOS_DEPLOYMENT_TARGET": ios_config["bundle"]["iOS"]["minimumSystemVersion"], "SDKROOT": run(["xcrun", "--sdk", "iphoneos", "--show-sdk-path"], env=env), "CARGO_INCREMENTAL": "0", "CARGO_BUILD_JOBS": "2"})
            if manifest["profile"] == "debug":
                env.update({"CARGO_PROFILE_DEV_DEBUG": "1", "CARGO_PROFILE_DEV_STRIP": "false"})
            # These overrides select different native-library or signing paths; keep them absent.
            for key in ["LIBSQLITE3_SYS_USE_PKG_CONFIG", "ZSTD_SYS_USE_PKG_CONFIG", "RING_PREGENERATE_ASM", "APPLE_DEVELOPMENT_TEAM", "IOS_CERTIFICATE", "IOS_CERTIFICATE_PASSWORD", "IOS_MOBILE_PROVISION", "APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_PROVISIONING_PROFILE", "APPLE_API_KEY", "APPLE_API_KEY_PATH", "APPLE_API_ISSUER"]:
                env.pop(key, None)
            env["CC_aarch64_apple_ios"] = run(["xcrun", "--sdk", "iphoneos", "--find", "clang"], env=env)
            env["CXX_aarch64_apple_ios"] = run(["xcrun", "--sdk", "iphoneos", "--find", "clang++"], env=env)
            env["AR_aarch64_apple_ios"] = run(["xcrun", "--find", "ar"], env=env)
            manifest["nativeEnvironment"] = {key: env.get(key) for key in ["SDKROOT", "IPHONEOS_DEPLOYMENT_TARGET", "CC_aarch64_apple_ios", "CXX_aarch64_apple_ios", "AR_aarch64_apple_ios", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_DEV_STRIP", "LIBSQLITE3_SYS_USE_PKG_CONFIG", "ZSTD_SYS_USE_PKG_CONFIG", "RING_PREGENERATE_ASM"]}
            xcodegen = install_xcodegen(evidence / "tools")
            env["PATH"] = str(xcodegen.parent) + os.pathsep + env["PATH"]
            manifest["xcodegen"] = run([str(xcodegen), "--version"], env=env)
            manifest["swiftVersion"] = run(["xcrun", "swift", "--version"], env=env)
        with stage(evidence, manifest, "rust-target"):
            run(["rustup", "target", "add", "aarch64-apple-ios"], env=env, log=evidence / "rust-target.log")
        with stage(evidence, manifest, "project-init"):
            require(not APPLE.exists(), "Diagnostics require a fresh checkout without gen/apple; no automatic deletion is performed")
            run(["pnpm", "tauri", "ios", "init", "--ci", "--skip-targets-install"], env=env, log=evidence / "init.log")
        with stage(evidence, manifest, "project-overlay"):
            spec = overlay()
            run([str(xcodegen), "generate", "--spec", str(spec)], env=env, log=evidence / "overlay.log")
        with stage(evidence, manifest, "device-build"):
            # Tauri owns the one frontend build via beforeBuildCommand.
            profile_args = ["--debug"] if manifest["profile"] == "debug" else []
            run(["pnpm", "tauri", "ios", "build", *profile_args, "--target", "aarch64", "--archive-only", "--no-sign", "--ci", "--", "--locked"], env=env, log=evidence / "build.log")
        with stage(evidence, manifest, "archive-collection"):
            collect_archives(evidence, manifest, env)
        manifest["status"] = "built"
    except Exception as error:
        manifest.update({"status": "failed", "error": str(error)})
        raise
    finally:
        write_json(evidence / "ios-build-manifest.json", manifest)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "overlay", "build", "summary"])
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            prepare()
        elif args.command == "overlay":
            print(overlay())
        elif args.command == "build":
            build()
        elif args.command == "summary":
            summarize()
    except (OSError, KeyError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"iOS diagnostics: {error}", file=sys.stderr)
        sys.exit(1)
