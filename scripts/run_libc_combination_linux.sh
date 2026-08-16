#!/bin/sh

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WORK=$(mktemp -d /tmp/respos-libc-linux.XXXXXX)
trap 'rm -rf "${WORK}"' EXIT HUP INT TERM

${CC:-cc} -std=c11 -O2 -Wall -Wextra -Werror -pthread \
    "${ROOT}/respos-software/libc-combination.c" \
    -o "${WORK}/libc-combination"
"${WORK}/libc-combination"
