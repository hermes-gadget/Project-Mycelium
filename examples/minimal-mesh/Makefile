CXX = g++
CXXFLAGS = -std=c++17 -fPIC -Wall -Wextra -I../../core/bridge/include
LDFLAGS = -shared

MESHCORE_INC = -I../../lib/meshcore/src

.PHONY: all clean

all: firmware.so

firmware.so: firmware_entry.cpp
	$(CXX) $(CXXFLAGS) $(MESHCORE_INC) $< $(LDFLAGS) -o $@

clean:
	rm -f firmware.so
