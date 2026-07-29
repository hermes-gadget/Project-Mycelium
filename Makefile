.PHONY: build test test-sdk-headers fmt lint

build:
	cargo build --workspace

test:
	cargo test --workspace

test-sdk-headers:
	./firmware-sdk/tests/check_abi.sh

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets -- -D warnings
