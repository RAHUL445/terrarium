#!/usr/bin/env python3
"""Seed an EMPTY, isolated `terra preview` host through real plugin operations.

Run once for --count 1000, inspect the UI, then --count 10000 --start 1000.
Never point this at the normal preview or desktop data. No deletion is performed.
"""
import argparse
import concurrent.futures
import json
import statistics
import time
import urllib.request
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port', type=int, default=8877)
parser.add_argument('--count', type=int, choices=[1000, 10000], required=True)
parser.add_argument('--start', type=int, default=0)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--confirm-isolated', action='store_true', required=True)
parser.add_argument('--measure-only', action='store_true', help='Recheck an existing fixture without adding records')
args = parser.parse_args()
if args.port == 8876 or not 0 <= args.start < args.count:
    parser.error('Use an isolated host (not port 8876), with 0 <= start < count')
url = f'http://127.0.0.1:{args.port}/invoke'

def invoke(command, **payload):
    body = json.dumps({'cmd': command, 'args': {'space': 'personal', **payload}}).encode()
    with urllib.request.urlopen(urllib.request.Request(url, body), timeout=120) as response:
        return json.load(response)['result']

def query(plugin, name, **params):
    return invoke('run_query', plugin=plugin, name=name, params=params)

existing = query('todo', 'counts', today='2026-11-04')[0]['open']
note_count = sum(row['count'] for row in query('notes', 'projects', project='') if row['project'] == '')
expected = args.count if args.measure_only else args.start
if existing != expected or note_count != expected:
    raise SystemExit(f'Refusing mismatched fixture: tasks={existing}, notes={note_count}, expected={expected}')

def seed(index):
    suffix = f'{index:05d}'
    keyword = ' needle-match' if index % 100 == 0 else ''
    task = invoke('run_action', plugin='todo', op='create', args={
        'title': f'Performance task {suffix}{keyword}', 'notes': 'Synthetic benchmark task. ' * 8,
        'tags': 'benchmark', 'priority': index % 4,
    })
    note = invoke('run_action', plugin='notes', op='create', args={
        'title': f'Performance note {suffix}{keyword}', 'project': 'Benchmark',
        'body': f'# Synthetic note {suffix}\n' + 'Local benchmark content. ' * 24,
    })
    return {'task': task['id'], 'note': note['id']}

started = time.perf_counter()
sample = None
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    indices = [] if args.measure_only else range(args.start, args.count)
    for count, result in enumerate(pool.map(seed, indices), 1):
        sample = result
        if count % 1000 == 0:
            print(f'Seeded {args.start + count} tasks and notes', flush=True)

cases = {
    'tasks_list': ('todo', 'tasks', {'today': '2026-11-04', 'filter': 'all', 'q': ''}),
    'tasks_search': ('todo', 'tasks', {'today': '2026-11-04', 'filter': 'all', 'q': 'needle-match'}),
    'notes_list': ('notes', 'list', {'q': '', 'project': '', 'utc_offset_ms': 19800000, 'today': '2026-11-04'}),
    'notes_search': ('notes', 'list', {'q': 'needle-match', 'project': '', 'utc_offset_ms': 19800000, 'today': '2026-11-04'}),
}
report = {'records_per_plugin': args.count, 'seed_seconds': round(time.perf_counter() - started, 2),
          'method': 'Real plugin operations; sequential HTTP query round trips against warm preview host. Seven measured samples after one warmup. Includes transport/serialization, excludes browser rendering.',
          'queries': {}, 'sample_ids': sample}
for label, (plugin, name, params) in cases.items():
    query(plugin, name, **params)
    times = []
    for _ in range(7):
        started = time.perf_counter()
        rows = query(plugin, name, **params)
        times.append(round((time.perf_counter() - started) * 1000, 2))
    report['queries'][label] = {'rows': len(rows), 'samples_ms': times,
                              'median_ms': statistics.median(times), 'max_ms': max(times)}
args.output.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2), flush=True)
