# 目标平台和构建模式
TARGET := riscv64gc-unknown-none-elf
MODE := release
KERNEL_ELF := target/$(TARGET)/$(MODE)/os
KERNEL_BIN := $(KERNEL_ELF).bin
DISASM_TMP := target/$(TARGET)/$(MODE)/asm

# 构建参数
ifeq ($(MODE), release)
	MODE_ARG := --release
endif

# 底层开发环境
BOARD := qemu
SBI ?= rustsbi
BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin

# 内核入口地址
KERNEL_ENTRY_PA := 0x80200000

# 工具链
OBJDUMP := rust-objdump --arch-name=riscv64
OBJCOPY := rust-objcopy --binary-architecture=riscv64

# # 反汇编？
# DISASM ?= -x

build: $(KERNEL_BIN)

# 检查编译环境
env:
	(rustup target list | grep "riscv64gc-unknown-none-elf (installed)") || rustup target add $(TARGET)
	cargo install cargo-binutils
	rustup component add rust-src
	rustup component add llvm-tools-preview

$(KERNEL_BIN): kernel
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $@

kernel:
	@cd ../user && make build
	@cargo build $(MODE_ARG)

clean:
	@cargo clean

# disasm: kernel
# 	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) | less

# disasm-vim: kernel
# 	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) > $(DISASM_TMP)
# 	@vim $(DISASM_TMP)
# 	@rm $(DISASM_TMP)

QEMU_ARGS := -machine virt \
			 -nographic \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA)

QEMU_NAME := qemu-system-riscv64

run: build
	@qemu-system-riscv64 $(QEMU_ARGS)

debug: build
	@tmux new-session -d \
		"qemu-system-riscv64 $(QEMU_ARGS) -s -S" && \
		tmux split-window -h "riscv64-unknown-elf-gdb -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'" && \
		tmux -2 attach-session -d

gdbserver: build
	@qemu-system-riscv64 $(QEMU_ARGS) -s -S

gdbclient:
	@riscv64-unknown-elf-gdb -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234