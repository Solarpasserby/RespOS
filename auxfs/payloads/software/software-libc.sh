#!/bin/sh

set -u

WORK=/tmp/respos-software-libc
SOURCE=/respos/libc-combination.c

export HOME=/tmp/respos-software-libc-home
export TMPDIR=/tmp
export LC_ALL=C
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

rm -rf "${WORK}" "${HOME}"
mkdir -p "${WORK}" "${HOME}"

echo "SOFTWARE_LIBC BEGIN"
uname -a || true

gcc -std=c11 -O2 -Wall -Wextra -Werror -pthread "${SOURCE}" \
    -o "${WORK}/libc-combination" \
    > "${WORK}/compile.txt" 2>&1 || {
        echo "SOFTWARE_LIBC compile FAIL"
        cat "${WORK}/compile.txt"
        exit 1
    }

"${WORK}/libc-combination"
status=$?
if [ "${status}" -eq 0 ]; then
    echo "SOFTWARE_LIBC ALL PASS"
    exit 0
fi

echo "SOFTWARE_LIBC ALL FAIL status=${status}"
exit "${status}"
