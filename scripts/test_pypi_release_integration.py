"""Exercise PyPI's final distribution-set check through its CLI and HTTP API."""

from __future__ import annotations

import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from threading import Thread
import unittest


CHECKER = Path(__file__).resolve().parent / "check_pypi_wheels.py"
TAGS = ["macosx_11_0_arm64", "macosx_10_15_x86_64", "manylinux_2_39_aarch64",
        "manylinux_2_39_x86_64", "win_amd64"]


class PyPIHandler(BaseHTTPRequestHandler):
    """Serve a version JSON fixture through a real local HTTP endpoint."""

    def do_GET(self) -> None:
        if self.path != "/pypi/jevscript/0.1.0/json":
            self.send_error(404)
            return
        data = self.server.payload.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: object) -> None:
        pass


class PyPIReleaseIntegrationTests(unittest.TestCase):
    """The published version must contain exactly the reviewed five wheels."""

    def test_final_set_rejects_extra_sdist_and_preserves_digest_checks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wheelhouse = root / "bundle/wheelhouse"
            wheelhouse.mkdir(parents=True)
            entries = []
            for tag in TAGS:
                wheel = wheelhouse / f"jevscript-0.1.0-py3-none-{tag}.whl"
                wheel.write_bytes(tag.encode())
                entries.append({"filename": wheel.name,
                                "digests": {"sha256": hashlib.sha256(wheel.read_bytes()).hexdigest()}})
            payload = root / "pypi.json"
            payload.write_text(json.dumps({"urls": entries}))
            server = ThreadingHTTPServer(("127.0.0.1", 0), PyPIHandler)
            server.payload = payload
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                command = [sys.executable, str(CHECKER), str(root / "bundle"), "--require-all",
                           "--registry", f"http://127.0.0.1:{server.server_port}"]
                def run() -> subprocess.CompletedProcess[str]:
                    return subprocess.run(command, capture_output=True, text=True)

                self.assertEqual(run().returncode, 0)
                payload.write_text(json.dumps({"urls": [*entries, {"filename": "jevscript-0.1.0.tar.gz",
                                                              "digests": {"sha256": "unused"}}]}))
                extra = run()
                self.assertNotEqual(extra.returncode, 0)
                self.assertIn("unreviewed distributions", extra.stderr)
                entries[0]["digests"]["sha256"] = "incorrect"
                payload.write_text(json.dumps({"urls": entries}))
                bad_digest = run()
                self.assertNotEqual(bad_digest.returncode, 0)
                self.assertIn("differs from verified bundle", bad_digest.stderr)
            finally:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    unittest.main()
