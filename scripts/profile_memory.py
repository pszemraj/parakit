#!/usr/bin/env python3
"""Sample a profile-memory example at acknowledged, stable checkpoints.

Uses only Python's standard library. Outputs metrics, native diagnostics, and
optional macOS vmmap summaries; never writes audio or transcript contents.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import platform
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path


def windows_memory(pid: int) -> dict[str, int]:
    """Read working set and private commit through GetProcessMemoryInfo."""
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
        ] + [
            (name, ctypes.c_size_t)
            for name in (
                "PeakWorkingSetSize",
                "WorkingSetSize",
                "QuotaPeakPagedPoolUsage",
                "QuotaPagedPoolUsage",
                "QuotaPeakNonPagedPoolUsage",
                "QuotaNonPagedPoolUsage",
                "PagefileUsage",
                "PeakPagefileUsage",
                "PrivateUsage",
            )
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    psapi.GetProcessMemoryInfo.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(Counters),
        wintypes.DWORD,
    ]
    psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
    handle = kernel.OpenProcess(0x0400 | 0x0010, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return {
            "working_set_bytes": counters.WorkingSetSize,
            "peak_working_set_bytes": counters.PeakWorkingSetSize,
            "private_commit_bytes": counters.PrivateUsage,
        }
    finally:
        kernel.CloseHandle(handle)


def proc_fields(path: Path) -> dict[str, int]:
    """Retain kernel memory fields in bytes, with their original names."""
    fields = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[2] == "kB":
            fields[parts[0].rstrip(":")] = int(parts[1]) * 1024
    return fields


def host_memory(pid: int) -> dict:
    """Return native metrics without pretending RSS and private commit are equal."""
    if sys.platform == "win32":
        return windows_memory(pid)
    if sys.platform.startswith("linux"):
        root = Path("/proc") / str(pid)
        return {
            "smaps_rollup_bytes": proc_fields(root / "smaps_rollup"),
            "status_bytes": proc_fields(root / "status"),
        }
    rss = subprocess.check_output(
        ["ps", "-o", "rss=", "-p", str(pid)],
        text=True,
        timeout=10,
    )
    return {"rss_bytes": int(rss.strip()) * 1024}


def nvidia_memory(pid: int) -> dict:
    """Keep unsupported WDDM/driver readings explicit instead of recording zero."""
    if not shutil.which("nvidia-smi"):
        return {"available": False}
    result = subprocess.run(
        [
            "nvidia-smi",
            "--query-compute-apps=pid,used_gpu_memory",
            "--format=csv,noheader,nounits",
        ],
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )
    rows = []
    for line in result.stdout.splitlines():
        fields = [part.strip() for part in line.split(",")]
        if len(fields) == 2 and fields[0] == str(pid):
            rows.append(
                {
                    "used_gpu_MiB": int(fields[1]) if fields[1].isdigit() else None,
                    "reported": fields[1],
                }
            )
    return {
        "available": result.returncode == 0,
        "process_rows": rows,
        "error": result.stderr.strip() or None,
    }


def file_identity(command: list[str], flag: str) -> dict[str, str | int] | None:
    """Record input path and size without reading its contents."""
    for index, argument in enumerate(command):
        if argument == flag:
            path = Path(command[index + 1])
        elif argument.startswith(flag + "="):
            path = Path(argument.split("=", 1)[1])
        else:
            continue
        return {"path": str(path.resolve()), "bytes": path.stat().st_size}
    return None


class HostPeakSampler:
    """Sample residency between checkpoints without using lifetime high-water marks."""

    interval_seconds = 0.02

    def __init__(self, pid: int) -> None:
        self.pid = pid
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._peak = {}
        self._samples = 0
        self._started = time.monotonic()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def observe(self, host: dict, sampled_at: float) -> None:
        """Retain the largest resident reading in the current interval."""
        if "status_bytes" in host:
            name, value = "rss_bytes", host["status_bytes"]["VmRSS"]
        elif "working_set_bytes" in host:
            name, value = "working_set_bytes", host["working_set_bytes"]
        else:
            name, value = "rss_bytes", host["rss_bytes"]
        with self._lock:
            if sampled_at >= self._started:
                self._peak[name] = max(self._peak.get(name, 0), value)
                self._samples += 1

    def snapshot(self) -> dict:
        """Return the interval's sampled peak with its sampling cadence."""
        with self._lock:
            return {
                **self._peak,
                "samples": self._samples,
                "interval_seconds": self.interval_seconds,
            }

    def reset(self) -> None:
        """Begin the next operation interval immediately before acknowledging it."""
        with self._lock:
            self._peak.clear()
            self._samples = 0
            self._started = time.monotonic()

    def _run(self) -> None:
        while not self._stop.is_set():
            sampled_at = time.monotonic()
            try:
                self.observe(host_memory(self.pid), sampled_at)
            except (OSError, ValueError, subprocess.SubprocessError):
                # The child can exit while a background sample is in progress.
                # Required checkpoint reads still fail visibly in the collector.
                pass
            self._stop.wait(self.interval_seconds)

    def start(self) -> None:
        """Start background sampling of the launched child."""
        self._thread.start()

    def close(self) -> None:
        """Stop sampling and wait for any native memory query to finish."""
        self._stop.set()
        self._thread.join()


def validate_checkpoints(records: list[dict]) -> None:
    """Require a complete lifecycle, including final close, from a successful child."""
    if not records or records[-1]["phase"] != "closed":
        raise RuntimeError(
            "profile-memory did not emit a complete checkpoint lifecycle"
        )
    cycles = records[-1]["cycle"]
    keep_loaded = records[0]["keep_loaded"]
    full_first = records[0]["full_first"]
    expected = [(0, "before_load")]
    for cycle in range(1, cycles + 1):
        if cycle == 1 or not keep_loaded:
            expected.extend([(cycle, "after_load"), (cycle, "after_warmup")])
        phases = (
            ("after_full", "after_short")
            if full_first
            else ("after_short", "after_full")
        )
        expected.extend((cycle, phase) for phase in phases)
        expected.append((cycle, "retained" if keep_loaded else "offloaded"))
    expected.append((cycles, "closed"))
    if (
        cycles < 1
        or [(record["cycle"], record["phase"]) for record in records] != expected
    ):
        raise RuntimeError(
            "profile-memory emitted an incomplete or out-of-order checkpoint lifecycle"
        )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Parse collector options and the profile-memory child command."""
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.ArgumentDefaultsHelpFormatter
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--vmmap", action="store_true", help="save macOS allocation summaries"
    )
    parser.add_argument(
        "--nvidia", action="store_true", help="sample process VRAM using nvidia-smi"
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("pass the profile-memory executable and arguments after --")
    args.command = command
    return args


def collect(output: Path, command: list[str], *, vmmap: bool, nvidia: bool) -> None:
    """Run and acknowledge a complete profile while collecting native memory metrics."""
    command = list(command)
    command.append("--wait-for-sampler")
    metadata = {
        "command": command,
        "platform": platform.platform(),
        "started_unix": time.time(),
        "model": file_identity(command, "--model"),
        "audio": file_identity(command, "--audio"),
    }
    if not shutil.which(command[0]):
        raise FileNotFoundError(f"profile-memory executable not found: {command[0]}")
    output.mkdir(parents=True, exist_ok=False)
    repo = Path(__file__).resolve().parent.parent
    for key, git_args in (
        ("commit", ["rev-parse", "HEAD"]),
        ("crispasr", ["submodule", "status", "vendor/CrispASR"]),
        ("working_tree", ["status", "--short"]),
    ):
        result = subprocess.run(
            ["git", *git_args], cwd=repo, capture_output=True, text=True, check=False
        )
        metadata[key] = result.stdout.strip()
    (output / "metadata.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )
    with (
        (output / "native.stderr").open("w", encoding="utf-8") as native,
        (output / "metrics.jsonl").open("w", encoding="utf-8") as metrics,
    ):
        process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=native,
            text=True,
            bufsize=1,
        )
        sampler = HostPeakSampler(process.pid)
        sampler.start()
        records = []
        try:
            for line in process.stdout:
                record = json.loads(line)
                if record["pid"] != process.pid:
                    raise RuntimeError(
                        "checkpoint PID does not match the launched process"
                    )
                record["sampled_unix"] = time.time()
                record["host"] = host_memory(process.pid)
                sampler.observe(record["host"], time.monotonic())
                record["host_interval_peak"] = sampler.snapshot()
                records.append(record)
                if nvidia:
                    record["nvidia"] = nvidia_memory(process.pid)
                if vmmap and sys.platform == "darwin":
                    name = f"{record['cycle']:02d}-{record['phase']}.vmmap.txt"
                    summary = subprocess.run(
                        ["vmmap", "-summary", str(process.pid)],
                        capture_output=True,
                        text=True,
                        timeout=30,
                        check=False,
                    )
                    (output / name).write_text(
                        summary.stdout + summary.stderr, encoding="utf-8"
                    )
                    record["vmmap"] = {"file": name, "exit_code": summary.returncode}
                metrics.write(json.dumps(record) + "\n")
                metrics.flush()
                print(json.dumps(record), flush=True)
                sampler.reset()
                process.stdin.write("continue\n")
                process.stdin.flush()
            status = process.wait()
            if status:
                raise RuntimeError(
                    f"profile-memory exited {status}; see {output / 'native.stderr'}"
                )
            validate_checkpoints(records)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            sampler.close()
            process.stdin.close()
            process.stdout.close()


def main(argv: list[str] | None = None) -> int:
    """Collect a profile and report expected failures without a traceback."""
    args = parse_args(argv)
    try:
        collect(args.output, args.command, vmmap=args.vmmap, nvidia=args.nvidia)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
