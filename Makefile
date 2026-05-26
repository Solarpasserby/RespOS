MODE ?= debug
TARGET := riscv64gc-unknown-none-elf

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

.PHONY: all rv prepare-cargo-config clean check-submit

all: rv

prepare-cargo-config:
	mkdir -p os/.cargo user/.cargo
	cp os/cargo/config.toml os/.cargo/config.toml
	cp user/cargo/config.toml user/.cargo/config.toml

rv: prepare-cargo-config
	$(MAKE) -C user build MODE=$(MODE) FEATURES=eval
	cd os && RESPOS_USER_PROFILE_DIR=$(CARGO_TARGET_DIR) \
		RESPOS_APP_REBUILD_STAMP=$$(date +%s%N) cargo build $(CARGO_BUILD_ARG)
	rust-objcopy --set-start=0x80200000 $(RV_ELF) $(KERNEL_RV)
	@rust-readobj -h -l $(KERNEL_RV) | awk '/Entry:/ || /VirtualAddress:/ || /PhysicalAddress:/ { print }'

check-submit: rv
	@file $(KERNEL_RV)

clean:
	rm -f $(KERNEL_RV)
	$(MAKE) -C os clean
