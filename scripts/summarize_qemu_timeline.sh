#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 TIMELINE_DIR" >&2
    exit 2
fi

timeline_dir=$1
samples=$timeline_dir/host-samples.csv
threads=$timeline_dir/host-threads.csv
events=$timeline_dir/serial-events.csv
system_samples=$timeline_dir/host-system.csv

[[ -r $samples && -r $threads && -r $events ]] || {
    echo "timeline directory is missing required CSV files: $timeline_dir" >&2
    exit 1
}

echo "[host]"
awk -F, '
    NR == 1 { next }
    {
        if (count > 0) {
            dt = $1 - previous_time;
            if (dt > 0) {
                measured += dt;
                core_seconds += ($3 / 100) * dt;
                if ($3 < 400) below_4 += dt;
                if ($3 < 800) below_8 += dt;
            }
        }
        if ($3 > peak_cpu) peak_cpu = $3;
        if ($4 > peak_rss) peak_rss = $4;
        previous_time = $1;
        count++;
    }
    END {
        average = measured > 0 ? core_seconds * 100 / measured : 0;
        low4 = measured > 0 ? below_4 * 100 / measured : 0;
        low8 = measured > 0 ? below_8 * 100 / measured : 0;
        printf "samples=%d measured_seconds=%.3f cpu_core_seconds=%.3f average_cpu_percent=%.1f peak_cpu_percent=%.1f below_400_percent=%.1f below_800_percent=%.1f peak_rss_kib=%d\n", count, measured, core_seconds, average, peak_cpu, low4, low8, peak_rss;
    }
' "$samples"

if [[ -r $system_samples ]]; then
    echo "[host-memory]"
    awk -F, '
        NR == 1 { next }
        NR == 2 { first_pin=$4; first_pout=$5; first_major=$6 }
        {
            if (minimum_available == 0 || $2 < minimum_available) minimum_available=$2;
            if ($3 < minimum_swap || minimum_swap == 0) minimum_swap=$3;
            if ($7 > maximum_load) maximum_load=$7;
            last_pin=$4; last_pout=$5; last_major=$6; count++;
        }
        END {
            printf "samples=%d minimum_mem_available_kib=%d minimum_swap_free_kib=%d pswpin_delta=%d pswpout_delta=%d pgmajfault_delta=%d maximum_load1=%.2f\n", count, minimum_available, minimum_swap, last_pin-first_pin, last_pout-first_pout, last_major-first_major, maximum_load;
        }
    ' "$system_samples"
fi

echo "[threads-by-name]"
awk -F, '
    NR == 1 { next }
    {
        key = $3 SUBSEP $4;
        if (key in last_time) {
            dt = $1 - last_time[key];
            if (dt > 0) core_seconds[$4] += ($5 / 100) * dt;
        }
        last_time[key] = $1;
        if ($5 > peak[$4]) peak[$4] = $5;
    }
    END {
        for (name in core_seconds)
            printf "%s core_seconds=%.3f peak_cpu_percent=%.1f\n", name, core_seconds[name], peak[name];
    }
' "$threads" | sort -t= -k2,2nr

echo "[serial-events]"
if [[ $(wc -l < "$events") -le 1 ]]; then
    echo "none"
else
    tail -n +2 "$events"
fi
