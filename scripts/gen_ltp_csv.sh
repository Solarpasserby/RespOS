#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

rv_log="${1:-${RV_OUTPUT:-rv-output.txt}}"
la_log="${2:-${LA_OUTPUT:-la-output.txt}}"
out_dir="${LTP_CSV_DIR:-judge}"
report_dir="${LTP_REPORT_DIR:-${LTP_SPLIT_DIR:-$out_dir/local-report}}"
compare_dir="${LTP_COMPARE_DIR:-$out_dir/local-compare}"
baseline_csv="${LTP_BASELINE_CSV:-judge/baseline/ltp-linux-baseline.csv}"
case_list="${LTP_CASE_LIST-user/oscomp_ltp_list.txt}"

mkdir -p "$out_dir" "$report_dir" "$compare_dir"
rm -f "$report_dir"/*.csv "$compare_dir"/*.csv
if [[ "$report_dir" != "$out_dir/local-split" && -d "$out_dir/local-split" ]]; then
    rm -f "$out_dir/local-split"/*.csv
fi
rm -f "$out_dir/ltp-local-report.csv" "$out_dir/ltp-local-report.md"
rm -f "$out_dir/ltp-compare.csv" "$out_dir/ltp-compare-summary.txt"

tmp_report="$(mktemp "${TMPDIR:-/tmp}/ltp-local-report.XXXXXX.csv")"
trap 'rm -f "$tmp_report"' EXIT

python3 judge/ltp_report.py "$rv_log" "$la_log" \
    --format csv \
    --output "$tmp_report" \
    --split-dir "$report_dir"

if [[ -f "$baseline_csv" ]]; then
    compare_args=(
        --baseline "$baseline_csv"
        --respos "$tmp_report"
        --split-dir "$compare_dir"
    )
    if [[ -n "$case_list" ]]; then
        if [[ -f "$case_list" ]]; then
            compare_args+=(--case-list "$case_list")
        else
            echo "skip case filter: case list not found: $case_list" >&2
        fi
    fi

    python3 judge/ltp_compare.py \
        "${compare_args[@]}"
else
    echo "skip compare: baseline CSV not found: $baseline_csv" >&2
fi

echo "generated:"
find "$report_dir" -maxdepth 1 -type f -name '*.csv' -print | sort | sed 's/^/  /'
if [[ -f "$baseline_csv" ]]; then
    find "$compare_dir" -maxdepth 1 -type f -name '*.csv' -print | sort | sed 's/^/  /'
fi
