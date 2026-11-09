#!/usr/bin/env python3
"""Launch an isolated profiling bundle repeatedly and wait for frontend readiness.

Uses TERRA_PROFILE_DIR and refuses an existing data root. It terminates only the
process it launched. This measures fresh processes, not cold OS disk caches.
"""
import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--app', type=Path, required=True)
parser.add_argument('--data', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--runs', type=int, default=3)
args = parser.parse_args()
if not args.data.is_absolute() or args.data.exists() or not 1 <= args.runs <= 10:
    parser.error('Use a new absolute data root and 1–10 runs')
binary = args.app / 'Contents/MacOS/terrarium-app'
if not binary.is_file():
    parser.error('Profiling app bundle executable not found')
args.data.mkdir(parents=True)
records = []
diagnostics = []
def save(failure=None):
    args.output.write_text(json.dumps({
        'method': 'Release app, fresh processes; first launch new data, later launches reuse the same isolated data. OS disk caches are not flushed. Ready is frontend startup completion, not first interactive paint or completion of every background widget.',
        'requested_runs': args.runs, 'completed_runs': len(records),
        'failure': failure, 'runs': records,
        'failure_output': diagnostics[-20:] if failure else [],
    }, indent=2) + '\n')

save()
for run in range(args.runs):
    started = time.perf_counter()
    process = subprocess.Popen([str(binary)], env={**os.environ, 'TERRA_PROFILE_DIR': str(args.data)},
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            buffered = b''
            while time.perf_counter() - started < 90:
                if process.poll() is not None:
                    save(f'Process exited early: {process.returncode}')
                    raise RuntimeError(f'Profiling process exited early: {process.returncode}')
                if not selector.select(timeout=1):
                    continue
                buffered += os.read(process.stdout.fileno(), 65536)
                lines = buffered.split(b'\n')
                buffered = lines.pop()
                diagnostics.extend(line.decode('utf-8', errors='replace')[:1000] for line in lines)
                ready = next((line for line in lines if line.startswith(b'[terrarium-native-perf] ')), None)
                if ready is None:
                    continue
                record = json.loads(ready.split(b' ', 1)[1])
                record.update(run=run + 1, new_data=run == 0,
                              process_ready_ms=round((time.perf_counter() - started) * 1000, 2))
                records.append(record)
                save()
                print(json.dumps(record), flush=True)
                break
            else:
                save('No frontend-ready signal in 90 seconds')
                raise TimeoutError('No frontend-ready signal in 90 seconds')
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stdout.close()
save()
