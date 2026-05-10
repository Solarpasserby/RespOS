# 目标平台和构建模式
TARGET := riscv64gc-unknown-none-elf
MODE ?= debug

GDB ?= riscv64-unknown-elf-gdb

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

KERNEL_ELF := target/$(TARGET)/$(CARGO_TARGET_DIR)/os
KERNEL_BIN := $(KERNEL_ELF).bin
DISASM_TMP := target/$(TARGET)/$(CARGO_TARGET_DIR)/asm

# 底层开发环境
BOARD := qemu
SBI ?= rustsbi
BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin

# 内核入口地址
KERNEL_ENTRY_PA := 0x80200000

# 工具链
OBJDUMP := rust-objdump --arch-name=riscv64
OBJCOPY := rust-objcopy --binary-architecture=riscv64
QEMU_NAME := qemu-system-riscv64

# 内核配置
MEM ?= 128M
SMP ?= 1
FS_IMG ?= ../img/sdcard-rv.img
DISK_IMG ?= disk.img

# # 反汇编？
# DISASM ?= -x

build: $(KERNEL_BIN)

build-debug:
	@$(MAKE) build MODE=debug

build-release:
	@$(MAKE) build MODE=release

build-release-debug:
	@$(MAKE) build MODE=release-debug

# 检查编译环境
env:
	(rustup target list | grep "riscv64gc-unknown-none-elf (installed)") || rustup target add $(TARGET)
	@command -v rust-objcopy >/dev/null || command -v rust-objdump >/dev/null || \
		(echo "Missing rust binutils; installing cargo-binutils..." && cargo install cargo-binutils)
	rustup component add rust-src
	rustup component add llvm-tools-preview
	@command -v $(GDB) >/dev/null || (echo "Missing GDB command: $(GDB)" && exit 1)
	@test -x "$$(command -v $(GDB))" || (echo "GDB is not executable: $$(command -v $(GDB))" && exit 1)

$(KERNEL_BIN): kernel
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $@

kernel:
	@cd ../user && make build MODE=$(MODE)
	@cargo build $(CARGO_BUILD_ARG)

clean:
	@cargo clean

# disasm: kernel
# 	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) | less

# disasm-vim: kernel
# 	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) > $(DISASM_TMP)
# 	@vim $(DISASM_TMP)
# 	@rm $(DISASM_TMP)

QEMU_BLOCK_ARGS := -drive file=$(FS_IMG),if=none,format=raw,id=x0 \
				   -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0

ifneq ($(wildcard $(DISK_IMG)),)
	QEMU_BLOCK_ARGS += -drive file=$(DISK_IMG),if=none,format=raw,id=x1 \
					   -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1
endif

QEMU_ARGS := -machine virt \
			 -m $(MEM) \
			 -nographic \
			 -smp $(SMP) \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 $(QEMU_BLOCK_ARGS) \
			 -no-reboot \
			 -rtc base=utc

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
		tmux split-window -h "$(GDB) -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'" && \
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
	@$(GDB) -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'
