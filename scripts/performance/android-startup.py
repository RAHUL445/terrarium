#!/usr/bin/env python3
"""Read duration-only startup records from the running Android app.

Build with --features startup-profiling and open the app on the phone first.
Does not launch/stop the app, clear logs/data, or save raw logcat output.
"""
import argparse
import json
import subprocess
from pathlib import Path


def collect(adb, serial):
    prefix = [adb, "-s", serial]
    pid = subprocess.check_output(prefix + ["shell", "pidof", "io.github.rahul445.terrarium"], text=True).strip()
    if not pid.isdigit():
        raise RuntimeError("Expected one running app process")
    log = subprocess.check_output(prefix + ["logcat", "--pid=" + pid, "-d", "-v", "brief"], text=True)
    records = []
    phases = {"store-ready", "bundle-install", "plugin-host-create", "sync-open", "setup-ready", "plugin-load"}
    for line in log.splitlines():
        for marker in ("[terrarium-startup] ", "[terrarium-native-perf] "):
            if marker not in line:
                continue
            try:
                data = json.loads(line.split(marker, 1)[1])
                if marker == "[terrarium-startup] " and data.get("phase") in phases:
                    records.append({"phase": data["phase"], "duration_ms": float(data["duration_ms"])})
                elif marker == "[terrarium-native-perf] ":
                    records.append({key: float(data[key]) for key in ("setup_ms", "native_ready_ms", "frontend_ready_ms")})
            except (ValueError, TypeError, KeyError):
                continue
    return {"method": "Current app process; duration-only opt-in checkpoints. No OS cache flush. Ready is startup sequence completion, not first paint.", "records": records}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = collect(args.adb, args.serial)
    if not result["records"]:
        raise SystemExit("No startup checkpoints found; check build feature and app readiness")
    with args.output.open("x") as stream:
        json.dump(result, stream, indent=2, allow_nan=False)
        stream.write("\n")
    print(json.dumps(result, allow_nan=False))
