SHELL := /bin/bash
.SHELLFLAGS := -o pipefail -c

MODE ?= release
RV_MODE ?= $(MODE)
LA_MODE ?= $(MODE)
MEM ?= 128M
SMP ?= 1
RV_FS_IMG ?= img/sdcard-rv.img
LA_FS_IMG ?= img/sdcard-la.img
LTP_IMAGE_SOURCE_DIR ?= /coursegrader/testdata
RV_DISK_IMG ?= disk.img
LA_DISK_IMG ?= disk-la.img
QEMU_RV ?= qemu-system-riscv64
QEMU_LA ?= qemu-system-loongarch64

RV_TARGET := riscv64gc-unknown-none-elf
LA_TARGET := loongarch64-unknown-none

RV_OUTPUT ?= rv-output.txt
LA_OUTPUT ?= la-output.txt

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

RV_QEMU_DISK_ARGS :=
ifneq ($(wildcard $(RV_DISK_IMG)),)
RV_QEMU_DISK_ARGS += -drive file=$(RV_DISK_IMG),if=none,format=raw,id=x1 \
	-device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
endif

LA_QEMU_DISK_ARGS :=
ifneq ($(wildcard $(LA_DISK_IMG)),)
LA_QEMU_DISK_ARGS += -drive file=$(LA_DISK_IMG),if=none,format=raw,id=x1 \
	-device virtio-blk-pci,drive=x1
endif

.PHONY: all build-rv build-la patch-rv-ltp-image patch-la-ltp-image rv la prepare-rv-cargo-config prepare-la-cargo-config clean check-submit

all: build-rv build-la

prepare-rv-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-riscv64.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

prepare-la-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-loongarch64.toml os/.cargo/config.toml
	cp user/cargo/config-loongarch64.toml user/.cargo/config.toml

build-rv: prepare-rv-cargo-config
	$(MAKE) -C user build ARCH=riscv64 MODE=$(RV_MODE) FEATURES=eval
	cd os && RESPOS_USER_PROFILE_DIR=$(RV_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(RV_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(RV_CARGO_BUILD_ARG)
	rust-objcopy --set-start=0x80200000 $(RV_ELF) $(KERNEL_RV)
	@rust-readobj -h -l $(KERNEL_RV) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

build-la: prepare-la-cargo-config
	$(MAKE) -C user build ARCH=loongarch64 MODE=$(LA_MODE) FEATURES=eval
	cd os && RESPOS_USER_PROFILE_DIR=$(LA_CARGO_TARGET_DIR) \
		RESPOS_USER_TARGET=$(LA_TARGET) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(LA_CARGO_BUILD_ARG)
	cp $(LA_ELF) $(KERNEL_LA)
	@rust-readobj -h -l $(KERNEL_LA) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

patch-rv-ltp-image:
	COURSEGRADER_TESTDATA=$(LTP_IMAGE_SOURCE_DIR) ./scripts/patch_ltp_image.sh $(RV_FS_IMG)

patch-la-ltp-image:
	COURSEGRADER_TESTDATA=$(LTP_IMAGE_SOURCE_DIR) ./scripts/patch_ltp_image.sh $(LA_FS_IMG)

rv: build-rv patch-rv-ltp-image
	$(QEMU_RV) -machine virt \
		-kernel $(KERNEL_RV) \
		-m $(MEM) \
		-nographic \
		-smp $(SMP) \
		-bios default \
		-drive file=$(RV_FS_IMG),if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-no-reboot \
		-device virtio-net-device,netdev=net \
		-netdev user,id=net \
		-rtc base=utc \
		$(RV_QEMU_DISK_ARGS) |& tee $(RV_OUTPUT)

la: build-la patch-la-ltp-image
	$(QEMU_LA) -machine virt \
		-kernel $(KERNEL_LA) \
		-m $(MEM) \
		-nographic \
		-smp $(SMP) \
		-drive file=$(LA_FS_IMG),if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 \
		-no-reboot \
		-device virtio-net-pci,netdev=net0 \
		-netdev user,id=net0,hostfwd=tcp::5555-:5555,hostfwd=udp::5555-:5555 \
		-rtc base=utc \
		$(LA_QEMU_DISK_ARGS) |& tee $(LA_OUTPUT)

check-submit: all
	@file $(KERNEL_RV)
	@file $(KERNEL_LA)

clean:
	rm -f $(KERNEL_RV) $(KERNEL_LA)
	$(MAKE) -C os clean
