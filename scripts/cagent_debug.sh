#!/bin/bash
# Diagnostic runner for the final-2026 CAgent tests.
#
# Run this script from the directory containing agent_lite,
# simple_llm_server, and busybox (normally /glibc in the pub image).
# Unlike the official runner, all diagnostic files are retained in /tmp.

set -u

BUSYBOX="${BUSYBOX:-./busybox}"
AGENT="${AGENT:-./agent_lite}"
SERVER="${SERVER:-./simple_llm_server}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8080}"
SELECTED_TEST="${1:-all}"
RUN_ID="${CAGENT_RUN_ID:-$(date +%s)_$$}"
LOG_DIR="/tmp/cagent_debug_${RUN_ID}"

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
    if [ "$agent_rc" -eq 0 ] && [ "$validation_rc" -eq 0 ]; then
        printf 'testcase cagent %s pass %s\n' "$test_name" "$duration"
    else
        printf 'testcase cagent %s reject %s\n' "$test_name" "$duration"
    fi
}

"$SERVER" "$PORT" >"$LOG_DIR/server.log" 2>&1 &
SERVER_PID=$!
"$BUSYBOX" sleep 1

TESTS="factorial date network cpu kernel fs-create fs-readwrite fs-directory fs-search fs-usage"
if [ "$SELECTED_TEST" = all ]; then
    TEST_PIDS=""
    for test_name in $TESTS; do
        run_test "$test_name" &
        TEST_PIDS="$TEST_PIDS $!"
    done
    for test_pid in $TEST_PIDS; do
        wait "$test_pid"
    done
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
