.PHONY: build test test-sdk-headers abi-inventory fmt lint

ABI_TARGET_DIR ?= $(if $(CARGO_TARGET_DIR),$(CARGO_TARGET_DIR)/debug,target/debug)
ABI_LIBRARY := $(ABI_TARGET_DIR)/libmeshemu_bridge.so
ABI_INVENTORY_DIR ?= $(ABI_TARGET_DIR)/abi-inventory

build:
	cargo build --workspace

test:
	cargo test --workspace

test-sdk-headers: abi-inventory
	./firmware-sdk/tests/check_abi.sh

abi-inventory:
	@set -eu; \
	cargo build -p meshemu_bridge >/dev/null; \
	mkdir -p "$(ABI_INVENTORY_DIR)"; \
	nm -D --defined-only "$(ABI_LIBRARY)" \
	  | awk '$$3 ~ /^meshemu_/ { print $$3 }' \
	  | sort -u > "$(ABI_INVENTORY_DIR)/exports.txt"; \
	rg -o --no-filename 'meshemu_[[:alnum:]_]+[[:space:]]*\(' core/bridge/include \
	  --glob 'meshemu_bridge_*.h' \
	  --glob '!meshemu_bridge_clock.h' \
	  | sed -E 's/[[:space:]]*\($$//' \
	  | sort -u > "$(ABI_INVENTORY_DIR)/headers.txt"; \
	rg -o --no-filename 'meshemu_[[:alnum:]_]+[[:space:]]*\(' firmware-sdk/tests/abi_link.c \
	  | sed -E 's/[[:space:]]*\($$//' \
	  | sort -u > "$(ABI_INVENTORY_DIR)/abi-calls.txt"; \
	exports=$$(wc -l < "$(ABI_INVENTORY_DIR)/exports.txt"); \
	headers=$$(wc -l < "$(ABI_INVENTORY_DIR)/headers.txt"); \
	abi_calls=$$(wc -l < "$(ABI_INVENTORY_DIR)/abi-calls.txt"); \
	printf 'ABI inventory: exports=%s headers=%s calls=%s\n' "$$exports" "$$headers" "$$abi_calls"; \
	if ! cmp -s "$(ABI_INVENTORY_DIR)/exports.txt" "$(ABI_INVENTORY_DIR)/headers.txt"; then \
		echo 'Built exports and canonical headers differ:'; \
		diff -u "$(ABI_INVENTORY_DIR)/headers.txt" "$(ABI_INVENTORY_DIR)/exports.txt"; \
		exit 1; \
	fi; \
	if ! cmp -s "$(ABI_INVENTORY_DIR)/exports.txt" "$(ABI_INVENTORY_DIR)/abi-calls.txt"; then \
		echo 'Built exports and ABI calls differ:'; \
		diff -u "$(ABI_INVENTORY_DIR)/abi-calls.txt" "$(ABI_INVENTORY_DIR)/exports.txt"; \
		exit 1; \
	fi

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets -- -D warnings
