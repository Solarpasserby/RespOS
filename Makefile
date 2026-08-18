SHELL := /bin/bash
.SHELLFLAGS := -o pipefail -c

# The contest platform invokes `make all`.  Keep that path deterministic:
# release kernels for both architectures plus the two auto-detect auxiliary
# disks consumed as the second virtio block device.  Local run targets are
# deliberately separate and never change the submission profile.
.NOTPARALLEL:

MODE ?= release
RV_MODE ?= $(MODE)
LA_MODE ?= $(MODE)

RV_TARGET := riscv64gc-unknown-none-elf
LA_TARGET := loongarch64-unknown-none

KERNEL_RV := kernel-rv
KERNEL_LA := kernel-la
KERNEL_VF2 := kernel-vf2.bin
KERNEL_LA_LS2K1000 := respos-ls2k1000.bin
RV_ELF = os/target/$(RV_TARGET)/$(RV_CARGO_TARGET_DIR)/os
LA_ELF = os/target/$(LA_TARGET)/$(LA_CARGO_TARGET_DIR)/os

# Submission artifacts.  These names and the auto profile are intentionally
# fixed: command-line overrides must not silently turn `make all` into a local
# preliminary or diagnostic build.
override SUBMIT_AUX_FS_DIR := respos
override SUBMIT_RV_DISK_IMG := disk.img
override SUBMIT_LA_DISK_IMG := disk-la.img
override SUBMIT_AUX_FS_SIZE := 16M

# Local root images.  Preliminary images are restored under unambiguous names
# from the retained official archives; final images are the public large disks.
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

# Local resource profiles.  The final defaults mirror the latest contest
# parameters recorded for RespOS; the platform itself supplies its QEMU args.
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

# User features are empty by default.  The legacy `eval` feature no longer
# selects the launcher; /respos/profile selects explicit or automatic policy.
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
	build-rv build-la build-la-ls2k1000 prepare-rv-cargo-config prepare-la-cargo-config \
	prepare-jh7110-cargo-config \
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
	build-vf2 \
	help clean

# Online-platform entry.  Do not add QEMU runs, downloads, or local root-image
# checks here: the evaluator provides those and only needs reproducible outputs.
all: submit

submit: validate-submit-profile build-rv build-la build-submit-disks

validate-submit-profile:
	@mode="$$(awk '{ line=$$0; sub(/^[ \t]+/, "", line); if (line != "" && substr(line, 1, 1) != "#") { print line; exit } }' $(SUBMIT_AUX_FS_DIR)/profile)"; \
		test "$$mode" = "mode=auto" || { \
			echo "submission profile must start with mode=auto, got '$$mode'" >&2; \
			exit 1; \
		}

build-submit-disks: validate-submit-profile
	truncate -s $(SUBMIT_AUX_FS_SIZE) $(SUBMIT_RV_DISK_IMG)
	mkfs.ext4 -q -F -d $(SUBMIT_AUX_FS_DIR) $(SUBMIT_RV_DISK_IMG)
	truncate -s $(SUBMIT_AUX_FS_SIZE) $(SUBMIT_LA_DISK_IMG)
	mkfs.ext4 -q -F -d $(SUBMIT_AUX_FS_DIR) $(SUBMIT_LA_DISK_IMG)

# Compatibility name used by older documentation.  It now always means the
# fixed auto-detect submission disks, never a caller-selected local profile.
build-disks: build-submit-disks

prepare-rv-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-riscv64.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

prepare-la-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-loongarch64.toml os/.cargo/config.toml
	cp user/cargo/config-loongarch64.toml user/.cargo/config.toml

prepare-jh7110-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-jh7110.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

build-rv: prepare-rv-cargo-config
	$(MAKE) -C user build ARCH=riscv64 MODE=$(RV_MODE) FEATURES=$(RV_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(RV_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(RV_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(RV_CARGO_BUILD_ARG) $(RV_KERNEL_DEFAULT_FEATURE_ARGS) $(RV_KERNEL_FEATURE_ARGS)
	rust-objcopy --set-start=0x80200000 $(RV_ELF) $(KERNEL_RV)
	@rust-readobj -h -l $(KERNEL_RV) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

# VisionFive 2 (JH7110) 真机镜像：raw binary，装载地址 0x40200000（由 linker_jh7110.ld 决定）。
build-vf2: prepare-jh7110-cargo-config
	$(MAKE) -C user build ARCH=riscv64 MODE=$(RV_MODE) FEATURES=$(RV_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(RV_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(RV_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(RV_CARGO_BUILD_ARG) --features board_jh7110
	rust-objcopy -O binary --gap-fill=0 $(RV_ELF) $(KERNEL_VF2)
	@file $(KERNEL_VF2)

build-la: prepare-la-cargo-config
	$(MAKE) -C user build ARCH=loongarch64 MODE=$(LA_MODE) FEATURES=$(LA_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(LA_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(LA_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(LA_CARGO_BUILD_ARG) $(LA_KERNEL_DEFAULT_FEATURE_ARGS) $(LA_KERNEL_FEATURE_ARGS)
	cp $(LA_ELF) $(KERNEL_LA)
	@rust-readobj -h -l $(KERNEL_LA) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

# LoongArch64 → 龙芯 2K1000LA 真机（Stage 1）。生成供 U-Boot TFTP + `go` 的 raw binary。
# 与 `build-la` 互斥（共享同一 Cargo target 目录），不得并行运行。
build-la-ls2k1000: prepare-la-cargo-config
	$(MAKE) -C user build ARCH=loongarch64 MODE=$(LA_MODE) FEATURES=$(LA_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(LA_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(LA_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) \
		cargo build $(LA_CARGO_BUILD_ARG) $(LA_KERNEL_DEFAULT_FEATURE_ARGS) --features board_ls2k1000,fault_trace
	rust-objcopy -O binary --strip-all $(LA_ELF) $(KERNEL_LA_LS2K1000)
	@ls -l $(KERNEL_LA_LS2K1000)

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
	@test -r $(AUX_FS_DIR)/profile || { echo "missing $(AUX_FS_DIR)/profile" >&2; exit 1; }
	truncate -s $(LOCAL_AUX_FS_SIZE) $(RV_DISK_IMG)
	mkfs.ext4 -q -F -d $(AUX_FS_DIR) $(RV_DISK_IMG)

build-la-local-disk:
	@test -r $(AUX_FS_DIR)/profile || { echo "missing $(AUX_FS_DIR)/profile" >&2; exit 1; }
	truncate -s $(LOCAL_AUX_FS_SIZE) $(LA_DISK_IMG)
	mkfs.ext4 -q -F -d $(AUX_FS_DIR) $(LA_DISK_IMG)

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

# Preliminary: contest_launcher reads mode=preliminary and execs the embedded
# testrunner, which emits the preliminary judge group markers.
run-rv-pre: RV_FS_IMG = $(RV_PRE_FS_IMG)
run-rv-pre: RV_DISK_IMG = $(RV_PRE_DISK_IMG)
run-rv-pre: AUX_FS_DIR = respos-preliminary
run-rv-pre: MEM = $(PRE_MEM)
run-rv-pre: SMP = $(PRE_SMP)
run-rv-pre: RV_OUTPUT = $(RV_PRE_OUTPUT)
run-rv-pre: build-rv check-rv-pre-image build-rv-local-disk run-rv-qemu

run-la-pre: LA_FS_IMG = $(LA_PRE_FS_IMG)
run-la-pre: LA_DISK_IMG = $(LA_PRE_DISK_IMG)
run-la-pre: AUX_FS_DIR = respos-preliminary
run-la-pre: LA_MEM = $(PRE_MEM)
run-la-pre: LA_SMP = $(PRE_SMP)
run-la-pre: LA_OUTPUT = $(LA_PRE_OUTPUT)
run-la-pre: build-la check-la-pre-image build-la-local-disk run-la-qemu

# Final: contest_launcher bypasses testrunner and serially runs the two official
# glibc scripts from the public root image.
run-rv-final: RV_FS_IMG = $(RV_FINAL_FS_IMG)
run-rv-final: RV_DISK_IMG = $(RV_FINAL_DISK_IMG)
run-rv-final: AUX_FS_DIR = respos-final
run-rv-final: MEM = $(RV_FINAL_MEM)
run-rv-final: SMP = $(RV_FINAL_SMP)
run-rv-final: RV_OUTPUT = $(RV_FINAL_OUTPUT)
run-rv-final: build-rv check-rv-final-image build-rv-local-disk run-rv-qemu

run-la-final: LA_FS_IMG = $(LA_FINAL_FS_IMG)
run-la-final: LA_DISK_IMG = $(LA_FINAL_DISK_IMG)
run-la-final: AUX_FS_DIR = respos-final
run-la-final: LA_MEM = $(LA_FINAL_MEM)
run-la-final: LA_SMP = $(LA_FINAL_SMP)
run-la-final: LA_OUTPUT = $(LA_FINAL_OUTPUT)
run-la-final: build-la check-la-final-image build-la-local-disk run-la-qemu

# Diagnostic: use the final root image but enter the embedded user shell so a
# single official script or an embedded probe can be run manually.
run-rv-diagnostic: RV_FS_IMG = $(RV_FINAL_FS_IMG)
run-rv-diagnostic: RV_DISK_IMG = $(RV_DIAGNOSTIC_DISK_IMG)
run-rv-diagnostic: AUX_FS_DIR = respos-diagnostic
run-rv-diagnostic: MEM = $(RV_DIAGNOSTIC_MEM)
run-rv-diagnostic: SMP = $(RV_DIAGNOSTIC_SMP)
run-rv-diagnostic: RV_OUTPUT = $(RV_DIAGNOSTIC_OUTPUT)
run-rv-diagnostic: RV_KERNEL_FEATURE_ARGS = --features "$(RV_DIAGNOSTIC_KERNEL_FEATURES)"
run-rv-diagnostic: build-rv check-rv-final-image build-rv-local-disk run-rv-qemu

run-la-diagnostic: LA_FS_IMG = $(LA_FINAL_FS_IMG)
run-la-diagnostic: LA_DISK_IMG = $(LA_DIAGNOSTIC_DISK_IMG)
run-la-diagnostic: AUX_FS_DIR = respos-diagnostic
run-la-diagnostic: LA_MEM = $(LA_DIAGNOSTIC_MEM)
run-la-diagnostic: LA_SMP = $(LA_DIAGNOSTIC_SMP)
run-la-diagnostic: LA_OUTPUT = $(LA_DIAGNOSTIC_OUTPUT)
run-la-diagnostic: LA_KERNEL_FEATURE_ARGS = --features "$(LA_DIAGNOSTIC_KERNEL_FEATURES)"
run-la-diagnostic: build-la check-la-final-image build-la-local-disk run-la-qemu

# Software compatibility: mount the archived Alpine root under -snapshot and
# expose a deterministic smoke script through an interactive Alpine shell.
run-rv-software: RV_FS_IMG = $(RV_SOFTWARE_FS_IMG)
run-rv-software: RV_DISK_IMG = $(RV_SOFTWARE_DISK_IMG)
run-rv-software: AUX_FS_DIR = respos-software
run-rv-software: MEM = $(RV_SOFTWARE_MEM)
run-rv-software: SMP = $(RV_SOFTWARE_SMP)
run-rv-software: RV_OUTPUT = $(RV_SOFTWARE_OUTPUT)
run-rv-software: build-rv check-rv-software-image build-rv-local-disk run-rv-qemu

run-la-software: LA_FS_IMG = $(LA_SOFTWARE_FS_IMG)
run-la-software: LA_DISK_IMG = $(LA_SOFTWARE_DISK_IMG)
run-la-software: AUX_FS_DIR = respos-software
run-la-software: LA_MEM = $(LA_SOFTWARE_MEM)
run-la-software: LA_SMP = $(LA_SOFTWARE_SMP)
run-la-software: LA_OUTPUT = $(LA_SOFTWARE_OUTPUT)
run-la-software: build-la check-la-software-image build-la-local-disk run-la-qemu

# Bootstrap: clone RespOS through Git-over-SSH into the final root image and
# build the matching architecture. The private key only enters a /tmp runtime
# auxiliary image and is never copied into the repository or archived images.
run-rv-bootstrap: RV_FS_IMG = $(RV_FINAL_FS_IMG)
run-rv-bootstrap: RV_DISK_IMG = $(RV_BOOTSTRAP_DISK_IMG)
run-rv-bootstrap: MEM = $(RV_BOOTSTRAP_MEM)
run-rv-bootstrap: SMP = $(RV_BOOTSTRAP_SMP)
run-rv-bootstrap: RV_OUTPUT = $(RV_BOOTSTRAP_OUTPUT)
run-rv-bootstrap: build-rv check-rv-final-image build-rv-bootstrap-disk run-rv-qemu

run-la-bootstrap: LA_FS_IMG = $(LA_FINAL_FS_IMG)
run-la-bootstrap: LA_DISK_IMG = $(LA_BOOTSTRAP_DISK_IMG)
run-la-bootstrap: LA_MEM = $(LA_BOOTSTRAP_MEM)
run-la-bootstrap: LA_SMP = $(LA_BOOTSTRAP_SMP)
run-la-bootstrap: LA_OUTPUT = $(LA_BOOTSTRAP_OUTPUT)
run-la-bootstrap: build-la check-la-final-image build-la-bootstrap-disk run-la-qemu

# Backward-compatible aliases.  New scripts and documentation should use the
# explicit names above.
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
	@echo "Online submission:"
	@echo "  make all              build kernels and auto-detect submission disks"
	@echo "  make check-submit     rebuild and validate all four submission artifacts"
	@echo "Local preliminary suite (embedded testrunner):"
	@echo "  make prepare-pre-images"
	@echo "  make run-rv-pre"
	@echo "  make run-la-pre"
	@echo "Local final scoring scripts:"
	@echo "  make run-rv-final     default: 16 GiB / 8 harts"
	@echo "  make run-la-final     default: 36 GiB / 12 harts"
	@echo "Interactive diagnostics:"
	@echo "  make run-rv-diagnostic"
	@echo "  make run-la-diagnostic"
	@echo "Alpine software compatibility:"
	@echo "  make run-rv-software  default: 4 GiB / 2 harts"
	@echo "  make run-la-software  default: 4 GiB / 2 harts; repairs only a /tmp copy"
	@echo "Git-over-SSH clone and self-build (requires BOOTSTRAP_SSH_KEY):"
	@echo "  make run-rv-bootstrap default: 8 GiB / 4 harts"
	@echo "  make run-la-bootstrap default: 8 GiB / 4 harts"

clean:
	rm -f $(KERNEL_RV) $(KERNEL_LA) $(KERNEL_VF2) $(SUBMIT_RV_DISK_IMG) $(SUBMIT_LA_DISK_IMG)
	$(MAKE) -C os clean
