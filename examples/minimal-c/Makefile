# Builds examples/minimal-c/firmware.so against the meshemu_bridge cdylib.
#
# The firmware links the bridge shared library directly (like
# ../minimal-mesh/Makefile), so the emulator must be run from this directory
# for the $ORIGIN-relative rpath to resolve.

CC ?= gcc
CFLAGS = -std=c11 -fPIC -Wall -Wextra \
	-I../../firmware-sdk \
	-I../../firmware-sdk/include \
	-I../../core/bridge/include
LDFLAGS = -shared -L../../target/debug -lmeshemu_bridge \
	-Wl,-rpath,'$$ORIGIN/../../target/debug'

.PHONY: all bridge clean

all: firmware.so

bridge:
	cargo build --manifest-path ../../Cargo.toml -p meshemu_bridge

firmware.so: firmware.c | bridge
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ firmware.c

clean:
	rm -f firmware.so
