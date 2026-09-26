"""Check PyPI's published artifact metadata against the release bytes."""

from __future__ import annotations

import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from check_pypi_wheels import check


class PublishedWheelTests(unittest.TestCase):
    def test_existing_wheel_must_match_and_all_wheels_must_exist(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory)
            wheelhouse = bundle / "wheelhouse"
            wheelhouse.mkdir()
            wheel = wheelhouse / "jevscript-0.1.0-py3-none-win_amd64.whl"
            wheel.write_bytes(b"verified wheel")
            filename = wheel.name

            def response(digest: str | None) -> io.BytesIO:
                urls = [] if digest is None else [{"filename": filename, "digests": {"sha256": digest}}]
                return io.BytesIO(json.dumps({"urls": urls}).encode())

            with patch("check_pypi_wheels.urllib.request.urlopen", return_value=response(hashlib.sha256(wheel.read_bytes()).hexdigest())):
                check(bundle, require_all=True)
            with patch("check_pypi_wheels.urllib.request.urlopen", return_value=response("0" * 64)):
                with self.assertRaisesRegex(ValueError, "differs"):
                    check(bundle)
            with patch("check_pypi_wheels.urllib.request.urlopen", side_effect=lambda *args, **kwargs: response(None)):
                check(bundle)
                with self.assertRaisesRegex(ValueError, "missing"):
                    check(bundle, require_all=True)


if __name__ == "__main__":
    unittest.main()
