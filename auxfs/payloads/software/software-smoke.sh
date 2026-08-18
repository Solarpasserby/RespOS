#!/bin/sh

set -u

WORK=/tmp/respos-software-smoke
HOME_DIR=/tmp/respos-software-home
failures=0

export HOME="${HOME_DIR}"
export TMPDIR=/tmp
export TERM=xterm
export LC_ALL=C
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

pass() {
    echo "SOFTWARE_COMPAT $1 PASS"
}

fail() {
    echo "SOFTWARE_COMPAT $1 FAIL"
    failures=$((failures + 1))
}

rm -rf "${WORK}" "${HOME_DIR}"
mkdir -p "${WORK}" "${HOME_DIR}"

echo "SOFTWARE_COMPAT BEGIN"
uname -a || true

git_ok=1
/usr/bin/git --version || git_ok=0
/usr/bin/git -h > "${WORK}/git-help.txt" || git_ok=0
test -s "${WORK}/git-help.txt" || git_ok=0
mkdir -p "${WORK}/git-repo" || git_ok=0
cd "${WORK}/git-repo" || git_ok=0
/usr/bin/git init . || git_ok=0
/usr/bin/git config user.name RespOS || git_ok=0
/usr/bin/git config user.email respos@example.invalid || git_ok=0
printf 'software compatibility\n' > README.md || git_ok=0
/usr/bin/git add . || git_ok=0
/usr/bin/git commit -m 'add README.md' || git_ok=0
/usr/bin/git status --porcelain > "${WORK}/git-status.txt" || git_ok=0
test ! -s "${WORK}/git-status.txt" || git_ok=0
/usr/bin/git log -1 --format=%s > "${WORK}/git-subject.txt" || git_ok=0
grep -qx 'add README.md' "${WORK}/git-subject.txt" || git_ok=0
if [ "${git_ok}" -eq 1 ]; then pass git_local; else fail git_local; fi

vim_ok=1
cd "${WORK}" || vim_ok=0
/usr/bin/vim -h > "${WORK}/vim-help.txt" || vim_ok=0
test -s "${WORK}/vim-help.txt" || vim_ok=0
/usr/bin/vim --version > "${WORK}/vim-version.txt" || vim_ok=0
head -n 1 "${WORK}/vim-version.txt" || vim_ok=0
printf 'before\n' > hello.c || vim_ok=0
/usr/bin/vim -Nu NONE -n -es \
    -c 'set noswapfile' \
    -c '%s/before/after/' \
    -c 'wq' hello.c || vim_ok=0
grep -qx after hello.c || vim_ok=0
if [ "${vim_ok}" -eq 1 ]; then pass vim_batch; else fail vim_batch; fi

gcc_ok=1
cd "${WORK}" || gcc_ok=0
/usr/bin/gcc --help > "${WORK}/gcc-help.txt" || gcc_ok=0
test -s "${WORK}/gcc-help.txt" || gcc_ok=0
/usr/bin/gcc --version || gcc_ok=0
printf '#include <stdio.h>\nint main(void) { puts("RESPOS_C_HELLO"); return 0; }\n' > helloworld.c || gcc_ok=0
/usr/bin/gcc helloworld.c || gcc_ok=0
./a.out > c-output.txt || gcc_ok=0
grep -qx RESPOS_C_HELLO c-output.txt || gcc_ok=0
if [ "${gcc_ok}" -eq 1 ]; then pass gcc_compile_run; else fail gcc_compile_run; fi

rust_ok=1
cd "${WORK}" || rust_ok=0
/usr/bin/rustc -h > "${WORK}/rustc-help.txt" || rust_ok=0
test -s "${WORK}/rustc-help.txt" || rust_ok=0
/usr/bin/rustc --version || rust_ok=0
printf 'fn main() { println!("RESPOS_RUST_HELLO"); }\n' > helloworld.rs || rust_ok=0
/usr/bin/rustc helloworld.rs || rust_ok=0
./helloworld > rust-output.txt || rust_ok=0
grep -qx RESPOS_RUST_HELLO rust-output.txt || rust_ok=0
if [ "${rust_ok}" -eq 1 ]; then pass rustc_compile_run; else fail rustc_compile_run; fi

if [ "${failures}" -eq 0 ]; then
    echo "SOFTWARE_COMPAT ALL PASS"
    exit 0
fi

echo "SOFTWARE_COMPAT ALL FAIL failures=${failures}"
exit 1
