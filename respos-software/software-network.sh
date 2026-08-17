#!/bin/sh

set -u

WORK=/tmp/respos-software-network
DNS_SERVER=10.0.2.3
HTTP_URL=http://example.com/
GIT_REMOTE=https://github.com/octocat/Hello-World.git
failures=0

export HOME=/tmp/respos-software-network-home
export TMPDIR=/tmp
export LC_ALL=C
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export GIT_TERMINAL_PROMPT=0

pass() {
    echo "SOFTWARE_NETWORK $1 PASS"
}

fail() {
    echo "SOFTWARE_NETWORK $1 FAIL"
    failures=$((failures + 1))
}

rm -rf "${WORK}" "${HOME}"
mkdir -p "${WORK}" "${HOME}"

echo "SOFTWARE_NETWORK BEGIN"
uname -a || true

# The launcher installs this fallback for the archived images. Keep the script
# runnable under older launchers without overwriting an existing DNS policy.
if ! grep -q 'nameserver' /etc/resolv.conf 2>/dev/null; then
    printf 'nameserver %s\n' "${DNS_SERVER}" > /etc/resolv.conf
fi

dns_ok=1
timeout 20 nslookup example.com "${DNS_SERVER}" > "${WORK}/dns.txt" 2>&1 || dns_ok=0
grep -q 'Address:' "${WORK}/dns.txt" || dns_ok=0
if [ "${dns_ok}" -eq 1 ]; then pass udp_dns; else fail udp_dns; fi

http_ok=1
timeout 30 wget -q -O "${WORK}/example.html" "${HTTP_URL}" || http_ok=0
test -s "${WORK}/example.html" || http_ok=0
if [ "${http_ok}" -eq 1 ]; then pass public_http; else fail public_http; fi

git_remote_ok=1
timeout 120 git ls-remote "${GIT_REMOTE}" HEAD > "${WORK}/ls-remote.txt" 2>&1 || git_remote_ok=0
grep -q '[[:space:]]HEAD$' "${WORK}/ls-remote.txt" || git_remote_ok=0
if [ "${git_remote_ok}" -eq 1 ]; then pass git_https_ls_remote; else fail git_https_ls_remote; fi

git_clone_ok=1
timeout 180 git clone --depth=1 "${GIT_REMOTE}" "${WORK}/hello-world" \
    > "${WORK}/clone.txt" 2>&1 || git_clone_ok=0
test -d "${WORK}/hello-world/.git" || git_clone_ok=0
test -f "${WORK}/hello-world/README" || git_clone_ok=0
if [ "${git_clone_ok}" -eq 1 ]; then
    pass git_https_clone
    git -C "${WORK}/hello-world" rev-parse --short HEAD || true
else
    fail git_https_clone
fi

if [ "${failures}" -eq 0 ]; then
    echo "SOFTWARE_NETWORK ALL PASS"
    exit 0
fi

echo "SOFTWARE_NETWORK ALL FAIL failures=${failures}"
exit 1
