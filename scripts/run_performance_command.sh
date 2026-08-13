#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 COMMAND [ARG ...]" >&2
    exit 2
fi

# Codex or an interactive shell may itself run with SCHED_IDLE and a positive
# nice value. Performance runs need absolute values, not `nice -n -10`'s
# relative adjustment.
/usr/bin/chrt --other --pid 0 $$
/usr/bin/renice -n -10 -p $$ >/dev/null

if [ -n "${RESPOS_PERF_SERIAL_LOG:-}" ] || [ -n "${RESPOS_PERF_TIMELINE_DIR:-}" ]; then
    if [ -z "${RESPOS_PERF_SERIAL_LOG:-}" ] || [ -z "${RESPOS_PERF_TIMELINE_DIR:-}" ]; then
        echo "RESPOS_PERF_SERIAL_LOG and RESPOS_PERF_TIMELINE_DIR must be set together" >&2
        exit 2
    fi
    script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
    (
        # Let exec replace this shell first, so metadata records the real QEMU
        # command while retaining the same PID.
        sleep 0.1
        exec "$script_dir/monitor_qemu_timeline.sh" \
            "$$" "$RESPOS_PERF_SERIAL_LOG" "$RESPOS_PERF_TIMELINE_DIR" \
            "${RESPOS_PERF_INTERVAL:-1}"
    ) &
fi

exec "$@"
