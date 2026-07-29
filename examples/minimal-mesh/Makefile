CXX = g++
CXXFLAGS = -std=c++17 -fPIC -Wall -Wextra \
	-I../../core/bridge/include \
	-isystem ../../lib/meshcore/test/mocks
LDFLAGS = -shared -L../../target/debug -lmeshemu_bridge \
	-Wl,-rpath,'$$ORIGIN/../../target/debug'

MESHCORE_INC = -I../../lib/meshcore/src
MESHCORE_SRC = \
	../../lib/meshcore/src/Dispatcher.cpp \
	../../lib/meshcore/src/Packet.cpp \
	../../lib/meshcore/src/helpers/StaticPoolPacketManager.cpp
BRIDGE_SRC = ../../core/bridge/src/meshemu_bridge_clock.cpp
SOURCES = firmware_entry.cpp $(MESHCORE_SRC) $(BRIDGE_SRC)

.PHONY: all bridge clean

all: firmware.so

bridge:
	cargo build --manifest-path ../../Cargo.toml -p meshemu_bridge

firmware.so: $(SOURCES) | bridge
	$(CXX) $(CXXFLAGS) $(MESHCORE_INC) $(SOURCES) $(LDFLAGS) -o $@

clean:
	rm -f firmware.so
