# dwarfstar-worker — the DwarfStar bee.
#
# The DwarfStar engine is a C library (antirez/ds4). C projects consume
# C libraries by compiling their sources into your own objects — no
# shared lib, no ABI, no version skew. This Makefile does that:
#
#   make            build build/dwarfstar-worker
#   make fetch      pull the pinned ds4 source into vendor/ds4
#   make clean      drop the build
#
# The source is pinned by commit. `make fetch` downloads it once; after
# that the build is offline. If you already have a ds4 checkout, point
# DS4_DIR at it and nothing is downloaded at all:
#
#   make DS4_DIR=~/src/ds4
#
# Pinned: antirez/ds4 @ 5379c1c9ac315f1540371b06a3db88491d4059ff
# Licence: MIT (see vendor/ds4/LICENSE). ds4 itself credits llama.cpp
# and GGML for the quantization formats and kernels it inherits.

DS4_REPO  ?= antirez/ds4
DS4_SHA   ?= 5379c1c9ac315f1540371b06a3db88491d4059ff
DS4_URL   ?= https://codeload.github.com/$(DS4_REPO)/tar.gz/$(DS4_SHA)

DS4_DIR   ?= vendor/ds4
BUILD     ?= out
BIN       ?= $(BUILD)/dwarfstar-bee

THRUM_DIR ?= ../../thrum-clients/c

CC        ?= cc
# The engine is compiled with the flags ds4's own Makefile uses, so the
# numerics match upstream. The .m file gets clang's default dialect
# (no -std), because it uses the GNU typeof extension.
CFLAGS    ?= -O3 -ffast-math -mcpu=native -Wall -Wextra -std=c99
OBJCFLAGS ?= -O3 -ffast-math -mcpu=native -Wall -Wextra -fobjc-arc
# The bee itself: strict C11, no fast-math games.
HIVE_CFLAGS ?= -O2 -std=c11 -Wall -Wextra -D_POSIX_C_SOURCE=200809L
LDFLAGS   ?=
LDLIBS    ?= -lm -pthread

UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
METAL_LDLIBS ?= $(LDLIBS) -framework Foundation -framework Metal
else
METAL_LDLIBS ?= $(LDLIBS)
endif

# The engine core: exactly the objects ds4's own Makefile links into
# every binary. Not the CLI, not the server, not the agent — the hive
# drives the engine itself.
DS4_CORE_SRC = ds4.c ds4_image.c ds4_distributed.c ds4_tp.c ds4_ssd.c ds4_layer_pack.c ds4_engram.c
DS4_CORE_OBJ = $(patsubst %.c,$(BUILD)/ds4/%.o,$(DS4_CORE_SRC))
DS4_METAL_OBJ = $(BUILD)/ds4/ds4_metal.o

HIVE_SRC = src/main.c src/ds4_bridge.c src/models.c src/shaders.c
CLIENT_SRC = $(THRUM_DIR)/thrum/json.c $(THRUM_DIR)/thrum/client.c

.PHONY: all fetch clean check test

all: $(BIN)

$(BIN): $(HIVE_SRC) $(CLIENT_SRC) $(DS4_CORE_OBJ) $(DS4_METAL_OBJ)
	@mkdir -p $(BUILD)
	$(CC) $(HIVE_CFLAGS) -I$(THRUM_DIR) -I$(DS4_DIR) -I$(DS4_DIR)/metal \
		-o $@ $(HIVE_SRC) $(CLIENT_SRC) $(DS4_CORE_OBJ) $(DS4_METAL_OBJ) \
		$(LDFLAGS) $(METAL_LDLIBS)
	@cp -R $(DS4_DIR)/metal $(BUILD)/metal
	@echo "built $@"

$(BUILD)/ds4/%.o: $(DS4_DIR)/%.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -I$(DS4_DIR) -I$(DS4_DIR)/metal -c -o $@ $<

$(BUILD)/ds4/ds4_metal.o: $(DS4_DIR)/ds4_metal.m
	@mkdir -p $(dir $@)
	$(CC) $(OBJCFLAGS) -I$(DS4_DIR) -I$(DS4_DIR)/metal -c -o $@ $<

$(BUILD)/thrum/%.o: $(THRUM_DIR)/thrum/%.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -I$(THRUM_DIR) -c -o $@ $<

# --- the engine source ------------------------------------------------------

fetch:
	@mkdir -p $(BUILD) $(DS4_DIR)
	@echo "fetching $(DS4_REPO) @ $(DS4_SHA)"
	@curl -fsSL "$(DS4_URL)" -o $(BUILD)/ds4.tar.gz
	@tar xzf $(BUILD)/ds4.tar.gz --strip-components=1 -C $(DS4_DIR)
	@rm -f $(BUILD)/ds4.tar.gz
	@echo "ds4 source in $(DS4_DIR)"

$(DS4_DIR)/ds4.h:
	@$(MAKE) --no-print-directory fetch

check: $(BIN)
	@echo "dwarfstar-bee speaks thrum to humd; point it at your models with DWARFSTAR_DIR"

$(BUILD)/wire_smoke: tests/wire_smoke.c $(CLIENT_SRC)
	@mkdir -p $(BUILD)
	$(CC) $(HIVE_CFLAGS) -I$(THRUM_DIR) -o $@ tests/wire_smoke.c $(CLIENT_SRC)

# Needs a humd on the socket: the smoke test asks it for one prompt and
# prints what comes back.
test: $(BIN) $(BUILD)/wire_smoke
	./$(BUILD)/wire_smoke

clean:
	rm -rf $(BUILD)
