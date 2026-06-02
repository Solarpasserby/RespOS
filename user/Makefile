# 架构选择: riscv64 | loongarch64
ARCH ?= riscv64
MODE ?= debug
FEATURES ?=
APP_DIR := src/bin

ifeq ($(ARCH),riscv64)
TARGET := riscv64gc-unknown-none-elf
OBJDUMP := rust-objdump --arch-name=riscv64
OBJCOPY := rust-objcopy --binary-architecture=riscv64
CARGO_CONFIG := cargo/config-riscv64.toml
else ifeq ($(ARCH),loongarch64)
TARGET := loongarch64-unknown-none
OBJDUMP := rust-objdump --arch-name=loongarch64
OBJCOPY := rust-objcopy --binary-architecture=loongarch64
CARGO_CONFIG := cargo/config-loongarch64.toml
else
	$(error Unsupported ARCH '$(ARCH)'. Use ARCH=riscv64 or ARCH=loongarch64)
endif

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

ifneq ($(strip $(FEATURES)),)
	CARGO_FEATURE_ARGS := --features "$(FEATURES)"
endif

TARGET_DIR := target/$(TARGET)/$(CARGO_TARGET_DIR)
APPS := $(wildcard $(APP_DIR)/*.rs)
ELFS := $(patsubst $(APP_DIR)/%.rs, $(TARGET_DIR)/%, $(APPS))
BINS := $(patsubst $(APP_DIR)/%.rs, $(TARGET_DIR)/%.bin, $(APPS))

prepare-cargo-config:
	@mkdir -p .cargo
	@cp $(CARGO_CONFIG) .cargo/config.toml

elf: prepare-cargo-config
	@cargo build $(CARGO_BUILD_ARG) $(CARGO_FEATURE_ARGS)

binary: elf
	@$(foreach elf, $(ELFS), $(OBJCOPY) $(elf) --strip-all -O binary $(patsubst $(TARGET_DIR)/%, $(TARGET_DIR)/%.bin, $(elf));)

build: binary

build-debug:
	@$(MAKE) build ARCH=$(ARCH) MODE=debug

build-release:
	@$(MAKE) build ARCH=$(ARCH) MODE=release

build-release-debug:
	@$(MAKE) build ARCH=$(ARCH) MODE=release-debug

clean:
	@cargo clean

.PHONY: prepare-cargo-config elf binary build build-debug build-release build-release-debug clean
