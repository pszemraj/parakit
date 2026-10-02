"""Regression checks for the profiling collector's input metadata."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "profile_memory", Path(__file__).resolve().parents[1] / "scripts/profile_memory.py",
)
profile_memory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile_memory)


class FileIdentityTests(unittest.TestCase):
    def test_both_flag_forms_record_path_and_size_without_reading(self) -> None:
        scratch = Path("target/tmp")
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as temporary:
            fixture = Path(temporary) / "fixture=input.wav"
            fixture.write_bytes(b"fixture")
            expected = {"path": str(fixture.resolve()), "bytes": 7}
            with patch.object(Path, "open", side_effect=AssertionError("input was read")):
                for flag in ("--audio", "--model"):
                    for arguments in ([flag, str(fixture)], [f"{flag}={fixture}"]):
                        with self.subTest(arguments=arguments):
                            self.assertEqual(
                                profile_memory.file_identity(["profile-memory", *arguments], flag),
                                expected,
                            )

    def test_absent_input_flag_has_no_identity(self) -> None:
        self.assertIsNone(profile_memory.file_identity(["profile-memory"], "--audio"))


if __name__ == "__main__":
    unittest.main()
