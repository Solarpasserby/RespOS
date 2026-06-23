#!/usr/bin/env python3
"""Filter ltp-linux-baseline.log to only include tests listed in oscomp_ltp_list.txt."""

import sys
import os

def parse_test_list(path):
    """Extract active (non-commented) test names from the list file."""
    tests = set()
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#'):
                continue
            tests.add(line)
    return tests

def filter_log(test_set, log_path, output_path):
    """Write log entries only for tests in test_set to output_path."""
    with open(log_path) as fin, open(output_path, 'w') as fout:
        writing = False
        for line in fin:
            if line.startswith('RUN LTP CASE '):
                test_name = line[len('RUN LTP CASE '):].strip()
                writing = test_name in test_set
            if writing:
                fout.write(line)

def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    repo_root = os.path.dirname(script_dir)

    test_list_path = os.path.join(repo_root, 'user', 'oscomp_ltp_list.txt')
    log_path = os.path.join(script_dir, 'baseline', 'ltp-linux-baseline.log')
    output_path = os.path.join(script_dir, 'baseline', 'ltp-linux-baseline-filtered.log')

    test_set = parse_test_list(test_list_path)
    print(f"Loaded {len(test_set)} test names from test list")
    filter_log(test_set, log_path, output_path)

    # Count lines in output vs input
    with open(log_path) as f:
        total_lines = sum(1 for _ in f)
    with open(output_path) as f:
        filtered_lines = sum(1 for _ in f)
    print(f"Input:  {total_lines} lines")
    print(f"Output: {filtered_lines} lines")
    print(f"Written to: {output_path}")

if __name__ == '__main__':
    main()
