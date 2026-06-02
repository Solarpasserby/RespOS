# 架构选择: riscv64 | loongarch64
ARCH ?= riscv64
MODE ?= debug

ifeq ($(MODE),debug)
	CARGO_BUILD_ARG :=
	CARGO_TARGET_DIR := debug
else ifeq ($(MODE),release)
	CARGO_BUILD_ARG := --release
	CARGO_TARGET_DIR := release
else ifeq ($(MODE),release-debug)
	CARGO_BUILD_ARG := --profile release-debug
	CARGO_TARGET_DIR := release-debug
else
	$(error Unsupported MODE '$(MODE)'. Use MODE=debug, MODE=release, or MODE=release-debug)
endif

# 内核配置（必须在架构 QEMU_ARGS 之前，因为 QEMU_ARGS 使用 := 展开）
MEM ?= 128M
SMP ?= 1
IMG_SCRIPT := ../scripts/get_img.sh

# ---- RISC-V 64 配置 ----
ifeq ($(ARCH),riscv64)
TARGET := riscv64gc-unknown-none-elf
GDB ?= riscv64-unknown-elf-gdb
BOOTLOADER := ../bootloader/rustsbi-qemu.bin
KERNEL_ENTRY_PA := 0x80200000
OBJDUMP := rust-objdump --arch-name=riscv64
OBJCOPY := rust-objcopy --binary-architecture=riscv64
QEMU_NAME := qemu-system-riscv64
QEMU_MACHINE := virt
FS_IMG ?= ../img/sdcard-rv.img
DISK_IMG ?= disk.img
CARGO_CONFIG := cargo/config-riscv64.toml
USER_ARCH := riscv64
USER_TARGET := riscv64gc-unknown-none-elf
USER_CARGO_CONFIG := ../user/cargo/config-riscv64.toml

QEMU_BLOCK_ARGS := -drive file=$(FS_IMG),if=none,format=raw,id=x0 \
				   -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
ifneq ($(wildcard $(DISK_IMG)),)
	QEMU_BLOCK_ARGS += -drive file=$(DISK_IMG),if=none,format=raw,id=x1 \
					   -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
endif

QEMU_ARGS = -machine $(QEMU_MACHINE) \
			 -m $(MEM) \
			 -nographic \
			 -smp $(SMP) \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 $(QEMU_BLOCK_ARGS) \
			 -no-reboot \
			 -rtc base=utc

# ---- LoongArch 64 配置 ----
else ifeq ($(ARCH),loongarch64)
TARGET := loongarch64-unknown-none
GDB ?= loongarch64-unknown-linux-gnu-gdb
KERNEL_ENTRY_PA := 0x00200000
OBJDUMP := rust-objdump --arch-name=loongarch64
OBJCOPY := rust-objcopy --binary-architecture=loongarch64
QEMU_NAME := qemu-system-loongarch64
QEMU_MACHINE := virt
# LA64 内核由 QEMU virt boot code 通过 -kernel 加载到 RAM。
BOOTLOADER :=
FS_IMG ?= ../img/sdcard-la.img
DISK_IMG ?=
CARGO_CONFIG := cargo/config-loongarch64.toml
USER_ARCH := loongarch64
USER_TARGET := loongarch64-unknown-none
USER_CARGO_CONFIG := ../user/cargo/config-loongarch64.toml

QEMU_BLOCK_ARGS :=
ifneq ($(wildcard $(FS_IMG)),)
	QEMU_BLOCK_ARGS += -drive file=$(FS_IMG),if=none,format=raw,id=x0 \
					   -device virtio-blk-pci,drive=x0
endif

QEMU_ARGS = -machine $(QEMU_MACHINE) \
			 -m $(MEM) \
			 -nographic \
			 -smp $(SMP) \
			 -kernel $(KERNEL_ELF) \
			 $(QEMU_BLOCK_ARGS) \
			 -no-reboot \
			 -rtc base=utc

else
$(error Unsupported ARCH '$(ARCH)'. Use ARCH=riscv64 or ARCH=loongarch64)
endif

KERNEL_ELF := target/$(TARGET)/$(CARGO_TARGET_DIR)/os
KERNEL_BIN := $(KERNEL_ELF).bin
DISASM_TMP := target/$(TARGET)/$(CARGO_TARGET_DIR)/asm

build: prepare-cargo-config $(KERNEL_BIN)

build-debug:
	@$(MAKE) build MODE=debug

build-release:
	@$(MAKE) build MODE=release

build-release-debug:
	@$(MAKE) build MODE=release-debug

img image:
	@$(IMG_SCRIPT)

# 检查编译环境
env:
	@echo "Checking environment for ARCH=$(ARCH)..."
	(rustup target list | grep "$(TARGET) (installed)") || rustup target add $(TARGET)
	@command -v rust-objcopy >/dev/null || command -v rust-objdump >/dev/null || \
		(echo "Missing rust binutils; installing cargo-binutils..." && cargo install cargo-binutils)
	rustup component add rust-src
	rustup component add llvm-tools-preview

$(KERNEL_BIN): kernel
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $@

prepare-cargo-config:
	@mkdir -p .cargo
	@cp $(CARGO_CONFIG) .cargo/config.toml
	@mkdir -p ../user/.cargo
	@cp $(USER_CARGO_CONFIG) ../user/.cargo/config.toml

kernel: prepare-cargo-config
	@cd ../user && make build ARCH=$(USER_ARCH) MODE=$(MODE)
	@PATH="$$HOME/.local/bin:$$PATH" RESPOS_USER_TARGET=$(USER_TARGET) \
		RESPOS_USER_PROFILE_DIR=$(CARGO_TARGET_DIR) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(CARGO_BUILD_ARG)

clean:
	@cargo clean

quick-run:
	@$(QEMU_NAME) $(QEMU_ARGS)

run: build quick-run

run-debug:
	@$(MAKE) run MODE=debug

run-release:
	@$(MAKE) run MODE=release

run-release-debug:
	@$(MAKE) run MODE=release-debug

debug: build
	@tmux new-session -d \
		"$(QEMU_NAME) $(QEMU_ARGS) -s -S" && \
		tmux split-window -h "$(GDB) -ex 'file $(KERNEL_ELF)' -ex 'target remote localhost:1234'" && \
		tmux -2 attach-session -d

debug-release:
	@$(MAKE) debug MODE=release

debug-release-debug:
	@$(MAKE) debug MODE=release-debug

gdbserver: build
	@$(QEMU_NAME) $(QEMU_ARGS) -s -S

gdbserver-debug:
	@$(MAKE) gdbserver MODE=debug

gdbserver-release:
	@$(MAKE) gdbserver MODE=release

gdbserver-release-debug:
	@$(MAKE) gdbserver MODE=release-debug

gdbclient:
	@$(GDB) -ex 'file $(KERNEL_ELF)' -ex 'target remote localhost:1234'

.PHONY: build build-debug build-release build-release-debug img image env prepare-cargo-config kernel clean quick-run run run-debug run-release run-release-debug debug debug-release debug-release-debug gdbserver gdbserver-debug gdbserver-release gdbserver-release-debug gdbclient
