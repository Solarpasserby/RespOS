#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
dist_dir="${1:-${repo_root}/dist}"
short_commit="$(git -C "${repo_root}" rev-parse --short=12 HEAD)"
archive_name="RespOS-${short_commit}.tar.gz"
archive_path="${dist_dir}/${archive_name}"
checksum_path="${archive_path}.sha256"

"${repo_root}/scripts/check_repo_state.sh" clean
mkdir -p "${dist_dir}"

temporary_archive="$(mktemp "${dist_dir}/.${archive_name}.XXXXXX")"
trap 'rm -f -- "${temporary_archive}"' EXIT

git -C "${repo_root}" archive --format=tar --prefix="RespOS-${short_commit}/" HEAD \
    | gzip -n >"${temporary_archive}"
mv -f -- "${temporary_archive}" "${archive_path}"
trap - EXIT

(
    cd "${dist_dir}"
    sha256sum "${archive_name}" >"${archive_name}.sha256"
)

printf '源码提交包：%s\n校验文件：%s\n' "${archive_path}" "${checksum_path}"
