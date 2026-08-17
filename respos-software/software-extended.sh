#!/bin/sh

set -u

WORK=/tmp/respos-software-extended
HOME_DIR=/tmp/respos-software-extended-home
failures=0

export HOME="${HOME_DIR}"
export TMPDIR=/tmp
export TERM=xterm
export LC_ALL=C
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

pass() {
    echo "SOFTWARE_EXTENDED $1 PASS"
}

fail() {
    echo "SOFTWARE_EXTENDED $1 FAIL"
    failures=$((failures + 1))
}

rm -rf "${WORK}" "${HOME_DIR}"
mkdir -p "${WORK}" "${HOME_DIR}"

echo "SOFTWARE_EXTENDED BEGIN"
uname -a || true

git_ok=1
GIT_WORK="${WORK}/git-repack"
mkdir -p "${GIT_WORK}" || git_ok=0
cd "${GIT_WORK}" || git_ok=0
/usr/bin/git init . || git_ok=0
/usr/bin/git config user.name RespOS || git_ok=0
/usr/bin/git config user.email respos@example.invalid || git_ok=0

i=0
while [ "${i}" -lt 64 ]; do
    printf 'base object %03d\n' "${i}" > "object-${i}.txt" || git_ok=0
    i=$((i + 1))
done
/usr/bin/git add . || git_ok=0
/usr/bin/git commit -m 'add base objects' || git_ok=0
base_branch=$(/usr/bin/git symbolic-ref --short HEAD) || git_ok=0

/usr/bin/git checkout -b feature || git_ok=0
i=0
while [ "${i}" -lt 32 ]; do
    printf 'feature update %03d\n' "${i}" >> "object-${i}.txt" || git_ok=0
    i=$((i + 1))
done
/usr/bin/git add . || git_ok=0
/usr/bin/git commit -m 'update feature objects' || git_ok=0

/usr/bin/git checkout "${base_branch}" || git_ok=0
printf 'main branch\n' > main-only.txt || git_ok=0
/usr/bin/git add main-only.txt || git_ok=0
/usr/bin/git commit -m 'add main object' || git_ok=0
/usr/bin/git merge --no-ff -m 'merge feature' feature || git_ok=0
/usr/bin/git branch -d feature || git_ok=0

/usr/bin/git gc --prune=now || git_ok=0
/usr/bin/git repack -a -d || git_ok=0
/usr/bin/git fsck --full --strict || git_ok=0
/usr/bin/git count-objects -v > "${WORK}/git-count.txt" || git_ok=0
grep -Eq '^packs: [1-9][0-9]*$' "${WORK}/git-count.txt" || git_ok=0
/usr/bin/git status --porcelain > "${WORK}/git-status.txt" || git_ok=0
test ! -s "${WORK}/git-status.txt" || git_ok=0
commit_count=$(/usr/bin/git rev-list --count HEAD) || git_ok=0
test "${commit_count}" -eq 4 || git_ok=0
if [ "${git_ok}" -eq 1 ]; then pass git_repack_fsck; else fail git_repack_fsck; fi

vim_ok=1
VIM_WORK="${WORK}/vim-recovery"
mkdir -p "${VIM_WORK}" || vim_ok=0
cd "${VIM_WORK}" || vim_ok=0
printf 'before crash\n' > recovery.txt || vim_ok=0
rm -f .recovery.txt.swp
rm -f "${WORK}/vim-swap-ready"
/usr/bin/vim -Nu NONE -i NONE -es \
    -c 'set directory=.' \
    -c 'set swapfile' \
    -c 'set updatecount=1' \
    -c 'call setline(1, "RECOVERED_FROM_SWAP")' \
    -c 'preserve' \
    -c 'call writefile(["ready"], "'"${WORK}"'/vim-swap-ready")' \
    -c 'sleep 10' recovery.txt > "${WORK}/vim-crash.txt" 2>&1 &
vim_pid=$!

i=0
while [ "${i}" -lt 100 ] && [ ! -s "${WORK}/vim-swap-ready" ]; do
    sleep 0.02
    i=$((i + 1))
done
test -s .recovery.txt.swp || vim_ok=0
test -s "${WORK}/vim-swap-ready" || vim_ok=0
kill -KILL "${vim_pid}" 2>/dev/null || vim_ok=0
wait "${vim_pid}" 2>/dev/null || true

/usr/bin/vim -Nu NONE -i NONE -n -es -r recovery.txt \
    -c 'set noswapfile' \
    -c 'wq!' > "${WORK}/vim-recover.txt" 2>&1 || vim_ok=0
grep -qx RECOVERED_FROM_SWAP recovery.txt || vim_ok=0
rm -f .recovery.txt.swp
if [ "${vim_ok}" -eq 1 ]; then pass vim_swap_recovery; else fail vim_swap_recovery; fi

make_ok=1
MAKE_WORK="${WORK}/make-parallel"
mkdir -p "${MAKE_WORK}" || make_ok=0
cd "${MAKE_WORK}" || make_ok=0
printf '%s\n' \
    'int add(int left, int right) { return left + right; }' > add.c || make_ok=0
printf '%s\n' \
    'int scale(int value) { return value * 2; }' > scale.c || make_ok=0
printf '%s\n' \
    '#include <stdio.h>' \
    'int add(int, int);' \
    'int scale(int);' \
    'int main(void) {' \
    '    printf("RESPOS_MAKE_%d\n", scale(add(19, 2)));' \
    '    return 0;' \
    '}' > main.c || make_ok=0
printf '%s\n' \
    'CC ?= gcc' \
    'AR ?= ar' \
    'CFLAGS = -O2 -Wall -Wextra -Werror' \
    'all: app' \
    'app: main.o libcalc.a' \
    '	$(CC) $(CFLAGS) -o $@ main.o libcalc.a' \
    'libcalc.a: add.o scale.o' \
    '	$(AR) rcs $@ add.o scale.o' \
    '%.o: %.c' \
    '	$(CC) $(CFLAGS) -c -o $@ $<' \
    'clean:' \
    '	rm -f app *.o libcalc.a' > Makefile || make_ok=0
/usr/bin/make -j2 > "${WORK}/make-first.txt" 2>&1 || make_ok=0
./app > "${WORK}/make-output.txt" || make_ok=0
grep -qx RESPOS_MAKE_42 "${WORK}/make-output.txt" || make_ok=0
/usr/bin/make clean > "${WORK}/make-clean.txt" 2>&1 || make_ok=0
/usr/bin/make -j2 > "${WORK}/make-second.txt" 2>&1 || make_ok=0
./app > "${WORK}/make-output-second.txt" || make_ok=0
grep -qx RESPOS_MAKE_42 "${WORK}/make-output-second.txt" || make_ok=0
if [ "${make_ok}" -eq 1 ]; then pass make_parallel_static; else fail make_parallel_static; fi

cargo_ok=1
CARGO_WORK="${WORK}/cargo-workspace"
mkdir -p "${CARGO_WORK}/helper/src" "${CARGO_WORK}/app/src" || cargo_ok=0
cd "${CARGO_WORK}" || cargo_ok=0
printf '%s\n' \
    '[workspace]' \
    'members = ["helper", "app"]' \
    'resolver = "2"' > Cargo.toml || cargo_ok=0
printf '%s\n' \
    '[package]' \
    'name = "helper"' \
    'version = "0.1.0"' \
    'edition = "2021"' > helper/Cargo.toml || cargo_ok=0
printf '%s\n' \
    'pub fn message() -> &'\''static str { "RESPOS_CARGO_FIRST" }' > helper/src/lib.rs || cargo_ok=0
printf '%s\n' \
    '[package]' \
    'name = "respos-cargo-app"' \
    'version = "0.1.0"' \
    'edition = "2021"' \
    '' \
    '[dependencies]' \
    'helper = { path = "../helper" }' > app/Cargo.toml || cargo_ok=0
printf '%s\n' \
    'fn main() { println!("{}", helper::message()); }' > app/src/main.rs || cargo_ok=0
CARGO_BUILD_JOBS=2 /usr/bin/cargo build --offline --release \
    > "${WORK}/cargo-first.txt" 2>&1 || cargo_ok=0
./target/release/respos-cargo-app > "${WORK}/cargo-output.txt" || cargo_ok=0
grep -qx RESPOS_CARGO_FIRST "${WORK}/cargo-output.txt" || cargo_ok=0

printf '%s\n' \
    'pub fn message() -> &'\''static str { "RESPOS_CARGO_SECOND" }' > helper/src/lib.rs || cargo_ok=0
CARGO_BUILD_JOBS=2 /usr/bin/cargo build --offline --release \
    > "${WORK}/cargo-second.txt" 2>&1 || cargo_ok=0
./target/release/respos-cargo-app > "${WORK}/cargo-output-second.txt" || cargo_ok=0
grep -qx RESPOS_CARGO_SECOND "${WORK}/cargo-output-second.txt" || cargo_ok=0
if [ "${cargo_ok}" -eq 1 ]; then pass cargo_offline_workspace; else fail cargo_offline_workspace; fi

if [ "${failures}" -eq 0 ]; then
    echo "SOFTWARE_EXTENDED ALL PASS"
    exit 0
fi

echo "SOFTWARE_EXTENDED ALL FAIL failures=${failures}"
exit 1
