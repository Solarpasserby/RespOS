#!/usr/bin/env python3
import argparse
import csv
import re
import sys
from pathlib import Path


RESULT_KEYS = ("passed", "failed", "broken", "skipped", "warnings")
GROUP_RE = re.compile(r"^#### OS COMP TEST GROUP START (ltp-(?:musl|glibc)) ####$")
CASE_RE = re.compile(r"^RUN LTP CASE (?P<name>\S+)$")
END_RE = re.compile(r"^FAIL LTP CASE (?P<name>\S+) : (?P<ret>-?\d+)$")
ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
RESULT_TOKEN_RE = re.compile(r"\b(TPASS|TFAIL|TBROK|TCONF|TWARN)\b")
TOKEN_COUNTS = {
    "TPASS": "passed",
    "TFAIL": "failed",
    "TBROK": "broken",
    "TCONF": "skipped",
    "TWARN": "warnings",
}


def empty_counts():
    return {key: 0 for key in RESULT_KEYS}


def case_status(row):
    if row["ret"] == 32 or row["skipped"] > 0 and row["passed"] == 0:
        return "skipped"
    if row["broken"]:
        return "broken"
    if row["failed"]:
        return "failed"
    if row["warnings"]:
        return "warning"
    if row["passed"]:
        return "passed"
    return "empty"


def parse_ltp_log(path):
    rows = []
    group = None
    current = None
    in_summary = False

    def finish_case(ret):
        nonlocal current
        if current is None:
            return
        row = {
            "file": path.name,
            "arch": "rv" if path.name.startswith("rv") else "la"
            if path.name.startswith("la")
            else "",
            "group": group,
            "case": current["case"],
            "ret": ret,
            **current["counts"],
        }
        row["all"] = sum(row[key] for key in RESULT_KEYS)
        row["status"] = case_status(row)
        rows.append(row)
        current = None

    data = path.read_bytes().decode("latin-1", errors="replace")
    for raw in data.splitlines():
        line = raw.strip()

        group_match = GROUP_RE.match(line)
        if group_match:
            group = group_match.group(1)
            current = None
            in_summary = False
            continue

        if not group:
            continue

        case_match = CASE_RE.match(line)
        if case_match:
            current = {
                "case": case_match.group("name"),
                "counts": empty_counts(),
            }
            in_summary = False
            continue

        if current is None:
            continue

        end_match = END_RE.match(line)
        if end_match:
            finish_case(int(end_match.group("ret")))
            in_summary = False
            continue

        if group == "ltp-musl":
            if line == "Summary:":
                in_summary = True
                continue
            if in_summary:
                if not line:
                    in_summary = False
                    continue
                parts = line.split()
                if len(parts) >= 2 and parts[0] in RESULT_KEYS:
                    current["counts"][parts[0]] += int(parts[1])
            continue

        if group == "ltp-glibc":
            plain = ANSI_RE.sub("", line)
            token_match = RESULT_TOKEN_RE.search(plain)
            if token_match:
                current["counts"][TOKEN_COUNTS[token_match.group(1)]] += 1

    return rows


def summarize(rows):
    totals = {}
    for row in rows:
        key = (row["file"], row["group"])
        total = totals.setdefault(
            key,
            {
                "file": row["file"],
                "group": row["group"],
                "cases": 0,
                "passed": 0,
                "failed": 0,
                "broken": 0,
                "skipped": 0,
                "warnings": 0,
                "all": 0,
                "empty": 0,
            },
        )
        total["cases"] += 1
        for name in RESULT_KEYS:
            total[name] += row[name]
        total["all"] += row["all"]
        if row["status"] == "empty":
            total["empty"] += 1
    return list(totals.values())


def write_csv(rows, out):
    fields = [
        "file",
        "arch",
        "group",
        "case",
        "ret",
        "status",
        "passed",
        "failed",
        "broken",
        "skipped",
        "warnings",
        "all",
    ]
    writer = csv.DictWriter(out, fieldnames=fields)
    writer.writeheader()
    writer.writerows(rows)
    total = {field: "" for field in fields}
    total["file"] = "TOTAL"
    for field in ("passed", "failed", "broken", "skipped", "warnings", "all"):
        total[field] = sum(row[field] for row in rows)
    writer.writerow(total)


def write_baseline_like_csv(rows, out):
    fields = [
        "case",
        "ret",
        "status",
        "passed",
        "failed",
        "broken",
        "skipped",
        "warnings",
        "all",
    ]
    writer = csv.DictWriter(out, fieldnames=fields)
    writer.writeheader()
    writer.writerows({field: row[field] for field in fields} for row in rows)
    total = {field: "" for field in fields}
    total["case"] = "TOTAL"
    for field in ("passed", "failed", "broken", "skipped", "warnings", "all"):
        total[field] = sum(row[field] for row in rows)
    writer.writerow(total)


def write_split_csvs(rows, out_dir):
    out_dir.mkdir(parents=True, exist_ok=True)
    groups = {}
    for row in rows:
        groups.setdefault((row["file"], row["group"]), []).append(row)

    for (file_name, group), group_rows in sorted(groups.items()):
        stem = Path(file_name).stem
        path = out_dir / f"{stem}-{group}.csv"
        with path.open("w", newline="") as out:
            write_baseline_like_csv(group_rows, out)


def markdown_table(headers, rows):
    yield "| " + " | ".join(headers) + " |"
    yield "| " + " | ".join("---" for _ in headers) + " |"
    for row in rows:
        yield "| " + " | ".join(str(row.get(header, "")) for header in headers) + " |"


def write_markdown(rows, out):
    summary_headers = [
        "file",
        "group",
        "cases",
        "passed",
        "failed",
        "broken",
        "skipped",
        "warnings",
        "all",
        "empty",
    ]
    detail_headers = [
        "file",
        "group",
        "case",
        "ret",
        "status",
        "passed",
        "failed",
        "broken",
        "skipped",
        "warnings",
        "all",
    ]
    out.write("# LTP Local Report\n\n")
    out.write("## Summary\n\n")
    for line in markdown_table(summary_headers, summarize(rows)):
        out.write(line + "\n")
    out.write("\n## Cases\n\n")
    for line in markdown_table(detail_headers, rows):
        out.write(line + "\n")


def main():
    parser = argparse.ArgumentParser(description="Parse RespOS LTP output into a table.")
    parser.add_argument(
        "logs",
        nargs="*",
        type=Path,
        default=[Path("rv-output.txt"), Path("la-output.txt")],
        help="LTP output logs to parse. Defaults to rv-output.txt and la-output.txt.",
    )
    parser.add_argument(
        "--format",
        choices=("markdown", "csv"),
        default="markdown",
        help="Output table format.",
    )
    parser.add_argument("-o", "--output", type=Path, help="Write table to this file.")
    parser.add_argument(
        "--split-dir",
        type=Path,
        help="Also write one baseline-format CSV per input file and LTP group into this directory.",
    )
    args = parser.parse_args()

    rows = []
    for path in args.logs:
        if not path.exists():
            raise SystemExit(f"missing log file: {path}")
        rows.extend(parse_ltp_log(path))

    if args.split_dir:
        write_split_csvs(rows, args.split_dir)

    out = args.output.open("w", newline="") if args.output else sys.stdout
    try:
        if args.format == "csv":
            write_csv(rows, out)
        else:
            write_markdown(rows, out)
    finally:
        if args.output:
            out.close()


if __name__ == "__main__":
    main()
