SHELL := /bin/bash
.SHELLFLAGS := -o pipefail -c

# 线上评测平台固定调用 `make all`。该入口必须保持确定性：只生成两个 QEMU 平台的
# release 内核，以及作为第二块 virtio 块设备使用的两份自动识别辅助盘。
# 本地运行入口与提交入口严格分离，不得修改线上提交配置。
.NOTPARALLEL:

MODE ?= release
RV_MODE ?= $(MODE)
LA_MODE ?= $(MODE)

RV_TARGET := riscv64gc-unknown-none-elf
LA_TARGET := loongarch64-unknown-none

KERNEL_QEMU_RV64 := kernel-rv
KERNEL_JH7110 := kernel-vf2.bin
KERNEL_QEMU_LOONGARCH64 := kernel-la
KERNEL_LS2K1000 := respos-ls2k1000.bin
# 兼容既有运行脚本与提交产物名称；新平台构建规则使用上面的完整平台名。
KERNEL_RV := $(KERNEL_QEMU_RV64)
KERNEL_LA := $(KERNEL_QEMU_LOONGARCH64)
RV_ELF = os/target/$(RV_TARGET)/$(RV_CARGO_TARGET_DIR)/os
LA_ELF = os/target/$(LA_TARGET)/$(LA_CARGO_TARGET_DIR)/os

# 线上提交产物名称和 auto 配置固定不变，禁止通过命令行覆盖把 `make all`
# 静默变成本地初赛或诊断构建。
override AUXFS_PROFILE_DIR := auxfs/profiles
override AUXFS_PAYLOAD_DIR := auxfs/payloads
override AUX_DISK_BUILDER := scripts/build_aux_disk.sh
override SUBMIT_AUX_PROFILE := $(AUXFS_PROFILE_DIR)/auto.profile
override SUBMIT_RV_DISK_IMG := disk.img
override SUBMIT_LA_DISK_IMG := disk-la.img
override SUBMIT_AUX_FS_SIZE := 16M

# 本地根文件系统镜像。初赛镜像从保留的官方压缩包恢复到含义明确的文件名，
# 决赛镜像使用公开的大容量磁盘镜像。
RV_PRE_ARCHIVE ?= img/sdcard-rv.img.xz
LA_PRE_ARCHIVE ?= img/sdcard-la.img.xz
RV_PRE_FS_IMG ?= img/sdcard-rv-pre.img
LA_PRE_FS_IMG ?= img/sdcard-la-pre.img
RV_FINAL_FS_IMG ?= img/sdcard-rv-pub.img
LA_FINAL_FS_IMG ?= img/sdcard-la-pub.img
RV_SOFTWARE_FS_IMG ?= img/alpine-linux-riscv64-ext4fs.img
LA_SOFTWARE_BASE_IMG ?= img/alpine-linux-loongarch64-ext4fs.img
LA_SOFTWARE_FS_IMG ?= /tmp/respos-la-software-root.img

RV_PRE_DISK_IMG ?= /tmp/respos-rv-preliminary.img
LA_PRE_DISK_IMG ?= /tmp/respos-la-preliminary.img
RV_FINAL_DISK_IMG ?= /tmp/respos-rv-final.img
LA_FINAL_DISK_IMG ?= /tmp/respos-la-final.img
RV_DIAGNOSTIC_DISK_IMG ?= /tmp/respos-rv-diagnostic.img
LA_DIAGNOSTIC_DISK_IMG ?= /tmp/respos-la-diagnostic.img
RV_SOFTWARE_DISK_IMG ?= /tmp/respos-rv-software.img
LA_SOFTWARE_DISK_IMG ?= /tmp/respos-la-software.img
RV_BOOTSTRAP_DISK_IMG ?= /tmp/respos-rv-bootstrap.img
LA_BOOTSTRAP_DISK_IMG ?= /tmp/respos-la-bootstrap.img
LOCAL_AUX_FS_SIZE ?= 16M
BOOTSTRAP_AUX_FS_SIZE ?= 64M
BOOTSTRAP_SSH_KEY ?=
LA_BOOTSTRAP_SSH_PACKAGE ?= /tmp/openssh-client_10.2p1-3_loong64.deb
LA_BOOTSTRAP_RUST_STD_ARCHIVE ?= /tmp/rust-std-nightly-loongarch64-unknown-none-2026-05-28.tar.xz

# 本地资源配置。决赛默认值与 RespOS 最近记录的比赛参数一致；
# 各 QEMU 平台规则负责补齐自己的设备参数。
PRE_MEM ?= 4G
PRE_SMP ?= 1
RV_FINAL_MEM ?= 16G
RV_FINAL_SMP ?= 8
LA_FINAL_MEM ?= 36G
LA_FINAL_SMP ?= 12
RV_DIAGNOSTIC_MEM ?= 4G
RV_DIAGNOSTIC_SMP ?= 1
LA_DIAGNOSTIC_MEM ?= 12G
LA_DIAGNOSTIC_SMP ?= 12
RV_DIAGNOSTIC_KERNEL_FEATURES ?= kernel_http
LA_DIAGNOSTIC_KERNEL_FEATURES ?= kernel_http
RV_SOFTWARE_MEM ?= 4G
RV_SOFTWARE_SMP ?= 2
LA_SOFTWARE_MEM ?= 4G
LA_SOFTWARE_SMP ?= 2
RV_BOOTSTRAP_MEM ?= 8G
RV_BOOTSTRAP_SMP ?= 4
LA_BOOTSTRAP_MEM ?= 8G
LA_BOOTSTRAP_SMP ?= 4

RV_PRE_OUTPUT ?= rv-output.txt
LA_PRE_OUTPUT ?= la-output.txt
RV_FINAL_OUTPUT ?= rv-final-output.txt
LA_FINAL_OUTPUT ?= la-final-output.txt
RV_DIAGNOSTIC_OUTPUT ?= /tmp/respos-rv-diagnostic.log
LA_DIAGNOSTIC_OUTPUT ?= /tmp/respos-la-diagnostic.log
RV_SOFTWARE_OUTPUT ?= /tmp/respos-rv-software.log
LA_SOFTWARE_OUTPUT ?= /tmp/respos-la-software.log
RV_BOOTSTRAP_OUTPUT ?= /tmp/respos-rv-bootstrap.log
LA_BOOTSTRAP_OUTPUT ?= /tmp/respos-la-bootstrap.log

QEMU_RV ?= qemu-system-riscv64
QEMU_LA ?= qemu-system-loongarch64

# 用户态 feature 默认留空。旧 `eval` feature 不再选择启动器；
# `/respos/profile` 负责选择明确策略或自动识别策略。
RV_USER_FEATURES ?=
LA_USER_FEATURES ?=
RV_KERNEL_FEATURES ?=
LA_KERNEL_FEATURES ?=
RV_KERNEL_NO_DEFAULT_FEATURES ?= 0
LA_KERNEL_NO_DEFAULT_FEATURES ?= 0

REQUESTED_GOALS := $(if $(MAKECMDGOALS),$(MAKECMDGOALS),all)
ifneq ($(filter all submit check-submit,$(REQUESTED_GOALS)),)
ifneq ($(RV_MODE),release)
$(error Submission entry requires RV_MODE=release)
endif
ifneq ($(LA_MODE),release)
$(error Submission entry requires LA_MODE=release)
endif
ifneq ($(strip $(RV_USER_FEATURES)$(LA_USER_FEATURES)$(RV_KERNEL_FEATURES)$(LA_KERNEL_FEATURES)),)
$(error Submission entry does not accept user or kernel features)
endif
ifneq ($(filter 1,$(RV_KERNEL_NO_DEFAULT_FEATURES) $(LA_KERNEL_NO_DEFAULT_FEATURES)),)
$(error Submission entry requires kernel default features)
endif
endif

ifneq ($(strip $(RV_KERNEL_FEATURES)),)
	RV_KERNEL_FEATURE_ARGS := --features "$(RV_KERNEL_FEATURES)"
endif
ifneq ($(strip $(LA_KERNEL_FEATURES)),)
	LA_KERNEL_FEATURE_ARGS := --features "$(LA_KERNEL_FEATURES)"
endif
ifeq ($(RV_KERNEL_NO_DEFAULT_FEATURES),1)
	RV_KERNEL_DEFAULT_FEATURE_ARGS := --no-default-features
endif
ifeq ($(LA_KERNEL_NO_DEFAULT_FEATURES),1)
	LA_KERNEL_DEFAULT_FEATURE_ARGS := --no-default-features
endif

ifeq ($(RV_MODE),debug)
	RV_CARGO_TARGET_DIR := debug
	RV_CARGO_BUILD_ARG :=
else ifeq ($(RV_MODE),release)
	RV_CARGO_TARGET_DIR := release
	RV_CARGO_BUILD_ARG := --release
else ifeq ($(RV_MODE),release-debug)
	RV_CARGO_TARGET_DIR := release-debug
	RV_CARGO_BUILD_ARG := --profile release-debug
else
	$(error Unsupported RV_MODE '$(RV_MODE)'. Use debug, release, or release-debug)
endif

ifeq ($(LA_MODE),debug)
	LA_CARGO_TARGET_DIR := debug
	LA_CARGO_BUILD_ARG :=
else ifeq ($(LA_MODE),release)
	LA_CARGO_TARGET_DIR := release
	LA_CARGO_BUILD_ARG := --release
else ifeq ($(LA_MODE),release-debug)
	LA_CARGO_TARGET_DIR := release-debug
	LA_CARGO_BUILD_ARG := --profile release-debug
else
	$(error Unsupported LA_MODE '$(LA_MODE)'. Use debug, release, or release-debug)
endif

.PHONY: all submit check-submit validate-submit-profile build-submit-disks build-disks \
	build-qemu-rv64 build-jh7110 build-qemu-loongarch64 build-ls2k1000 \
	build-rv build-vf2 build-la build-la-ls2k1000 \
	prepare-qemu-rv64-cargo-config prepare-jh7110-cargo-config \
	prepare-qemu-loongarch64-cargo-config prepare-ls2k1000-cargo-config \
	prepare-rv-cargo-config prepare-la-cargo-config \
	prepare-pre-images check-rv-pre-image check-la-pre-image \
	check-rv-final-image check-la-final-image \
	prepare-la-software-root check-rv-software-image check-la-software-image \
	build-rv-local-disk build-la-local-disk run-rv-qemu run-la-qemu \
	build-rv-bootstrap-disk prepare-la-bootstrap-ssh prepare-la-bootstrap-rust-std \
	build-la-bootstrap-disk \
	run-rv-pre run-la-pre run-rv-final run-la-final \
	run-rv-diagnostic run-la-diagnostic run-rv-software run-la-software \
	run-rv-bootstrap run-la-bootstrap \
	rv la run-rv-pub run-la-pub \
	help clean

# 线上评测入口。这里不得加入 QEMU 运行、下载或本地根镜像检查；
# 评测平台会提供这些环境，只需要可复现的提交产物。
all: submit

submit: validate-submit-profile build-qemu-rv64 build-qemu-loongarch64 build-submit-disks

validate-submit-profile:
	@mode="$$(awk '{ line=$$0; sub(/^[ \t]+/, "", line); if (line != "" && substr(line, 1, 1) != "#") { print line; exit } }' $(SUBMIT_AUX_PROFILE))"; \
		test "$$mode" = "mode=auto" || { \
			echo "submission profile must start with mode=auto, got '$$mode'" >&2; \
			exit 1; \
		}

build-submit-disks: validate-submit-profile
	@bash $(AUX_DISK_BUILDER) $(SUBMIT_RV_DISK_IMG) $(SUBMIT_AUX_FS_SIZE) $(SUBMIT_AUX_PROFILE)
	@bash $(AUX_DISK_BUILDER) $(SUBMIT_LA_DISK_IMG) $(SUBMIT_AUX_FS_SIZE) $(SUBMIT_AUX_PROFILE)

# 兼容旧文档的目标名。它现在始终表示固定的自动识别提交盘，
# 不再接受调用方选择的本地运行配置。
build-disks: build-submit-disks

prepare-qemu-rv64-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-riscv64.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

prepare-jh7110-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-jh7110.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

prepare-qemu-loongarch64-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-loongarch64.toml os/.cargo/config.toml
	cp user/cargo/config-loongarch64.toml user/.cargo/config.toml

prepare-ls2k1000-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-ls2k1000.toml os/.cargo/config.toml
	cp user/cargo/config-ls2k1000.toml user/.cargo/config.toml

# 四个平台采用同构入口：准备平台 Cargo 配置、构建同架构用户程序、构建内核、转换平台产物。
# QEMU RV64：输出带固定入口地址的 ELF，供 QEMU `-kernel` 直接加载。
build-qemu-rv64: prepare-qemu-rv64-cargo-config
	$(MAKE) -C user build ARCH=riscv64 MODE=$(RV_MODE) FEATURES=$(RV_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(RV_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(RV_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(RV_CARGO_BUILD_ARG) $(RV_KERNEL_DEFAULT_FEATURE_ARGS) $(RV_KERNEL_FEATURE_ARGS)
	rust-objcopy --set-start=0x80200000 $(RV_ELF) $(KERNEL_QEMU_RV64)
	@rust-readobj -h -l $(KERNEL_QEMU_RV64) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

# JH7110（VisionFive 2）：输出 raw binary；装载地址 0x40200000 由 linker_jh7110.ld 决定。
build-jh7110: prepare-jh7110-cargo-config
	$(MAKE) -C user build ARCH=riscv64 MODE=$(RV_MODE) FEATURES=$(RV_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(RV_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(RV_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(RV_CARGO_BUILD_ARG) \
		$(RV_KERNEL_DEFAULT_FEATURE_ARGS) --features "board_jh7110 $(RV_KERNEL_FEATURES)"
	rust-objcopy -O binary --gap-fill=0 $(RV_ELF) $(KERNEL_JH7110)
	@file $(KERNEL_JH7110)

# QEMU LoongArch64：保留内核 ELF，供 QEMU `-kernel` 直接加载。
build-qemu-loongarch64: prepare-qemu-loongarch64-cargo-config
	$(MAKE) -C user build ARCH=loongarch64 MODE=$(LA_MODE) FEATURES=$(LA_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(LA_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(LA_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(LA_CARGO_BUILD_ARG) $(LA_KERNEL_DEFAULT_FEATURE_ARGS) $(LA_KERNEL_FEATURE_ARGS)
	cp $(LA_ELF) $(KERNEL_QEMU_LOONGARCH64)
	@rust-readobj -h -l $(KERNEL_QEMU_LOONGARCH64) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

# LS2K1000：输出供 U-Boot TFTP + `go` 使用的 raw binary。
build-ls2k1000: prepare-ls2k1000-cargo-config
	$(MAKE) -C user build ARCH=loongarch64 MODE=$(LA_MODE) FEATURES=$(LA_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(LA_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(LA_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) \
		cargo build $(LA_CARGO_BUILD_ARG) $(LA_KERNEL_DEFAULT_FEATURE_ARGS) \
		--features "board_ls2k1000 fault_trace $(LA_KERNEL_FEATURES)"
	rust-objcopy -O binary --strip-all $(LA_ELF) $(KERNEL_LS2K1000)
	@file $(KERNEL_LS2K1000)

# 兼容旧命令；新脚本和文档应使用上面的完整平台名。
prepare-rv-cargo-config: prepare-qemu-rv64-cargo-config
prepare-la-cargo-config: prepare-qemu-loongarch64-cargo-config
build-rv: build-qemu-rv64
build-vf2: build-jh7110
build-la: build-qemu-loongarch64
build-la-ls2k1000: build-ls2k1000

$(RV_PRE_FS_IMG): $(RV_PRE_ARCHIVE)
	@mkdir -p $(@D)
	xz -dc $< > $@.tmp
	mv $@.tmp $@

$(LA_PRE_FS_IMG): $(LA_PRE_ARCHIVE)
	@mkdir -p $(@D)
	xz -dc $< > $@.tmp
	mv $@.tmp $@

prepare-pre-images: $(RV_PRE_FS_IMG) $(LA_PRE_FS_IMG)

check-rv-pre-image: $(RV_PRE_FS_IMG)
	@test "$$(stat -c %s $(RV_PRE_FS_IMG))" -ge 1073741824 || { \
		echo "$(RV_PRE_FS_IMG) is too small for the preliminary suite; restore it from $(RV_PRE_ARCHIVE)" >&2; \
		exit 1; \
	}
	@debugfs -R 'stat /musl/basic_testcode.sh' $(RV_PRE_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
		echo "$(RV_PRE_FS_IMG) does not contain the preliminary test suite" >&2; \
		exit 1; \
	}

check-la-pre-image: $(LA_PRE_FS_IMG)
	@test "$$(stat -c %s $(LA_PRE_FS_IMG))" -ge 1073741824 || { \
		echo "$(LA_PRE_FS_IMG) is too small for the preliminary suite; restore it from $(LA_PRE_ARCHIVE)" >&2; \
		exit 1; \
	}
	@debugfs -R 'stat /musl/basic_testcode.sh' $(LA_PRE_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
		echo "$(LA_PRE_FS_IMG) does not contain the preliminary test suite" >&2; \
		exit 1; \
	}

check-rv-final-image:
	@test -r $(RV_FINAL_FS_IMG) || { echo "missing $(RV_FINAL_FS_IMG)" >&2; exit 1; }
	@debugfs -R 'stat /glibc/cagent_testcode.sh' $(RV_FINAL_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
		echo "$(RV_FINAL_FS_IMG) has no CAgent script" >&2; exit 1; \
	}
	@debugfs -R 'stat /glibc/buildstorm_testcode.sh' $(RV_FINAL_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
		echo "$(RV_FINAL_FS_IMG) has no BuildStorm script" >&2; exit 1; \
	}

check-la-final-image:
	@test -r $(LA_FINAL_FS_IMG) || { echo "missing $(LA_FINAL_FS_IMG)" >&2; exit 1; }
	@debugfs -R 'stat /glibc/cagent_testcode.sh' $(LA_FINAL_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
		echo "$(LA_FINAL_FS_IMG) has no CAgent script" >&2; exit 1; \
	}
	@debugfs -R 'stat /glibc/buildstorm_testcode.sh' $(LA_FINAL_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
		echo "$(LA_FINAL_FS_IMG) has no BuildStorm script" >&2; exit 1; \
	}

check-rv-software-image:
	@test -r $(RV_SOFTWARE_FS_IMG) || { echo "missing $(RV_SOFTWARE_FS_IMG); run scripts/get_img.sh software rv" >&2; exit 1; }
	@for path in /usr/bin/git /usr/bin/vim /usr/bin/gcc /usr/bin/rustc /usr/bin/make /usr/bin/ar /usr/bin/cargo /usr/bin/flock /sbin/apk /bin/tar /bin/gzip /bin/sh; do \
		debugfs -R "stat $$path" $(RV_SOFTWARE_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
			echo "$(RV_SOFTWARE_FS_IMG) is missing $$path" >&2; exit 1; \
		}; \
	done

prepare-la-software-root: $(LA_SOFTWARE_FS_IMG)
	@echo "Prepared LA software root copy: $(LA_SOFTWARE_FS_IMG)"
	@sha256sum $(LA_SOFTWARE_FS_IMG)

$(LA_SOFTWARE_FS_IMG): $(LA_SOFTWARE_BASE_IMG)
	@mkdir -p $(@D)
	cp --reflink=auto --sparse=always $< $@.tmp
	@status=0; e2fsck -p $@.tmp || status=$$?; \
		test $$status -le 1 || { echo "e2fsck failed for LA software image copy: $$status" >&2; exit $$status; }
	mv -f $@.tmp $@

check-la-software-image: prepare-la-software-root
	@for path in /usr/bin/git /usr/bin/vim /usr/bin/gcc /usr/bin/rustc /usr/bin/make /usr/bin/ar /usr/bin/cargo /usr/bin/flock /sbin/apk /bin/tar /bin/gzip /bin/sh; do \
		debugfs -R "stat $$path" $(LA_SOFTWARE_FS_IMG) 2>&1 | grep -q '^Inode:' || { \
			echo "$(LA_SOFTWARE_FS_IMG) is missing $$path" >&2; exit 1; \
		}; \
	done
	@tune2fs -l $(LA_SOFTWARE_FS_IMG) 2>/dev/null | grep -q '^Filesystem state:[[:space:]]*clean[[:space:]]*$$' || { \
		echo "$(LA_SOFTWARE_FS_IMG) is not clean after e2fsck" >&2; exit 1; \
	}

build-rv-local-disk:
	@bash $(AUX_DISK_BUILDER) $(RV_DISK_IMG) $(LOCAL_AUX_FS_SIZE) \
		$(AUX_PROFILE) $(AUX_PAYLOAD_DIRS)

build-la-local-disk:
	@bash $(AUX_DISK_BUILDER) $(LA_DISK_IMG) $(LOCAL_AUX_FS_SIZE) \
		$(AUX_PROFILE) $(AUX_PAYLOAD_DIRS)

build-rv-bootstrap-disk:
	@test -n "$(BOOTSTRAP_SSH_KEY)" || { echo "set BOOTSTRAP_SSH_KEY to a temporary read-only SSH key" >&2; exit 1; }
	@bash scripts/build_bootstrap_disk.sh $(RV_BOOTSTRAP_DISK_IMG) $(BOOTSTRAP_AUX_FS_SIZE) $(BOOTSTRAP_SSH_KEY)

prepare-la-bootstrap-ssh:
	@bash scripts/get_bootstrap_ssh.sh $(LA_BOOTSTRAP_SSH_PACKAGE)

prepare-la-bootstrap-rust-std:
	@bash scripts/get_bootstrap_rust_std.sh $(LA_BOOTSTRAP_RUST_STD_ARCHIVE)

build-la-bootstrap-disk: prepare-la-bootstrap-ssh prepare-la-bootstrap-rust-std
	@test -n "$(BOOTSTRAP_SSH_KEY)" || { echo "set BOOTSTRAP_SSH_KEY to a temporary read-only SSH key" >&2; exit 1; }
	@bash scripts/build_bootstrap_disk.sh $(LA_BOOTSTRAP_DISK_IMG) $(BOOTSTRAP_AUX_FS_SIZE) $(BOOTSTRAP_SSH_KEY) $(LA_BOOTSTRAP_SSH_PACKAGE) $(LA_BOOTSTRAP_RUST_STD_ARCHIVE)

run-rv-qemu:
	@test -r $(RV_FS_IMG) || { echo "missing root image $(RV_FS_IMG)" >&2; exit 1; }
	@test -r $(RV_DISK_IMG) || { echo "missing auxiliary image $(RV_DISK_IMG)" >&2; exit 1; }
	$(QEMU_RV) -machine virt \
		-kernel $(KERNEL_RV) \
		-m $(MEM) \
		-nographic \
		-snapshot \
		-smp $(SMP) \
		-bios default \
		-drive file=$(RV_FS_IMG),if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-no-reboot \
		-device virtio-net-device,netdev=net \
		-netdev user,id=net,hostfwd=tcp::8080-:80 \
		-rtc base=utc \
		-drive file=$(RV_DISK_IMG),if=none,format=raw,id=x1 \
		-device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1 |& tee $(RV_OUTPUT)

run-la-qemu:
	@test -r $(LA_FS_IMG) || { echo "missing root image $(LA_FS_IMG)" >&2; exit 1; }
	@test -r $(LA_DISK_IMG) || { echo "missing auxiliary image $(LA_DISK_IMG)" >&2; exit 1; }
	$(QEMU_LA) -machine virt \
		-kernel $(KERNEL_LA) \
		-m $(LA_MEM) \
		-nographic \
		-snapshot \
		-smp $(LA_SMP) \
		-drive file=$(LA_FS_IMG),if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 \
		-no-reboot \
		-device virtio-net-pci,netdev=net0 \
		-netdev user,id=net0,hostfwd=tcp::8080-:80,hostfwd=tcp::5555-:5555,hostfwd=udp::5555-:5555 \
		-rtc base=utc \
		-drive file=$(LA_DISK_IMG),if=none,format=raw,id=x1 \
		-device virtio-blk-pci,drive=x1 |& tee $(LA_OUTPUT)

# 初赛：contest_launcher 读取 mode=preliminary，执行内嵌 testrunner，
# 由 testrunner 输出初赛评测分组标记。
run-rv-pre: RV_FS_IMG = $(RV_PRE_FS_IMG)
run-rv-pre: RV_DISK_IMG = $(RV_PRE_DISK_IMG)
run-rv-pre: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/preliminary.profile
run-rv-pre: MEM = $(PRE_MEM)
run-rv-pre: SMP = $(PRE_SMP)
run-rv-pre: RV_OUTPUT = $(RV_PRE_OUTPUT)
run-rv-pre: build-qemu-rv64 check-rv-pre-image build-rv-local-disk run-rv-qemu

run-la-pre: LA_FS_IMG = $(LA_PRE_FS_IMG)
run-la-pre: LA_DISK_IMG = $(LA_PRE_DISK_IMG)
run-la-pre: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/preliminary.profile
run-la-pre: LA_MEM = $(PRE_MEM)
run-la-pre: LA_SMP = $(PRE_SMP)
run-la-pre: LA_OUTPUT = $(LA_PRE_OUTPUT)
run-la-pre: build-qemu-loongarch64 check-la-pre-image build-la-local-disk run-la-qemu

# 决赛：contest_launcher 跳过 testrunner，依次执行公开根镜像中的两份官方 glibc 脚本。
run-rv-final: RV_FS_IMG = $(RV_FINAL_FS_IMG)
run-rv-final: RV_DISK_IMG = $(RV_FINAL_DISK_IMG)
run-rv-final: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/final.profile
run-rv-final: MEM = $(RV_FINAL_MEM)
run-rv-final: SMP = $(RV_FINAL_SMP)
run-rv-final: RV_OUTPUT = $(RV_FINAL_OUTPUT)
run-rv-final: build-qemu-rv64 check-rv-final-image build-rv-local-disk run-rv-qemu

run-la-final: LA_FS_IMG = $(LA_FINAL_FS_IMG)
run-la-final: LA_DISK_IMG = $(LA_FINAL_DISK_IMG)
run-la-final: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/final.profile
run-la-final: LA_MEM = $(LA_FINAL_MEM)
run-la-final: LA_SMP = $(LA_FINAL_SMP)
run-la-final: LA_OUTPUT = $(LA_FINAL_OUTPUT)
run-la-final: build-qemu-loongarch64 check-la-final-image build-la-local-disk run-la-qemu

# 诊断：使用决赛根镜像，但进入内嵌用户 shell，以便手工运行单个官方脚本或内嵌探针。
run-rv-diagnostic: RV_FS_IMG = $(RV_FINAL_FS_IMG)
run-rv-diagnostic: RV_DISK_IMG = $(RV_DIAGNOSTIC_DISK_IMG)
run-rv-diagnostic: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/diagnostic.profile
run-rv-diagnostic: MEM = $(RV_DIAGNOSTIC_MEM)
run-rv-diagnostic: SMP = $(RV_DIAGNOSTIC_SMP)
run-rv-diagnostic: RV_OUTPUT = $(RV_DIAGNOSTIC_OUTPUT)
run-rv-diagnostic: RV_KERNEL_FEATURE_ARGS = --features "$(RV_DIAGNOSTIC_KERNEL_FEATURES)"
run-rv-diagnostic: build-qemu-rv64 check-rv-final-image build-rv-local-disk run-rv-qemu

run-la-diagnostic: LA_FS_IMG = $(LA_FINAL_FS_IMG)
run-la-diagnostic: LA_DISK_IMG = $(LA_DIAGNOSTIC_DISK_IMG)
run-la-diagnostic: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/diagnostic.profile
run-la-diagnostic: LA_MEM = $(LA_DIAGNOSTIC_MEM)
run-la-diagnostic: LA_SMP = $(LA_DIAGNOSTIC_SMP)
run-la-diagnostic: LA_OUTPUT = $(LA_DIAGNOSTIC_OUTPUT)
run-la-diagnostic: LA_KERNEL_FEATURE_ARGS = --features "$(LA_DIAGNOSTIC_KERNEL_FEATURES)"
run-la-diagnostic: build-qemu-loongarch64 check-la-final-image build-la-local-disk run-la-qemu

# 软件兼容性：以 `-snapshot` 挂载归档的 Alpine 根镜像，
# 通过交互式 Alpine shell 提供可复现的冒烟脚本。
run-rv-software: RV_FS_IMG = $(RV_SOFTWARE_FS_IMG)
run-rv-software: RV_DISK_IMG = $(RV_SOFTWARE_DISK_IMG)
run-rv-software: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/software.profile
run-rv-software: AUX_PAYLOAD_DIRS = $(AUXFS_PAYLOAD_DIR)/software
run-rv-software: MEM = $(RV_SOFTWARE_MEM)
run-rv-software: SMP = $(RV_SOFTWARE_SMP)
run-rv-software: RV_OUTPUT = $(RV_SOFTWARE_OUTPUT)
run-rv-software: build-qemu-rv64 check-rv-software-image build-rv-local-disk run-rv-qemu

run-la-software: LA_FS_IMG = $(LA_SOFTWARE_FS_IMG)
run-la-software: LA_DISK_IMG = $(LA_SOFTWARE_DISK_IMG)
run-la-software: AUX_PROFILE = $(AUXFS_PROFILE_DIR)/software.profile
run-la-software: AUX_PAYLOAD_DIRS = $(AUXFS_PAYLOAD_DIR)/software
run-la-software: LA_MEM = $(LA_SOFTWARE_MEM)
run-la-software: LA_SMP = $(LA_SOFTWARE_SMP)
run-la-software: LA_OUTPUT = $(LA_SOFTWARE_OUTPUT)
run-la-software: build-qemu-loongarch64 check-la-software-image build-la-local-disk run-la-qemu

# 自举：通过 Git-over-SSH 把 RespOS 克隆到决赛根镜像，并构建对应架构。
# 私钥只进入 `/tmp` 下的临时辅助盘，不会复制到仓库或归档镜像。
run-rv-bootstrap: RV_FS_IMG = $(RV_FINAL_FS_IMG)
run-rv-bootstrap: RV_DISK_IMG = $(RV_BOOTSTRAP_DISK_IMG)
run-rv-bootstrap: MEM = $(RV_BOOTSTRAP_MEM)
run-rv-bootstrap: SMP = $(RV_BOOTSTRAP_SMP)
run-rv-bootstrap: RV_OUTPUT = $(RV_BOOTSTRAP_OUTPUT)
run-rv-bootstrap: build-qemu-rv64 check-rv-final-image build-rv-bootstrap-disk run-rv-qemu

run-la-bootstrap: LA_FS_IMG = $(LA_FINAL_FS_IMG)
run-la-bootstrap: LA_DISK_IMG = $(LA_BOOTSTRAP_DISK_IMG)
run-la-bootstrap: LA_MEM = $(LA_BOOTSTRAP_MEM)
run-la-bootstrap: LA_SMP = $(LA_BOOTSTRAP_SMP)
run-la-bootstrap: LA_OUTPUT = $(LA_BOOTSTRAP_OUTPUT)
run-la-bootstrap: build-qemu-loongarch64 check-la-final-image build-la-bootstrap-disk run-la-qemu

# 兼容旧运行命令；新脚本和文档应使用上面的明确名称。
rv:
	@echo "make rv is an alias for make run-rv-pre"
	@$(MAKE) run-rv-pre

la:
	@echo "make la is an alias for make run-la-pre"
	@$(MAKE) run-la-pre

run-rv-pub:
	@echo "make run-rv-pub is an alias for make run-rv-final"
	@$(MAKE) run-rv-final

run-la-pub:
	@echo "make run-la-pub is an alias for make run-la-final"
	@$(MAKE) run-la-final

check-submit: submit
	@test -s $(KERNEL_RV) -a -s $(KERNEL_LA)
	@test -s $(SUBMIT_RV_DISK_IMG) -a -s $(SUBMIT_LA_DISK_IMG)
	@file $(KERNEL_RV) $(KERNEL_LA) $(SUBMIT_RV_DISK_IMG) $(SUBMIT_LA_DISK_IMG)
	@for image in $(SUBMIT_RV_DISK_IMG) $(SUBMIT_LA_DISK_IMG); do \
		mode="$$(debugfs -R 'cat /profile' $$image 2>/dev/null | awk '{ line=$$0; sub(/^[ \t]+/, "", line); if (line != "" && substr(line, 1, 1) != "#") { print line; exit } }')"; \
		test "$$mode" = "mode=auto" || { echo "$$image does not contain mode=auto" >&2; exit 1; }; \
	done

help:
	@echo "四平台构建："
	@echo "  make build-qemu-rv64          构建 QEMU RV64 内核（kernel-rv）"
	@echo "  make build-jh7110             构建 JH7110/VisionFive 2 镜像（kernel-vf2.bin）"
	@echo "  make build-qemu-loongarch64   构建 QEMU LoongArch64 内核（kernel-la）"
	@echo "  make build-ls2k1000           构建 LS2K1000 镜像（respos-ls2k1000.bin）"
	@echo "线上提交："
	@echo "  make all                      构建两个 QEMU 内核和两份自动识别提交盘"
	@echo "  make check-submit             重新构建并检查四个提交产物"
	@echo "本地初赛测例（内嵌 testrunner）："
	@echo "  make prepare-pre-images"
	@echo "  make run-rv-pre"
	@echo "  make run-la-pre"
	@echo "本地决赛评分脚本："
	@echo "  make run-rv-final             默认 16 GiB / 8 hart"
	@echo "  make run-la-final             默认 36 GiB / 12 hart"
	@echo "交互式诊断："
	@echo "  make run-rv-diagnostic"
	@echo "  make run-la-diagnostic"
	@echo "Alpine 软件兼容性："
	@echo "  make run-rv-software          默认 4 GiB / 2 hart"
	@echo "  make run-la-software          默认 4 GiB / 2 hart；只修复 /tmp 下的副本"
	@echo "Git-over-SSH 克隆与自举构建（需要 BOOTSTRAP_SSH_KEY）："
	@echo "  make run-rv-bootstrap         默认 8 GiB / 4 hart"
	@echo "  make run-la-bootstrap         默认 8 GiB / 4 hart"
	@echo "兼容旧名：build-rv、build-vf2、build-la、build-la-ls2k1000"

clean:
	rm -f $(KERNEL_QEMU_RV64) $(KERNEL_JH7110) $(KERNEL_QEMU_LOONGARCH64) $(KERNEL_LS2K1000) \
		$(SUBMIT_RV_DISK_IMG) $(SUBMIT_LA_DISK_IMG)
	$(MAKE) -C os clean
