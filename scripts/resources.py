"""Measure process startup, CPU, peak RSS, and binary size without a model call.

python3 scripts/resources.py target/release/ainz
python3 scripts/resources.py target/release/examples/embedded --embedded
"""

import argparse
import json
import platform
from pathlib import Path
import resource
import statistics
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--embedded", action="store_true")
    parser.add_argument("--samples", type=int, default=25)
    args = parser.parse_args()
    if args.samples < 2:
        parser.error("at least two samples are required")
    binary = args.binary.resolve(strict=True)
    command = [str(binary)] + ([] if args.embedded else ["--version"])
    timings = []
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    for _ in range(args.samples):
        start = time.perf_counter()
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, timeout=30)
        timings.append((time.perf_counter() - start) * 1000)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    warm = sorted(timings[1:])
    print(json.dumps({
        "platform": platform.platform(),
        "binary": str(binary),
        "scenario": "two embedded turns with checkpoint/resume" if args.embedded else "--version",
        "samples": args.samples,
        "binary_mib": round(binary.stat().st_size / 1024**2, 2),
        "first_launch_ms": round(timings[0], 2),
        "warm_median_ms": round(statistics.median(warm), 2),
        "warm_p95_ms": round(warm[min(len(warm) - 1, int(len(warm) * .95))], 2),
        "peak_rss_mib": round(after.ru_maxrss / (1024**2 if sys.platform == "darwin" else 1024), 2),
        "mean_cpu_ms": round(1000 * (after.ru_utime + after.ru_stime -
                                    before.ru_utime - before.ru_stime) / args.samples, 2),
    }, indent=2))


if __name__ == "__main__":
    main()
