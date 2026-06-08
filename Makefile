MODE ?= debug
TARGET := riscv64gc-unknown-none-elf
MEM ?= 128M
SMP ?= 1
FS_IMG ?= img/sdcard-rv.img
DISK_IMG ?= disk.img
QEMU ?= qemu-system-riscv64
TESTRUNNER_LOG ?= testrunner_output.log

ifeq ($(MODE),debug)
	CARGO_TARGET_DIR := debug
	CARGO_BUILD_ARG :=
else ifeq ($(MODE),release)
	CARGO_TARGET_DIR := release
	CARGO_BUILD_ARG := --release
else ifeq ($(MODE),release-debug)
	CARGO_TARGET_DIR := release-debug
	CARGO_BUILD_ARG := --profile release-debug
else
	$(error Unsupported MODE '$(MODE)'. Use MODE=debug, MODE=release, or MODE=release-debug)
endif

KERNEL_RV := kernel-rv
RV_ELF := os/target/$(TARGET)/$(CARGO_TARGET_DIR)/os
QEMU_DISK_ARGS :=
ifneq ($(wildcard $(DISK_IMG)),)
QEMU_DISK_ARGS += -drive file=$(DISK_IMG),if=none,format=raw,id=x1 \
	-device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
endif

.PHONY: all build-rv rv prepare-cargo-config clean check-submit

all: build-rv

prepare-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config-riscv64.toml os/.cargo/config.toml
	cp user/cargo/config-riscv64.toml user/.cargo/config.toml

build-rv: prepare-cargo-config
	$(MAKE) -C user build MODE=$(MODE) FEATURES=eval
	cd os && RESPOS_USER_PROFILE_DIR=$(CARGO_TARGET_DIR) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(CARGO_BUILD_ARG)
	rust-objcopy --set-start=0x80200000 $(RV_ELF) $(KERNEL_RV)
	@rust-readobj -h -l $(KERNEL_RV) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

rv: build-rv
	$(QEMU) -machine virt \
		-kernel $(KERNEL_RV) \
		-m $(MEM) \
		-nographic \
		-smp $(SMP) \
		-bios default \
		-drive file=$(FS_IMG),if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-no-reboot \
		-device virtio-net-device,netdev=net \
		-netdev user,id=net \
		-rtc base=utc \
		$(QEMU_DISK_ARGS) 2>&1 | tee $(TESTRUNNER_LOG)

check-submit: build-rv
	@file $(KERNEL_RV)

clean:
	rm -f $(KERNEL_RV)
	$(MAKE) -C os clean
