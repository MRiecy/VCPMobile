"""Exercise file ownership and real diagnostic failure handling without Apple tools."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


g1 = load("g1", "g1.py")


class EngineeringFileTests(unittest.TestCase):
    def test_overlay_replays_without_changing_cli_spec_or_duplicating_resources(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            project = Path(temporary) / "apple"
            project.mkdir()
            source = "name: vcp-mobile\ntargets:\n  vcp-mobile_iOS:\n    platform: iOS\n"
            (project / "project.yml").write_text(source, encoding="utf-8")
            with patch.object(g1, "APPLE", project):
                output = g1.overlay(project)
                first = output.read_bytes()
                g1.overlay(project)
                self.assertEqual(first, output.read_bytes())
            self.assertEqual((project / "project.yml").read_text(encoding="utf-8"), source)
            overlay = json.loads(first)
            self.assertEqual(overlay["include"], ["project.yml"])
            self.assertEqual(len(overlay["targets"]["vcp-mobile_iOS"]["sources"]), 2)
            self.assertEqual(g1.digest(project / "VCPResources" / "PrivacyInfo.xcprivacy"), g1.digest(g1.IOS / "PrivacyInfo.xcprivacy"))
            voice = g1.verify_voice_resources(project / "VCPResources")
            self.assertIn("voice/silero_vad.ort", {entry["file"] for entry in voice})
            self.assertIn({"path": "VCPResources/voice", "type": "folder", "buildPhase": "resources"}, overlay["targets"]["vcp-mobile_iOS"]["sources"])

    def test_archive_voice_resources_reject_missing_and_changed_model(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            app = Path(temporary) / "App.app"
            (app / "voice").mkdir(parents=True)
            with self.assertRaisesRegex(RuntimeError, "not embedded"):
                g1.verify_voice_resources(app)
            for source in g1.voice_assets():
                (app / "voice" / source.name).write_bytes(source.read_bytes())
            g1.verify_voice_resources(app)
            with (app / "voice" / "silero_vad.ort").open("r+b") as model:
                model.write(b"bad")
            with self.assertRaisesRegex(RuntimeError, "differs from the build input"):
                g1.verify_voice_resources(app)

    def test_overlay_copies_share_sources_into_the_extension_only(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            project = Path(temporary) / "apple"
            project.mkdir()
            (project / "project.yml").write_text("name: vcp-mobile\n", encoding="utf-8")
            with patch.object(g1, "APPLE", project):
                overlay = json.loads(g1.overlay(project).read_text(encoding="utf-8"))
            extension = overlay["targets"]["VCPShare"]
            source_files = {source.name: source for source in (g1.IOS / "share").rglob("*.swift")}
            compiled_paths = [entry["path"] for entry in extension["sources"] if entry["path"].endswith(".swift")]
            self.assertEqual(set(compiled_paths), {f"VCPShare/{name}" for name in source_files})
            self.assertEqual(len(compiled_paths), len(source_files))
            for source_path in compiled_paths:
                generated = project / source_path
                self.assertTrue(generated.is_file())
                self.assertEqual(g1.digest(generated), g1.digest(source_files[generated.name]))
            main = overlay["targets"]["vcp-mobile_iOS"]
            self.assertTrue(set(compiled_paths).isdisjoint(entry["path"] for entry in main["sources"]))
            self.assertEqual(main["dependencies"], [{"target": "VCPShare", "embed": True}])
            self.assertEqual(extension["type"], "app-extension")
            self.assertEqual(extension.get("dependencies", []), [])
            self.assertEqual(extension["settings"]["base"]["APPLICATION_EXTENSION_API_ONLY"], "YES")

    def test_overlay_rejects_outside_generated_tree(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            with patch.object(g1, "APPLE", Path(temporary) / "apple"):
                with self.assertRaisesRegex(RuntimeError, "must stay in gen/apple"):
                    g1.overlay(temporary)


class DiagnosticEvidenceTests(unittest.TestCase):
    def test_failed_subprocess_keeps_exit_status_log_and_failed_stage(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            evidence = Path(temporary)
            manifest = {"stages": []}
            with self.assertRaisesRegex(RuntimeError, "Command failed \\(7\\)"):
                with g1.stage(evidence, manifest, "device-build"):
                    g1.run([sys.executable, "-c", "import sys; print('native compile failed'); sys.exit(7)"], log=evidence / "build.log")
            saved = json.loads((evidence / "ios-build-manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(saved["failedStage"], "device-build")
            self.assertEqual(saved["stages"][0]["status"], "failed")
            self.assertIn("native compile failed", (evidence / "build.log").read_text(encoding="utf-8"))

    def test_dependency_failure_has_summary_before_apple_build_starts(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            root = Path(temporary).resolve()
            with patch.dict(os.environ, {
                "RUNNER_TEMP": str(root), "GITHUB_STEP_SUMMARY": str(root / "job.md"),
                "IOS_DIAGNOSTIC_STEPS": json.dumps({"dependencies": {"outcome": "failure"}, "build": {"outcome": "skipped"}}),
            }), patch.object(g1, "APPLE", root / "apple"):
                g1.prepare()
                (g1.evidence_dir() / "dependencies.log").write_text("registry unavailable\n", encoding="utf-8")
                g1.summarize()
                manifest = json.loads((g1.evidence_dir() / "ios-build-manifest.json").read_text(encoding="utf-8"))
                self.assertEqual(manifest["status"], "failed")
                self.assertEqual(manifest["failedStage"], "dependencies")
                self.assertEqual(manifest["stages"], [])
                self.assertIn("registry unavailable", (root / "job.md").read_text(encoding="utf-8"))

    def test_killed_stage_leaves_checkpoint_and_can_be_summarized(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            root = Path(temporary).resolve()
            environment = {
                "RUNNER_TEMP": str(root), "GITHUB_STEP_SUMMARY": str(root / "job.md"),
                "IOS_DIAGNOSTIC_STEPS": json.dumps({"build": {"outcome": "failure"}}),
            }
            with patch.dict(os.environ, environment), patch.object(g1, "APPLE", root / "apple"):
                g1.prepare()
                program = "\n".join([
                    "import json, os, sys", "sys.path.insert(0, sys.argv[1])", "import g1",
                    "evidence = g1.evidence_dir()",
                    "manifest = json.loads((evidence / 'ios-build-manifest.json').read_text())",
                    "with g1.stage(evidence, manifest, 'device-build'):", "    os._exit(9)",
                ])
                result = subprocess.run([sys.executable, "-B", "-c", program, str(Path(__file__).resolve().parent)], check=False)
                self.assertEqual(result.returncode, 9)
                g1.summarize()
                manifest = json.loads((g1.evidence_dir() / "ios-build-manifest.json").read_text(encoding="utf-8"))
                self.assertEqual(manifest["status"], "failed")
                self.assertEqual(manifest["failedStage"], "device-build")
                self.assertEqual(manifest["stages"][0]["status"], "interrupted")

    def test_failure_evidence_collects_project_inputs_without_build_products(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            root = Path(temporary)
            project = root / "apple"
            (project / "build").mkdir(parents=True)
            (project / "App.xcodeproj").mkdir()
            (project / "project.yml").write_text("name: App", encoding="utf-8")
            (project / "App.xcodeproj" / "project.pbxproj").write_text("generated input", encoding="utf-8")
            (project / "build" / "large-product.json").write_text("build product", encoding="utf-8")
            with patch.object(g1, "APPLE", project):
                g1.collect_generated_inputs(root / "evidence")
            saved = root / "evidence" / "generated-project"
            self.assertEqual((saved / "App.xcodeproj" / "project.pbxproj").read_text(encoding="utf-8"), "generated input")
            self.assertFalse((saved / "build").exists())


if __name__ == "__main__":
    unittest.main()
