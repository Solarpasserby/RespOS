#!/usr/bin/env python3
import argparse
import csv
import sys
from pathlib import Path


RESPOS_KEY = ("file", "group", "case")
COUNT_FIELDS = ("passed", "failed", "broken", "skipped", "warnings", "all")


def read_csv(path):
    with path.open(newline="") as f:
        return [
            row
            for row in csv.DictReader(f)
            if row.get("case") != "TOTAL" and row.get("file") != "TOTAL"
        ]


def read_case_list(path):
    cases = set()
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        cases.add(line.split()[0])
    return cases


def as_int(row, key, default=0):
    value = row.get(key, "")
    if value == "":
        return default
    return int(value)


def baseline_by_case(rows):
    return {row["case"]: row for row in rows}


def respos_by_key(rows):
    return {(row["file"], row["group"], row["case"]): row for row in rows}


def is_baseline_candidate(row):
    return row["status"] == "passed" and as_int(row, "passed") > 0


def classify(base, respos):
    if respos is None:
        return "missing"
    if respos["status"] == "passed":
        if as_int(respos, "passed") < as_int(base, "passed"):
            return "partial"
        return "ok"
    return respos["status"]


def compare(baseline_rows, respos_rows, case_filter=None):
    base_cases = baseline_by_case(baseline_rows)
    respos_keys = sorted({(row["file"], row["group"]) for row in respos_rows})
    respos_cases = respos_by_key(respos_rows)

    rows = []
    for file_name, group in respos_keys:
        for case, base in base_cases.items():
            if case_filter is not None and case not in case_filter:
                continue
            if not is_baseline_candidate(base):
                continue
            respos = respos_cases.get((file_name, group, case))
            status = classify(base, respos)
            if status == "ok":
                continue

            row = {
                "file": file_name,
                "group": group,
                "case": case,
                "status": status,
                "lost_passed": as_int(base, "passed")
                - (as_int(respos, "passed") if respos else 0),
                "linux_status": base["status"],
                "linux_ret": base["ret"],
                "linux_passed": as_int(base, "passed"),
                "linux_all": as_int(base, "all"),
                "respos_status": respos["status"] if respos else "missing",
                "respos_ret": respos["ret"] if respos else "",
            }
            for field in COUNT_FIELDS:
                row[f"respos_{field}"] = as_int(respos, field) if respos else 0
            rows.append(row)

    rows.sort(
        key=lambda row: (
            row["file"],
            row["group"],
            -row["lost_passed"],
            row["case"],
        )
    )
    return rows


def write_csv(rows, out):
    fields = [
        "file",
        "group",
        "case",
        "status",
        "lost_passed",
        "linux_status",
        "linux_ret",
        "linux_passed",
        "linux_all",
        "respos_status",
        "respos_ret",
        "respos_passed",
        "respos_failed",
        "respos_broken",
        "respos_skipped",
        "respos_warnings",
        "respos_all",
    ]
    writer = csv.DictWriter(out, fieldnames=fields)
    writer.writeheader()
    writer.writerows(rows)


def write_split_csvs(rows, out_dir, expected_keys=None):
    out_dir.mkdir(parents=True, exist_ok=True)
    groups = {}
    for row in rows:
        groups.setdefault((row["file"], row["group"]), []).append(row)

    keys = set(groups)
    if expected_keys:
        keys.update(expected_keys)

    for file_name, group in sorted(keys):
        stem = Path(file_name).stem
        path = out_dir / f"{stem}-{group}-compare.csv"
        with path.open("w", newline="") as out:
            write_csv(groups.get((file_name, group), []), out)


def write_summary(rows, out):
    current = None
    for row in rows:
        key = (row["file"], row["group"])
        if key != current:
            current = key
            group_rows = [r for r in rows if (r["file"], r["group"]) == key]
            lost = sum(r["lost_passed"] for r in group_rows)
            out.write(f"\n## {key[0]} {key[1]}: {len(group_rows)} cases, lost_passed={lost}\n")
        out.write(
            "{case:20} {status:8} lost={lost_passed:<4} "
            "linux=pass:{linux_passed}/all:{linux_all} "
            "respos={respos_status}:ret{respos_ret}:pass{respos_passed}:"
            "fail{respos_failed}:brok{respos_broken}:skip{respos_skipped}:warn{respos_warnings}\n".format(
                **row
            )
        )


def main():
    parser = argparse.ArgumentParser(description="Compare Linux LTP baseline CSV with RespOS CSV.")
    parser.add_argument(
        "--baseline",
        type=Path,
        default=Path("judge/baseline/ltp-linux-baseline.csv"),
        help="Linux baseline CSV.",
    )
    parser.add_argument(
        "--respos",
        type=Path,
        default=Path("judge/ltp-local-report.csv"),
        help="RespOS report CSV.",
    )
    parser.add_argument(
        "--format",
        choices=("summary", "csv"),
        default="summary",
        help="Output format.",
    )
    parser.add_argument("-o", "--output", type=Path, help="Write output to this file.")
    parser.add_argument(
        "--split-dir",
        type=Path,
        help="Also write one compare CSV per input file and LTP group into this directory.",
    )
    parser.add_argument(
        "--case-list",
        type=Path,
        help="Only compare cases listed in this file. Blank lines and lines starting with # are ignored.",
    )
    args = parser.parse_args()

    case_filter = read_case_list(args.case_list) if args.case_list else None
    baseline_rows = read_csv(args.baseline)
    respos_rows = read_csv(args.respos)
    respos_keys = {(row["file"], row["group"]) for row in respos_rows}
    rows = compare(baseline_rows, respos_rows, case_filter)

    if args.split_dir:
        write_split_csvs(rows, args.split_dir, respos_keys)
        if not args.output:
            return

    out = args.output.open("w", newline="") if args.output else sys.stdout
    try:
        if args.format == "csv":
            write_csv(rows, out)
        else:
            write_summary(rows, out)
    finally:
        if args.output:
            out.close()


if __name__ == "__main__":
    main()
