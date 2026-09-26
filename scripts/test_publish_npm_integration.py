"""Exercise the release publish command against an HTTP registry and fake npm."""

from __future__ import annotations

import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from threading import Thread
import unittest


ROOT = Path(__file__).resolve().parents[1]
PUBLISH = ROOT / "scripts/publish_npm.py"


class RegistryHandler(BaseHTTPRequestHandler):
    """Serve npm's version metadata from a file the fake npm process updates."""

    def do_GET(self) -> None:
        if self.path != "/jevscript/0.1.0" or not self.server.metadata.is_file():
            self.send_error(404)
            return
        data = self.server.metadata.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: object) -> None:
        pass


class PublishIntegrationTests(unittest.TestCase):
    """Cross the CLI, HTTP, tarball, and npm process boundaries."""

    def setUp(self) -> None:
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.tarball = self.bundle / "jevscript-0.1.0.tgz"
        data = json.dumps({"name": "jevscript", "version": "0.1.0"}).encode()
        with tarfile.open(self.tarball, "w:gz") as archive:
            member = tarfile.TarInfo("package/package.json")
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))
        self.metadata = self.root / "registry.json"
        self.calls = self.root / "npm-calls.json"
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        fake_npm = fake_bin / "npm"
        fake_npm.write_text(
            "#!/usr/bin/env python3\n"
            "import base64,hashlib,json,os,pathlib,sys\n"
            "args=sys.argv[1:]\n"
            "path=pathlib.Path(args[1])\n"
            "pathlib.Path(os.environ['NPM_CALLS']).write_text(json.dumps(args))\n"
            "digest=base64.b64encode(hashlib.sha512(path.read_bytes()).digest()).decode()\n"
            "pathlib.Path(os.environ['NPM_METADATA']).write_text(json.dumps({'name':'jevscript','version':'0.1.0','dist':{'integrity':'sha512-'+digest}}))\n"
        )
        fake_npm.chmod(0o755)
        self.env = {**os.environ, "PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}",
                    "NPM_CALLS": str(self.calls), "NPM_METADATA": str(self.metadata)}
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), RegistryHandler)
        self.server.metadata = self.metadata
        self.thread = Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.registry = f"http://127.0.0.1:{self.server.server_port}"

    def metadata_for(self, tarball: bytes) -> None:
        digest = base64.b64encode(hashlib.sha512(tarball).digest()).decode()
        self.metadata.write_text(json.dumps({"name": "jevscript", "version": "0.1.0",
                                             "dist": {"integrity": f"sha512-{digest}"}}))

    def run_publish(self, *, bootstrap: bool = False, credential: bool = False,
                    require_published: bool = False) -> subprocess.CompletedProcess[str]:
        env = dict(self.env)
        env.pop("NODE_AUTH_TOKEN", None)
        if credential:
            env["NODE_AUTH_TOKEN"] = "test-only"
        args = [sys.executable, str(PUBLISH), "bundle", "--registry", self.registry]
        if bootstrap:
            args.append("--bootstrap")
        if require_published:
            args.append("--require-published")
        return subprocess.run(args, cwd=self.root, env=env, capture_output=True, text=True)

    def test_missing_version_bootstraps_exact_relative_tarball(self) -> None:
        result = self.run_publish(bootstrap=True, credential=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.calls.read_text()),
                         ["publish", "./bundle/jevscript-0.1.0.tgz", "--access", "public", "--provenance"])
        self.assertIn("exact verified tarball", result.stdout)

    def test_missing_bootstrap_credential_stops_before_publish(self) -> None:
        result = self.run_publish(bootstrap=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("bootstrap credential is absent", result.stderr)
        self.assertFalse(self.calls.exists())

    def test_exact_existing_version_skips_publish(self) -> None:
        self.metadata_for(self.tarball.read_bytes())
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already holds the exact", result.stdout)
        self.assertFalse(self.calls.exists())

    def test_different_existing_version_blocks_publish(self) -> None:
        self.metadata_for(b"different tarball")
        result = self.run_publish()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("differs from the verified tarball", result.stderr)
        self.assertFalse(self.calls.exists())

    def test_normal_oidc_path_publishes_missing_version(self) -> None:
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.calls.read_text())[1], "./bundle/jevscript-0.1.0.tgz")

    def test_final_bootstrap_check_rejects_missing_version(self) -> None:
        result = self.run_publish(require_published=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing the verified tarball", result.stderr)
        self.assertFalse(self.calls.exists())


if __name__ == "__main__":
    unittest.main()
