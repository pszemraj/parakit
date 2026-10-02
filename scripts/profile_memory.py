#!/usr/bin/env python3
"""Sample a profile-memory example at acknowledged, stable checkpoints.

Uses only Python's standard library. Outputs metrics, native diagnostics, and
optional macOS vmmap summaries; never writes audio or transcript contents.
"""

import argparse
import ctypes
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time


def windows_memory(pid):
    """Read working set and private commit through GetProcessMemoryInfo."""
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
        ] + [(name, ctypes.c_size_t) for name in (
            "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
            "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage",
            "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage",
            "PrivateUsage",
        )]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    psapi.GetProcessMemoryInfo.argtypes = [
        wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD,
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


def proc_fields(path):
    """Retain kernel memory fields in bytes, with their original names."""
    fields = {}
    for line in path.read_text().splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[2] == "kB":
            fields[parts[0].rstrip(":")] = int(parts[1]) * 1024
    return fields


def host_memory(pid):
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
        ["ps", "-o", "rss=", "-p", str(pid)], text=True, timeout=10,
    )
    return {"rss_bytes": int(rss.strip()) * 1024}


def nvidia_memory(pid):
    """Keep unsupported WDDM/driver readings explicit instead of recording zero."""
    if not shutil.which("nvidia-smi"):
        return {"available": False}
    result = subprocess.run(
        ["nvidia-smi", "--query-compute-apps=pid,used_gpu_memory",
         "--format=csv,noheader,nounits"],
        capture_output=True, text=True, timeout=15, check=False,
    )
    rows = []
    for line in result.stdout.splitlines():
        fields = [part.strip() for part in line.split(",")]
        if len(fields) == 2 and fields[0] == str(pid):
            rows.append({"used_gpu_MiB": int(fields[1]) if fields[1].isdigit() else None,
                         "reported": fields[1]})
    return {"available": result.returncode == 0, "process_rows": rows,
            "error": result.stderr.strip() or None}


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--vmmap", action="store_true", help="save macOS allocation summaries")
    parser.add_argument("--nvidia", action="store_true", help="sample process VRAM using nvidia-smi")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("pass the profile-memory executable and arguments after --")
    command.append("--wait-for-sampler")
    args.output.mkdir(parents=True, exist_ok=False)
    metadata = {"command": command, "platform": platform.platform(),
                "started_unix": time.time(), "model": file_identity(command, "--model"),
                "audio": file_identity(command, "--audio")}
    repo = Path(__file__).resolve().parent.parent
    for key, git_args in (
        ("commit", ["rev-parse", "HEAD"]),
        ("crispasr", ["submodule", "status", "vendor/CrispASR"]),
        ("working_tree", ["status", "--short"]),
    ):
        result = subprocess.run(["git", *git_args], cwd=repo, capture_output=True,
                                text=True, check=False)
        metadata[key] = result.stdout.strip()
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    with (args.output / "native.stderr").open("w") as native, \
            (args.output / "metrics.jsonl").open("w") as metrics:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=native, text=True, bufsize=1)
        try:
            for line in process.stdout:
                record = json.loads(line)
                if record["pid"] != process.pid:
                    raise RuntimeError("checkpoint PID does not match the launched process")
                record["sampled_unix"] = time.time()
                record["host"] = host_memory(process.pid)
                if args.nvidia:
                    record["nvidia"] = nvidia_memory(process.pid)
                if args.vmmap and sys.platform == "darwin":
                    name = f'{record["cycle"]:02d}-{record["phase"]}.vmmap.txt'
                    vmmap = subprocess.run(["vmmap", "-summary", str(process.pid)],
                                           capture_output=True, text=True, timeout=30, check=False)
                    (args.output / name).write_text(vmmap.stdout + vmmap.stderr)
                    record["vmmap"] = {"file": name, "exit_code": vmmap.returncode}
                metrics.write(json.dumps(record) + "\n")
                metrics.flush()
                print(json.dumps(record), flush=True)
                process.stdin.write("continue\n")
                process.stdin.flush()
            status = process.wait()
            if status:
                raise RuntimeError(f"profile-memory exited {status}; see {args.output / 'native.stderr'}")
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            process.stdin.close()
            process.stdout.close()


if __name__ == "__main__":
    main()
