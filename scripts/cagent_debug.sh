#!/bin/bash
# Diagnostic runner for the final-2026 CAgent tests.
#
# Run this script from the directory containing agent_lite,
# simple_llm_server, and busybox (normally /glibc in the pub image).
# Unlike the official runner, all diagnostic files are retained in /tmp.

set -u

CAGENT_DIR="${CAGENT_DIR:-/glibc}"
BUSYBOX="${BUSYBOX:-$CAGENT_DIR/busybox}"
AGENT="${AGENT:-$CAGENT_DIR/agent_lite}"
SERVER="${SERVER:-$CAGENT_DIR/simple_llm_server}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8080}"
SELECTED_TEST="${1:-all}"
RUN_ID="${CAGENT_RUN_ID:-$(date +%s)_$$}"
LOG_DIR="/tmp/cagent_debug_${RUN_ID}"

cd "$CAGENT_DIR" || exit 1
mkdir -p "$LOG_DIR" || exit 1

SERVER_PID=""
cleanup_server() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup_server EXIT INT TERM

case_data() {
    case "$1" in
        factorial)
            PROMPT="Calculate factorial of 10 using bash"
            VALIDATION="grep -q '3628800'"
            COMMAND="echo 3628800"
            TIMEOUT=20
            ;;
        date)
            PROMPT="What day was it 100 days ago?"
            VALIDATION="grep -qE '(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)'"
            COMMAND="date -d '100 days ago' '+%A, %B %d, %Y'"
            TIMEOUT=20
            ;;
        network)
            PROMPT="Count ESTABLISHED TCP connections"
            VALIDATION="grep -qE '[0-9]+'"
            COMMAND="ss -tan | grep ESTAB | wc -l"
            TIMEOUT=25
            ;;
        cpu)
            PROMPT="How many CPU cores?"
            VALIDATION="grep -qE '[0-9]+'"
            COMMAND="nproc"
            TIMEOUT=20
            ;;
        kernel)
            PROMPT="What is the kernel version?"
            VALIDATION="grep -qE '[0-9]+\.[0-9]+'"
            COMMAND="uname -r"
            TIMEOUT=20
            ;;
        fs-create)
            PROMPT="Create a file named test_file.txt with content 'Hello OS'"
            VALIDATION="test -f test_file.txt && grep -q 'Hello OS' test_file.txt"
            COMMAND="printf 'Hello OS\\n' > test_file.txt"
            TIMEOUT=25
            ;;
        fs-readwrite)
            PROMPT="Create test_input.txt with numbers 1 to 5, then read it and sum the numbers"
            VALIDATION="grep -qE '15|fifteen'"
            COMMAND="printf '1\\n2\\n3\\n4\\n5\\n' > test_input.txt && awk '{sum += \$1} END {print sum}' test_input.txt"
            TIMEOUT=30
            ;;
        fs-directory)
            PROMPT="Create directory test_dir, create 3 files inside it, then count the files"
            VALIDATION="test -d test_dir && [ \$(ls test_dir | wc -l) -ge 3 ]"
            COMMAND="mkdir -p test_dir && touch test_dir/file1 test_dir/file2 test_dir/file3 && ls test_dir | wc -l"
            TIMEOUT=30
            ;;
        fs-search)
            PROMPT="Find all .sh files in current directory and count them"
            VALIDATION="grep -qE '[0-9]+'"
            COMMAND="find . -name '*.sh' | wc -l"
            TIMEOUT=35
            ;;
        fs-usage)
            PROMPT="Check disk usage of current directory in human readable format"
            VALIDATION="grep -qE '[0-9]+[KMG]?'"
            COMMAND="df -h / | awk 'NR==2 {print \$5}'"
            TIMEOUT=25
            ;;
        relative-exec)
            # Diagnostic only: distinguish AT_FDCWD/relative exec lookup from
            # the command, server, and agent layers.
            PROMPT="Run uname using a relative executable path"
            VALIDATION="grep -qE '[0-9]+\\.[0-9]+'"
            COMMAND="cd /glibc && ./busybox uname -r"
            TIMEOUT=20
            ;;
        *)
            return 1
            ;;
    esac
}

run_test() {
    local test_name="$1"
    local prefix="$LOG_DIR/$test_name"
    local start_time end_time duration agent_rc validation_rc probe_rc

    case_data "$test_name" || return 2
    printf '%s\n' "$PROMPT" >"$prefix.prompt"
    printf '%s\n' "$COMMAND" >"$prefix.command"
    printf '%s\n' "$VALIDATION" >"$prefix.validation"

    # Execute the fixed command separately so stdout, stderr, and the raw shell
    # exit code remain observable even when popen/pclose is the failing layer.
    "$BUSYBOX" sh -c "$COMMAND" >"$prefix.command.stdout" 2>"$prefix.command.stderr"
    probe_rc=$?
    printf '%s\n' "$probe_rc" >"$prefix.command.exit"

    start_time=$(date +%s%3N)
    "$BUSYBOX" timeout "${TIMEOUT}s" "$AGENT" \
        --workspace . --host "$HOST" --port "$PORT" "$PROMPT" \
        >"$prefix.agent.log" 2>&1
    agent_rc=$?
    printf '%s\n' "$agent_rc" >"$prefix.agent.exit"

    "$BUSYBOX" sh -c "$VALIDATION" <"$prefix.agent.log"
    validation_rc=$?
    printf '%s\n' "$validation_rc" >"$prefix.validation.exit"

    end_time=$(date +%s%3N)
    duration=$((end_time - start_time))
    printf '%s\n' "$duration" >"$prefix.duration_ms"
    if [ "$agent_rc" -eq 0 ] && [ "$validation_rc" -eq 0 ]; then
        printf 'testcase cagent %s pass %s\n' "$test_name" "$duration"
    else
        printf 'testcase cagent %s reject %s\n' "$test_name" "$duration"
    fi
}

"$SERVER" "$PORT" >"$LOG_DIR/server.log" 2>&1 &
SERVER_PID=$!
"$BUSYBOX" sleep 1

# Minimal signal-interrupt probe for a server blocked in accept(2).  It avoids
# agents entirely so a missing completion marker identifies the socket/signal
# layer rather than CAgent scheduling or validation.
if [ "$SELECTED_TEST" = server-interrupt ]; then
    printf '%s\n' "$(date +%s%3N)" >"$LOG_DIR/server_interrupt.kill_started_ms"
    kill "$SERVER_PID"
    printf '%s\n' "$?" >"$LOG_DIR/server_interrupt.kill.exit"
    wait "$SERVER_PID"
    printf '%s\n' "$?" >"$LOG_DIR/server_interrupt.wait.exit"
    printf '%s\n' "$(date +%s%3N)" >"$LOG_DIR/server_interrupt.completed_ms"
    SERVER_PID=""
    exit 0
fi

# Hold an accepted connection open without sending an HTTP request, then stop
# the server. This distinguishes interrupting accept(2) from interrupting the
# read(2) inside handle_client(). Diagnostic only.
if [ "$SELECTED_TEST" = server-read-interrupt ]; then
    # Keep nc's stdin open through a FIFO.  A background shell pipeline would
    # leave its sleep(1) child outside CLIENT_PID, making the diagnostic's own
    # cleanup look like a socket/signal hang.
    READ_PROBE_FIFO="$LOG_DIR/read_probe_fifo"
    "$BUSYBOX" mkfifo "$READ_PROBE_FIFO" || exit 1
    "$BUSYBOX" nc "$HOST" "$PORT" <"$READ_PROBE_FIFO" >"$LOG_DIR/read_probe_client.log" 2>&1 &
    CLIENT_PID=$!
    exec 3>"$READ_PROBE_FIFO"
    "$BUSYBOX" sleep 1
    printf '%s\n' "$(date +%s%3N)" >"$LOG_DIR/server_read_interrupt.kill_started_ms"
    kill "$SERVER_PID"
    printf '%s\n' "$?" >"$LOG_DIR/server_read_interrupt.kill.exit"
    wait "$SERVER_PID"
    printf '%s\n' "$?" >"$LOG_DIR/server_read_interrupt.wait.exit"
    exec 3>&-
    wait "$CLIENT_PID" 2>/dev/null || true
    printf '%s\n' "$(date +%s%3N)" >"$LOG_DIR/server_read_interrupt.completed_ms"
    SERVER_PID=""
    exit 0
fi

if [ "$SELECTED_TEST" = timeout-storm ]; then
    TIMEOUT_STORM_COUNT="${TIMEOUT_STORM_COUNT:-10}"
    timeout_probe() {
        local index="$1" start end rc
        start=$(date +%s%3N)
        "$BUSYBOX" timeout 3 "$BUSYBOX" sleep 60
        rc=$?
        end=$(date +%s%3N)
        printf '%s\n' "$rc" >"$LOG_DIR/timeout_storm_$index.exit"
        printf '%s\n' "$((end - start))" >"$LOG_DIR/timeout_storm_$index.duration_ms"
    }
    PIDS=""
    index=1
    while [ "$index" -le "$TIMEOUT_STORM_COUNT" ]; do
        timeout_probe "$index" &
        PIDS="$PIDS $!"
        index=$((index + 1))
    done
    for pid in $PIDS; do
        wait "$pid"
    done
    exit 0
fi

DEFAULT_TESTS="factorial date network cpu kernel fs-create fs-readwrite fs-directory fs-search fs-usage"
# Optional space-separated subset for controlled concurrency experiments.  Keep
# the default identical to the official ten-agent shape.
TESTS="${CAGENT_TESTS:-$DEFAULT_TESTS}"
# The guest's minimal Rust shell tokenizes on spaces and has no quoting or
# environment-assignment syntax.  Accept commas too, so an injected command
# can select a subset through BusyBox `env` without changing test contents.
TESTS="${TESTS//,/ }"
if [ "$SELECTED_TEST" = all ]; then
    if [ "${CAGENT_SERIAL:-0}" = 1 ]; then
        for test_name in $TESTS; do
            run_test "$test_name"
        done
    else
        TEST_PIDS=""
        for test_name in $TESTS; do
            run_test "$test_name" &
            TEST_PIDS="$TEST_PIDS $!"
        done
        for test_pid in $TEST_PIDS; do
            wait "$test_pid"
        done
    fi
else
    if ! case_data "$SELECTED_TEST"; then
        echo "unknown CAgent test: $SELECTED_TEST" >&2
        exit 2
    fi
    run_test "$SELECTED_TEST"
fi

cleanup_server
SERVER_PID=""
printf 'CAgent debug logs retained in %s\n' "$LOG_DIR"
