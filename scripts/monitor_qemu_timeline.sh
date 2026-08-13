#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: $0 PID SERIAL_LOG OUTPUT_DIR [INTERVAL_SECONDS]" >&2
    exit 2
}

[[ $# -ge 3 && $# -le 4 ]] || usage

pid=$1
serial_log=$2
output_dir=$3
interval=${4:-1}
clock_ticks=$(getconf CLK_TCK)
page_size=$(getconf PAGESIZE)

[[ $pid =~ ^[0-9]+$ ]] || usage
[[ $interval =~ ^[0-9]+([.][0-9]+)?$ ]] || usage
[[ -r /proc/$pid/stat ]] || {
    echo "process $pid is not running" >&2
    exit 1
}

mkdir -p "$output_dir"
samples="$output_dir/host-samples.csv"
threads="$output_dir/host-threads.csv"
events="$output_dir/serial-events.csv"
system_samples="$output_dir/host-system.csv"
metadata="$output_dir/metadata.txt"

{
    echo "monitor_pid=$pid"
    echo "serial_log=$serial_log"
    echo "interval_seconds=$interval"
    echo "clock_ticks=$clock_ticks"
    echo "page_size=$page_size"
    echo "host_cpus=$(nproc)"
    echo "started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    if [[ -r /proc/$pid/cmdline ]]; then
        tr '\0' ' ' < "/proc/$pid/cmdline"
        echo
    fi
} > "$metadata"

echo "monotonic_s,pid,cpu_percent,rss_kib,nlwp,stat,psr" > "$samples"
echo "monotonic_s,pid,tid,comm,cpu_percent,rss_kib,stat,psr" > "$threads"
echo "monotonic_s,marker,line" > "$events"
echo "monotonic_s,mem_available_kib,swap_free_kib,pswpin_pages,pswpout_pages,pgmajfault,load1" > "$system_samples"

declare -A previous_ticks=()
declare -A previous_time=()

interval_cpu_percent() {
    local sample_id=$1 ticks=$2 now=$3 old_ticks old_time
    old_ticks=${previous_ticks[$sample_id]:-}
    old_time=${previous_time[$sample_id]:-}
    previous_ticks[$sample_id]=$ticks
    previous_time[$sample_id]=$now
    if [[ -z $old_ticks || -z $old_time ]]; then
        interval_cpu_result=0.000
        return
    fi
    interval_cpu_result=$(awk -v ticks="$ticks" -v old_ticks="$old_ticks" -v now="$now" \
        -v old_time="$old_time" -v hz="$clock_ticks" \
        'BEGIN {
            elapsed = now - old_time;
            if (elapsed <= 0 || ticks < old_ticks) print "0.000";
            else printf "%.3f", (ticks - old_ticks) * 100 / hz / elapsed;
        }')
}

read_task_stat() {
    local task_stat=$1 stat_line rest
    [[ -r $task_stat ]] || return 1
    IFS= read -r stat_line < "$task_stat" || return 1
    # comm may contain spaces and parentheses. Everything after the final ") "
    # has stable proc_pid_stat field positions: state is 1, utime 12,
    # stime 13, num_threads 18, rss 22 and processor 37.
    rest=${stat_line##*) }
    awk '{print $12 + $13, $1, $18, $22, $37}' <<< "$rest"
}

process_serial_events() {
    local now=$1 line_count
    [[ -r $serial_log ]] || return
    line_count=$(wc -l < "$serial_log")
    if (( line_count <= last_line )); then
        return
    fi
    sed -n "$((last_line + 1)),${line_count}p" "$serial_log" |
        awk -v now="$now" '
            /OS COMP TEST GROUP START/ {marker="group_start"}
            /OS COMP TEST GROUP END/ {marker="group_end"}
            /BUILDSTORM_BEGIN/ {marker="buildstorm_begin"}
            /BUILDSTORM_COMPILE/ {marker="buildstorm_compile"}
            /Finished `dev` profile/ {marker="cargo_dev_finished"}
            /Finished `release` profile/ {marker="cargo_release_finished"}
            /Compiling compiler_builtins/ {marker="compile_core_begin"}
            /Compiling arceos-helloworld/ {marker="compile_app_begin"}
            marker != "" {
                line=$0; gsub(/"/, "\"\"", line);
                print now "," marker ",\"" line "\"";
                marker="";
            }
        ' >> "$events"
    last_line=$line_count
}

last_line=0
while [[ -r /proc/$pid/stat ]]; do
    now=$(cut -d' ' -f1 /proc/uptime)
    if read -r ticks state nlwp rss_pages psr < <(read_task_stat "/proc/$pid/stat"); then
        interval_cpu_percent "p:$pid" "$ticks" "$now"
        cpu_percent=$interval_cpu_result
        rss_kib=$((rss_pages * page_size / 1024))
        printf '%s,%s,%s,%s,%s,%s,%s\n' \
            "$now" "$pid" "$cpu_percent" "$rss_kib" "$nlwp" "$state" "$psr" >> "$samples"
    fi

    for task_dir in /proc/"$pid"/task/[0-9]*; do
        [[ -r $task_dir/stat ]] || continue
        tid=${task_dir##*/}
        read -r ticks state _ rss_pages psr < <(read_task_stat "$task_dir/stat") || continue
        interval_cpu_percent "t:$tid" "$ticks" "$now"
        cpu_percent=$interval_cpu_result
        comm=$(<"$task_dir/comm")
        comm=${comm//,/_}
        rss_kib=$((rss_pages * page_size / 1024))
        printf '%s,%s,%s,%s,%s,%s,%s,%s\n' \
            "$now" "$pid" "$tid" "$comm" "$cpu_percent" "$rss_kib" "$state" "$psr" >> "$threads"
    done

    read -r mem_available swap_free < <(
        awk '/^MemAvailable:/ {mem=$2} /^SwapFree:/ {swap=$2} END {print mem+0, swap+0}' /proc/meminfo
    )
    read -r pswpin pswpout pgmajfault < <(
        awk '/^pswpin / {pin=$2} /^pswpout / {pout=$2} /^pgmajfault / {major=$2} END {print pin+0, pout+0, major+0}' /proc/vmstat
    )
    read -r load1 _ < /proc/loadavg
    printf '%s,%s,%s,%s,%s,%s,%s\n' \
        "$now" "$mem_available" "$swap_free" "$pswpin" "$pswpout" "$pgmajfault" "$load1" \
        >> "$system_samples"

    process_serial_events "$now"
    sleep "$interval"
done

# QEMU can write final group/perf markers immediately before exit, after the
# last process sample. Drain them once more so the event CSV closes cleanly.
process_serial_events "$(cut -d' ' -f1 /proc/uptime)"
echo "finished_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$metadata"
