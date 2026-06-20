#!/usr/bin/env bash
set -euo pipefail

image="${1:-img/sdcard-rv.img}"
grader_img_dir="${COURSEGRADER_TESTDATA:-/coursegrader/testdata}"

prepare_image() {
    local image="$1"
    local image_name
    local image_dir

    if [[ -f "$image" ]]; then
        return
    fi

    image_name="$(basename "$image")"
    image_dir="$(dirname "$image")"
    mkdir -p "$image_dir"

    if [[ -f "${grader_img_dir}/${image_name}" ]]; then
        cp "${grader_img_dir}/${image_name}" "$image"
    elif [[ -f "${grader_img_dir}/${image_name}.gz" ]]; then
        gzip -dc "${grader_img_dir}/${image_name}.gz" > "$image"
    elif [[ -f "${grader_img_dir}/${image_name}.xz" ]]; then
        xz -dc "${grader_img_dir}/${image_name}.xz" > "$image"
    fi
}

prepare_image "$image"

if [[ ! -f "$image" ]]; then
    echo "image not found: $image" >&2
    echo "also tried: ${grader_img_dir}/$(basename "$image")[.gz|.xz]" >&2
    exit 1
fi

if command -v e2fsck >/dev/null 2>&1; then
    status=0
    e2fsck -fy "$image" >/dev/null || status=$?
    if (( status > 1 )); then
        echo "e2fsck failed for $image with status $status" >&2
        exit "$status"
    fi
fi

tmp_passwd="/tmp/respos-ltp-passwd.$$"
tmp_group="/tmp/respos-ltp-group.$$"
tmp_mkfs="/tmp/respos-ltp-mkfs.$$"
cleanup() {
    rm -f "$tmp_passwd" "$tmp_group" "$tmp_mkfs"
}
trap cleanup EXIT HUP INT TERM

printf 'root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/nonexistent:/bin/false\n' > "$tmp_passwd"
cat > "$tmp_group" <<'GROUP'
root:x:0:root
daemon:x:1:daemon
bin:x:2:bin
sys:x:3:sys
adm:x:4:adm
tty:x:5:tty
disk:x:6:disk
lp:x:7:lp
mail:x:8:mail
news:x:9:news
uucp:x:10:uucp
nogroup:x:65534:nobody
nobody:x:65534:nobody
GROUP
printf '#!/bin/sh\nexit 0\n' > "$tmp_mkfs"

has_path() {
    local output

    if ! output="$(debugfs -R "stat $1" "$image" 2>&1)"; then
        echo "$output" >&2
        exit 1
    fi
    [[ "$output" != *"File not found"* ]]
}

if ! has_path /etc; then
    debugfs -w -R 'mkdir /etc' "$image"
fi

debugfs -w -R 'rm /etc/passwd' "$image" >/dev/null 2>&1 || true
debugfs -w -R "write $tmp_passwd /etc/passwd" "$image"
debugfs -w -R 'rm /etc/group' "$image" >/dev/null 2>&1 || true
debugfs -w -R "write $tmp_group /etc/group" "$image"

for dir in /musl /glibc; do
    if has_path "$dir"; then
        for tool in mkfs.ext2 mkfs.ext3 mkfs.ext4 mkfs.vfat; do
            if ! has_path "$dir/$tool"; then
                debugfs -w -R "write $tmp_mkfs $dir/$tool" "$image"
                debugfs -w -R "sif $dir/$tool mode 0100755" "$image"
            fi
        done
    fi
done
