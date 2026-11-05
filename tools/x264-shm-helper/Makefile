CC ?= cc
PKG_CONFIG ?= pkg-config
CFLAGS ?= -O2
OUTPUT ?= coreav-x264-shm
CPPFLAGS += $(shell $(PKG_CONFIG) --cflags x264 2>/dev/null)
LDLIBS += $(shell $(PKG_CONFIG) --libs x264 2>/dev/null)

.PHONY: all test clean check-library
all: $(OUTPUT)

check-library:
	@$(PKG_CONFIG) --exists x264 || { printf '%s\n' 'Install libx264 and set PKG_CONFIG_PATH before building this operator-only helper.' >&2; exit 1; }

$(OUTPUT): main.c | check-library
	$(CC) $(CPPFLAGS) $(CFLAGS) -std=c11 -Wall -Wextra -Werror $< $(LDFLAGS) $(LDLIBS) -o $@

test: $(OUTPUT)
	python3 test_helper.py --helper $(abspath $(OUTPUT))

clean:
	rm -f $(OUTPUT)
