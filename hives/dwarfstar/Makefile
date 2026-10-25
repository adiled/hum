# dwarfstar: the DwarfStar engine (antirez/ds4), linked into a hum worker.
#
#   make           out/dwarfstar-bee and the Metal kernels beside it
#   make check     out/dwarfstar-bee-stub: the bee with no engine, for the
#                  thrum side, on any machine
#   make fetch     the pinned ds4 source into vendor/ds4
#   make clean     drop out/
#
# ds4 is a C library, and a C program takes a C library by compiling its
# sources into its own objects: exactly the objects ds4's own Makefile
# links into every binary on macOS, with its flags, so the numerics match
# upstream. The source is pinned by commit and fetched once; point DS4_DIR
# at a checkout you already have and nothing is fetched. The engine's
# Metal backend is what is wired, so the bee builds on macOS.
#
# The thrum side is the C client in thrum-clients/c, through thrum.mk,
# which brings cJSON in through pkg-config.

DS4_REPO ?= antirez/ds4
DS4_SHA  ?= 5379c1c9ac315f1540371b06a3db88491d4059ff
DS4_URL  ?= https://codeload.github.com/$(DS4_REPO)/tar.gz/$(DS4_SHA)
DS4_DIR  ?= vendor/ds4

BUILD := out
BIN   := $(BUILD)/dwarfstar-bee
STUB  := $(BUILD)/dwarfstar-bee-stub

THRUM_BUILD := $(BUILD)/thrum-c
include ../../thrum-clients/c/thrum.mk

DS4_CFLAGS    ?= -O3 -ffast-math -g -mcpu=native -Wall -Wextra -std=c99
DS4_OBJCFLAGS ?= -O3 -ffast-math -g -mcpu=native -Wall -Wextra -fobjc-arc
DS4_LDLIBS    := -lm -pthread -framework Foundation -framework Metal
DS4_CORE      := ds4 ds4_image ds4_distributed ds4_tp ds4_ssd ds4_layer_pack ds4_engram
DS4_SOURCES   := $(DS4_CORE:%=$(DS4_DIR)/%.c) $(DS4_DIR)/ds4_metal.m
DS4_OBJ       := $(DS4_CORE:%=$(BUILD)/ds4/%.o) $(BUILD)/ds4/ds4_metal.o

BEE_CFLAGS ?= -std=c11 -O2 -g -Wall -Wextra -Wpedantic -Wshadow
BEE_HDR    := $(wildcard src/*.h) $(THRUM_HDR)

.PHONY: all check fetch clean darwin

all: $(BIN)

darwin:
	@test "$$(uname -s)" = Darwin || { \
		echo "dwarfstar links ds4's Metal engine, which needs macOS; make check builds the rest anywhere" >&2; \
		exit 1; }

$(BIN): $(BUILD)/main.o $(BUILD)/models.o $(BUILD)/gpu.o $(BUILD)/shaders.o $(BUILD)/ds4_bridge.o $(DS4_OBJ) $(THRUM_LIB) | darwin
	$(CC) -o $@ $(filter %.o,$^) $(THRUM_LIBS) $(DS4_LDLIBS)
	@rm -rf $(BUILD)/metal && cp -R $(DS4_DIR)/metal $(BUILD)/metal
	@echo "built $@"

check: $(STUB)

$(STUB): $(BUILD)/main.o $(BUILD)/models.o $(BUILD)/gpu.o $(BUILD)/stub/ds4_stub.o $(THRUM_LIB)
	$(CC) -o $@ $(filter %.o,$^) $(THRUM_LIBS)
	@echo "built $@"

$(BUILD)/main.o $(BUILD)/models.o $(BUILD)/gpu.o $(BUILD)/shaders.o: $(BUILD)/%.o: src/%.c $(BEE_HDR)
	@mkdir -p $(dir $@)
	$(CC) $(BEE_CFLAGS) $(THRUM_CFLAGS) -c -o $@ $<

$(BUILD)/stub/ds4_stub.o: src/ds4_stub.c $(BEE_HDR)
	@mkdir -p $(dir $@)
	$(CC) $(BEE_CFLAGS) -c -o $@ $<

$(BUILD)/ds4_bridge.o: src/ds4_bridge.c $(BEE_HDR) $(DS4_DIR)/ds4.h | darwin
	@mkdir -p $(dir $@)
	$(CC) $(BEE_CFLAGS) $(THRUM_CFLAGS) -I$(DS4_DIR) -c -o $@ $<

$(BUILD)/ds4/%.o: $(DS4_DIR)/%.c | darwin
	@mkdir -p $(dir $@)
	$(CC) $(DS4_CFLAGS) -I$(DS4_DIR) -I$(DS4_DIR)/metal -c -o $@ $<

$(BUILD)/ds4/ds4_metal.o: $(DS4_DIR)/ds4_metal.m | darwin
	@mkdir -p $(dir $@)
	$(CC) $(DS4_OBJCFLAGS) -I$(DS4_DIR) -I$(DS4_DIR)/metal -c -o $@ $<

$(DS4_SOURCES): | $(DS4_DIR)/ds4.h ;

$(DS4_DIR)/ds4.h:
	@$(MAKE) --no-print-directory fetch

fetch:
	@mkdir -p $(BUILD) $(DS4_DIR)
	@echo "fetching $(DS4_REPO) @ $(DS4_SHA)"
	@curl -fsSL "$(DS4_URL)" -o $(BUILD)/ds4.tar.gz
	@tar xzf $(BUILD)/ds4.tar.gz --strip-components=1 -C $(DS4_DIR)
	@rm -f $(BUILD)/ds4.tar.gz
	@echo "ds4 source in $(DS4_DIR)"

clean:
	rm -rf $(BUILD)
