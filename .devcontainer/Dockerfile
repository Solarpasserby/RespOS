FROM zhouzhouyi/os-contest:20260104

USER root

RUN set -eux; \
    test -d /root/.cargo; \
    test -d /root/.rustup; \
    chmod o+x /root; \
    chmod a+x /opt/kendryte-toolchain/bin/riscv64-unknown-elf-*; \
    rm -rf /home/ubuntu/.cargo /home/ubuntu/.rustup; \
    mkdir -p /home/ubuntu/.cargo/bin; \
    cp /root/.cargo/config /home/ubuntu/.cargo/config.toml; \
    ln -sfn /root/.rustup /home/ubuntu/.rustup; \
    for tool_path in /root/.cargo/bin/*; do \
        tool="$(basename "${tool_path}")"; \
        ln -sf "${tool_path}" "/usr/local/bin/${tool}"; \
        ln -sf "${tool_path}" "/home/ubuntu/.cargo/bin/${tool}"; \
    done; \
    chown -R ubuntu:ubuntu /home/ubuntu/.cargo

ENV CARGO_HOME=/workspaces/RespOS/.cargo-home
ENV RUSTUP_HOME=/root/.rustup
ENV RUSTUP_TOOLCHAIN=nightly-2025-05-20
ENV PATH=/home/ubuntu/.cargo/bin:/opt/qemu-bin-10.0.2/bin:/opt/riscv64-linux-musl-cross/bin:/opt/loongarch64-linux-musl-cross/bin:/opt/gcc-13.2.0-loongarch64-linux-gnu/bin/:/opt/toolchain-loongarch64-linux-gnu-gcc8-host-x86_64-2022-07-18/bin/:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/opt/kendryte-toolchain/bin:/opt/riscv64-lp64d--musl--bleeding-edge-2024.02-1/bin

USER ubuntu
