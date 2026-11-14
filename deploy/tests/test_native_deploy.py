import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEPLOY_DIR = ROOT / "deploy"


class NativeDeployTests(unittest.TestCase):
    def test_shell_scripts_have_valid_bash_syntax(self) -> None:
        scripts = [
            DEPLOY_DIR / "install-linux.sh",
            DEPLOY_DIR / "build-deb.sh",
            DEPLOY_DIR / "build-debian-os.sh",
            DEPLOY_DIR / "mcm-appliance.sh",
        ]
        for script in scripts:
            self.assertTrue(script.is_file(), f"Missing script: {script}")
            result = subprocess.run(
                ["bash", "-n", str(script)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(
                result.returncode,
                0,
                f"bash -n failed for {script.name}: {result.stderr}",
            )

    def test_install_linux_help_succeeds(self) -> None:
        result = subprocess.run(
            ["bash", str(DEPLOY_DIR / "install-linux.sh"), "--help"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--port", result.stdout)
        self.assertIn("--data-dir", result.stdout)
        self.assertIn("--uninstall", result.stdout)

    def test_systemd_unit_is_native_and_well_formed(self) -> None:
        unit_text = (DEPLOY_DIR / "mcm.service").read_text(encoding="utf-8")
        self.assertIn("[Unit]", unit_text)
        self.assertIn("[Service]", unit_text)
        self.assertIn("[Install]", unit_text)
        self.assertIn("ExecStart=/usr/local/bin/mcm", unit_text)
        self.assertNotIn("EOF", unit_text)
        self.assertNotIn("docker", unit_text.lower())
        self.assertNotIn("podman", unit_text.lower())

    def test_build_deb_rejects_unknown_option(self) -> None:
        result = subprocess.run(
            ["bash", str(DEPLOY_DIR / "build-deb.sh"), "--invalid-flag"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unknown option", result.stderr)

    @unittest.skipUnless(shutil.which("dpkg-deb"), "dpkg-deb not installed")
    def test_build_deb_packages_prebuilt_binaries(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            for name in ("mcm", "mcm-cli"):
                binary = bin_dir / name
                binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                os.chmod(binary, 0o755)

            result = subprocess.run(
                [
                    "bash",
                    str(DEPLOY_DIR / "build-deb.sh"),
                    "--arch",
                    "amd64",
                    "--bin-dir",
                    str(bin_dir),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            deb_path = ROOT / "dist" / "mcm_0.1.0_amd64.deb"
            self.assertTrue(deb_path.is_file(), f"Expected {deb_path}")
            try:
                inspect = subprocess.run(
                    ["dpkg-deb", "-I", str(deb_path)],
                    capture_output=True,
                    text=True,
                    check=True,
                )
                self.assertIn("Package: mcm", inspect.stdout)
                self.assertIn("Architecture: amd64", inspect.stdout)
            finally:
                if deb_path.exists():
                    deb_path.unlink()


if __name__ == "__main__":
    unittest.main()
