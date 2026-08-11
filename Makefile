SHELL := /bin/bash
.SHELLFLAGS := -o pipefail -c

MODE ?= release
RV_MODE ?= $(MODE)
LA_MODE ?= $(MODE)
MEM ?= 4G
SMP ?= 1
RV_FS_IMG ?= img/sdcard-rv.img
LA_FS_IMG ?= img/sdcard-la.img
PUB_INTERACTIVE_MEM ?= 4G
PUB_INTERACTIVE_SMP ?= 1
RV_PUB_FS_IMG ?= img/sdcard-rv-pub.img
LA_PUB_FS_IMG ?= img/sdcard-la-pub.img
RV_DISK_IMG ?= disk.img
LA_DISK_IMG ?= disk-la.img
AUX_FS_DIR ?= respos
AUX_FS_SIZE ?= 16M
QEMU_RV ?= qemu-system-riscv64
QEMU_LA ?= qemu-system-loongarch64

RV_TARGET := riscv64gc-unknown-none-elf
LA_TARGET := loongarch64-unknown-none

RV_OUTPUT ?= rv-output.txt
LA_OUTPUT ?= la-output.txt
RV_PUB_OUTPUT ?= /tmp/respos-rv-pub-output.txt
LA_PUB_OUTPUT ?= /tmp/respos-la-pub-output.txt
RV_USER_FEATURES ?= eval
LA_USER_FEATURES ?= eval
RV_KERNEL_FEATURES ?=
LA_KERNEL_FEATURES ?=

ifneq ($(strip $(RV_KERNEL_FEATURES)),)
	RV_KERNEL_FEATURE_ARGS := --features "$(RV_KERNEL_FEATURES)"
endif
ifneq ($(strip $(LA_KERNEL_FEATURES)),)
	LA_KERNEL_FEATURE_ARGS := --features "$(LA_KERNEL_FEATURES)"
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

KERNEL_RV := kernel-rv
KERNEL_LA := kernel-la
RV_ELF := os/target/$(RV_TARGET)/$(RV_CARGO_TARGET_DIR)/os
LA_ELF := os/target/$(LA_TARGET)/$(LA_CARGO_TARGET_DIR)/os

RV_QEMU_DISK_ARGS := -drive file=$(RV_DISK_IMG),if=none,format=raw,id=x1 \
	-device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1

LA_QEMU_DISK_ARGS := -drive file=$(LA_DISK_IMG),if=none,format=raw,id=x1 \
	-device virtio-blk-pci,drive=x1

.PHONY: all build-rv build-la build-disks force-build-disk rv la run-rv-pub run-la-pub \
	check-pub-images prepare-rv-cargo-config prepare-la-cargo-config clean check-submit

all: build-rv build-la build-disks

build-disks: $(RV_DISK_IMG) $(LA_DISK_IMG)

force-build-disk:

$(RV_DISK_IMG) $(LA_DISK_IMG): force-build-disk $(AUX_FS_DIR)/profile
	truncate -s $(AUX_FS_SIZE) $@
	mkfs.ext4 -q -F -d $(AUX_FS_DIR) $@

prepare-rv-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-riscv64.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

prepare-la-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-loongarch64.toml os/.cargo/config.toml
	cp user/cargo/config-loongarch64.toml user/.cargo/config.toml

build-rv: prepare-rv-cargo-config
	$(MAKE) -C user build ARCH=riscv64 MODE=$(RV_MODE) FEATURES=$(RV_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(RV_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(RV_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(RV_CARGO_BUILD_ARG) $(RV_KERNEL_FEATURE_ARGS)
	rust-objcopy --set-start=0x80200000 $(RV_ELF) $(KERNEL_RV)
	@rust-readobj -h -l $(KERNEL_RV) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

build-la: prepare-la-cargo-config
	$(MAKE) -C user build ARCH=loongarch64 MODE=$(LA_MODE) FEATURES=$(LA_USER_FEATURES)
	cd os && RESPOS_USER_PROFILE_DIR=$(LA_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(LA_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(LA_CARGO_BUILD_ARG) $(LA_KERNEL_FEATURE_ARGS)
	cp $(LA_ELF) $(KERNEL_LA)
	@rust-readobj -h -l $(KERNEL_LA) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

rv: build-rv build-disks
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
		-netdev user,id=net \
		-rtc base=utc \
		$(RV_QEMU_DISK_ARGS) |& tee $(RV_OUTPUT)

la: build-la build-disks
	$(QEMU_LA) -machine virt \
		-kernel $(KERNEL_LA) \
		-m $(MEM) \
		-nographic \
		-snapshot \
		-smp $(SMP) \
		-drive file=$(LA_FS_IMG),if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 \
		-no-reboot \
		-device virtio-net-pci,netdev=net0 \
		-netdev user,id=net0,hostfwd=tcp::5555-:5555,hostfwd=udp::5555-:5555 \
		-rtc base=utc \
		$(LA_QEMU_DISK_ARGS) |& tee $(LA_OUTPUT)

# Pub-image interactive targets. The user `initproc` executes testrunner only
# when the user crate is built with the `eval` feature. Clearing it makes
# initproc start the embedded user_shell, which is the first step for examining
# the pub images. Keep this path single-core until the kernel's SMP path is
# implemented and the final-round guest launcher is known.
run-rv-pub: RV_FS_IMG=$(RV_PUB_FS_IMG)
run-rv-pub: RV_USER_FEATURES=
run-rv-pub: MEM=$(PUB_INTERACTIVE_MEM)
run-rv-pub: SMP=$(PUB_INTERACTIVE_SMP)
run-rv-pub: RV_OUTPUT=$(RV_PUB_OUTPUT)
run-rv-pub: check-pub-images rv

run-la-pub: LA_FS_IMG=$(LA_PUB_FS_IMG)
run-la-pub: LA_USER_FEATURES=
run-la-pub: MEM=$(PUB_INTERACTIVE_MEM)
run-la-pub: SMP=$(PUB_INTERACTIVE_SMP)
run-la-pub: LA_OUTPUT=$(LA_PUB_OUTPUT)
run-la-pub: check-pub-images la

check-pub-images:
	@test -r "$(RV_PUB_FS_IMG)" || { echo "missing $(RV_PUB_FS_IMG)" >&2; exit 1; }
	@test -r "$(LA_PUB_FS_IMG)" || { echo "missing $(LA_PUB_FS_IMG)" >&2; exit 1; }
	@file "$(RV_PUB_FS_IMG)" "$(LA_PUB_FS_IMG)"

check-submit: all
	@file $(KERNEL_RV)
	@file $(KERNEL_LA)

clean:
	rm -f $(KERNEL_RV) $(KERNEL_LA) $(RV_DISK_IMG) $(LA_DISK_IMG)
	$(MAKE) -C os clean
