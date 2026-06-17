#!/usr/bin/env sh
set -eu

image="${1:-img/sdcard-rv.img}"

if [ ! -f "$image" ]; then
    echo "image not found: $image" >&2
    exit 1
fi

tmp_passwd="/tmp/respos-ltp-passwd.$$"
tmp_group="/tmp/respos-ltp-group.$$"
tmp_mkfs="/tmp/respos-ltp-mkfs.$$"
cleanup() {
    rm -f "$tmp_passwd" "$tmp_group" "$tmp_mkfs"
}
trap cleanup EXIT HUP INT TERM

printf 'root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/nonexistent:/bin/false\n' > "$tmp_passwd"
printf 'root:x:0:\nnogroup:x:65534:\nnobody:x:65534:\n' > "$tmp_group"
printf '#!/bin/sh\nexit 0\n' > "$tmp_mkfs"

has_path() {
    ! debugfs -R "stat $1" "$image" 2>&1 | grep -q 'File not found'
}

if ! has_path /etc; then
    debugfs -w -R 'mkdir /etc' "$image"
fi

if ! has_path /etc/passwd; then
    debugfs -w -R "write $tmp_passwd /etc/passwd" "$image"
fi

if ! has_path /etc/group; then
    debugfs -w -R "write $tmp_group /etc/group" "$image"
fi

for dir in /musl /glibc; do
    if has_path "$dir"; then
        if ! has_path "$dir/mkfs.ext2"; then
            debugfs -w -R "write $tmp_mkfs $dir/mkfs.ext2" "$image"
            debugfs -w -R "sif $dir/mkfs.ext2 mode 0100755" "$image"
        fi
        for tool in mkfs.ext3 mkfs.ext4 mkfs.vfat; do
            if has_path "$dir/$tool"; then
                debugfs -w -R "rm $dir/$tool" "$image"
            fi
        done
    fi
done
