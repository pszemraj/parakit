"""Regression checks for profiler metadata, sampling, and checkpoint acknowledgement."""

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "profile_memory",
    Path(__file__).resolve().parents[1] / "scripts/profile_memory.py",
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
            with patch.object(
                Path, "open", side_effect=AssertionError("input was read")
            ):
                for flag in ("--audio", "--model"):
                    for arguments in ([flag, str(fixture)], [f"{flag}={fixture}"]):
                        with self.subTest(arguments=arguments):
                            self.assertEqual(
                                profile_memory.file_identity(
                                    ["profile-memory", *arguments], flag
                                ),
                                expected,
                            )

    def test_absent_input_flag_has_no_identity(self) -> None:
        self.assertIsNone(profile_memory.file_identity(["profile-memory"], "--audio"))


class CollectorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        Path("target/tmp").mkdir(parents=True, exist_ok=True)

    @staticmethod
    def checkpoints(keep_loaded: bool, full_first: bool) -> list[tuple[int, str]]:
        phases = [(0, "before_load")]
        for cycle in (1, 2):
            if cycle == 1 or not keep_loaded:
                phases.extend([(cycle, "after_load"), (cycle, "after_warmup")])
            transcripts = (
                ("after_full", "after_short")
                if full_first
                else ("after_short", "after_full")
            )
            phases.extend((cycle, phase) for phase in transcripts)
            phases.append((cycle, "retained" if keep_loaded else "offloaded"))
        return [*phases, (2, "closed")]

    def run_child(self, output: Path, script: str, *arguments: str) -> tuple[int, str]:
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()) as errors,
        ):
            status = profile_memory.main(
                [
                    "--output",
                    str(output),
                    "--",
                    sys.executable,
                    "-c",
                    script,
                    *arguments,
                ]
            )
        return status, errors.getvalue()

    def child_script(
        self,
        phases: list[tuple[int, str]],
        *,
        keep_loaded: bool = False,
        full_first: bool = False,
        transient: bool = False,
    ) -> str:
        return f"""
import json, os, sys, time
for cycle, phase in {phases!r}:
    if {transient!r} and phase == 'after_load':
        allocation = bytearray(64 * 1024 * 1024)
        time.sleep(0.15)
        del allocation
    print(json.dumps({{'pid': os.getpid(), 'cycle': cycle, 'phase': phase,
                     'keep_loaded': {keep_loaded!r}, 'full_first': {full_first!r}}}), flush=True)
    if sys.stdin.readline() != 'continue\\n':
        raise SystemExit(9)
"""

    def test_complete_lifecycle_is_acknowledged_in_both_orders_and_modes(self) -> None:
        for keep_loaded in (False, True):
            for full_first in (False, True):
                with (
                    self.subTest(keep_loaded=keep_loaded, full_first=full_first),
                    tempfile.TemporaryDirectory(dir="target/tmp") as temporary,
                ):
                    output = Path(temporary) / "profile"
                    phases = self.checkpoints(keep_loaded, full_first)
                    script = self.child_script(
                        phases, keep_loaded=keep_loaded, full_first=full_first
                    )
                    self.assertEqual(self.run_child(output, script), (0, ""))
                    records = [
                        json.loads(line)
                        for line in (output / "metrics.jsonl")
                        .read_text(encoding="utf-8")
                        .splitlines()
                    ]
                    self.assertEqual(
                        [(row["cycle"], row["phase"]) for row in records], phases
                    )
                    self.assertTrue(
                        all(
                            row["host_interval_peak"]["samples"] >= 1 for row in records
                        )
                    )

    def test_empty_incomplete_and_failed_children_return_failure(self) -> None:
        complete = self.checkpoints(False, False)
        for script, message in (
            ("pass", "complete checkpoint lifecycle"),
            (self.child_script(complete[:-1]), "complete checkpoint lifecycle"),
            (
                self.child_script(
                    [row for row in complete if row != (2, "after_warmup")]
                ),
                "incomplete or out-of-order",
            ),
            (
                "import sys; print('child failed', file=sys.stderr); sys.exit(7)",
                "exited 7",
            ),
        ):
            with (
                self.subTest(message=message),
                tempfile.TemporaryDirectory(dir="target/tmp") as temporary,
            ):
                output = Path(temporary) / "profile"
                status, errors = self.run_child(output, script)
                self.assertEqual(status, 1)
                self.assertIn(message, errors)
                self.assertNotIn("Traceback", errors)
                if "exited" in message:
                    self.assertIn(
                        "child failed",
                        (output / "native.stderr").read_text(encoding="utf-8"),
                    )

    def test_bad_input_or_executable_does_not_create_output_directory(self) -> None:
        with tempfile.TemporaryDirectory(dir="target/tmp") as temporary:
            output = Path(temporary) / "profile"
            status, errors = self.run_child(
                output, "pass", "--audio", str(Path(temporary) / "missing.wav")
            )
            self.assertEqual(status, 1)
            self.assertIn("missing.wav", errors)
            self.assertFalse(output.exists())
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(
                    profile_memory.main(
                        [
                            "--output",
                            str(output),
                            "--",
                            str(Path(temporary) / "missing-binary"),
                        ]
                    ),
                    1,
                )
            self.assertFalse(output.exists())
            self.assertEqual(
                self.run_child(
                    output, self.child_script(self.checkpoints(False, False))
                ),
                (0, ""),
            )

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux RSS allocation check")
    def test_sampling_records_a_transient_load_allocation(self) -> None:
        with tempfile.TemporaryDirectory(dir="target/tmp") as temporary:
            output = Path(temporary) / "profile"
            script = self.child_script(self.checkpoints(False, False), transient=True)
            self.assertEqual(self.run_child(output, script), (0, ""))
            records = [
                json.loads(line)
                for line in (output / "metrics.jsonl")
                .read_text(encoding="utf-8")
                .splitlines()
            ]
            loaded = [record for record in records if record["phase"] == "after_load"]
            for record in loaded:
                self.assertGreater(
                    record["host_interval_peak"]["rss_bytes"],
                    record["host"]["status_bytes"]["VmRSS"] + 32 * 1024 * 1024,
                )


class HostPeakSamplerTests(unittest.TestCase):
    def test_interval_reset_does_not_reuse_lifetime_peak_or_late_sample(self) -> None:
        sampler = profile_memory.HostPeakSampler(1)
        sampler.observe({"status_bytes": {"VmRSS": 100}}, time.monotonic())
        sampler.observe({"status_bytes": {"VmRSS": 40}}, time.monotonic())
        self.assertEqual(sampler.snapshot()["rss_bytes"], 100)
        previous = time.monotonic()
        sampler.reset()
        sampler.observe({"status_bytes": {"VmRSS": 200}}, previous)
        sampler.observe({"status_bytes": {"VmRSS": 30}}, time.monotonic())
        self.assertEqual(sampler.snapshot()["rss_bytes"], 30)
        self.assertEqual(sampler.snapshot()["samples"], 1)

    def test_windows_working_set_keeps_its_native_name(self) -> None:
        sampler = profile_memory.HostPeakSampler(1)
        sampler.observe(
            {"working_set_bytes": 50, "peak_working_set_bytes": 100}, time.monotonic()
        )
        self.assertEqual(sampler.snapshot()["working_set_bytes"], 50)
        self.assertNotIn("rss_bytes", sampler.snapshot())

    def test_annotations_are_postponed_for_python39(self) -> None:
        self.assertIsInstance(
            profile_memory.file_identity.__annotations__["return"], str
        )


if __name__ == "__main__":
    unittest.main()
