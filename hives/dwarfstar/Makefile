DS4_REPO ?= antirez/ds4
DS4_SHA  ?= 5379c1c9ac315f1540371b06a3db88491d4059ff
DS4_URL  ?= https://codeload.github.com/$(DS4_REPO)/tar.gz/$(DS4_SHA)
DS4_DIR  ?= vendor/ds4

BUILD := out
BIN   := $(BUILD)/dwarfstar-bee
STUB  := $(BUILD)/dwarfstar-bee-stub
TESTS := $(BUILD)/test-chat
CONFORM := $(BUILD)/conform-with-ds4

THRUM_BUILD := $(BUILD)/thrum-c
include ../../thrum-clients/c/thrum.mk

DS4_CFLAGS    ?= -O3 -ffast-math -g -mcpu=native -Wall -Wextra -std=c99
DS4_OBJCFLAGS ?= -O3 -ffast-math -g -mcpu=native -Wall -Wextra -fobjc-arc
DS4_LDLIBS    := -lm -pthread -framework Foundation -framework Metal
DS4_CORE      := ds4 ds4_image ds4_distributed ds4_tp ds4_ssd ds4_layer_pack ds4_engram
DS4_SOURCES   := $(DS4_CORE:%=$(DS4_DIR)/%.c) $(DS4_DIR)/ds4_metal.m
DS4_OBJ       := $(DS4_CORE:%=$(BUILD)/ds4/%.o) $(BUILD)/ds4/ds4_metal.o
DS4_SERVER_OBJ := $(BUILD)/ds4/ds4_help.o $(BUILD)/ds4/ds4_kvstore.o $(BUILD)/ds4/rax.o

BEE_CFLAGS ?= -std=c11 -O2 -g -Wall -Wextra -Wpedantic -Wshadow
BEE_HDR    := $(wildcard src/*.h) $(THRUM_HDR)
BEE_OBJ    := $(BUILD)/main.o $(BUILD)/chat.o $(BUILD)/models.o $(BUILD)/gpu.o

.PHONY: all check test conform fetch clean darwin

all: $(BIN)

darwin:
	@test "$$(uname -s)" = Darwin || { \
		echo "dwarfstar links ds4's Metal engine, which needs macOS; make check builds the rest anywhere" >&2; \
		exit 1; }

$(BIN): $(BEE_OBJ) $(BUILD)/shaders.o $(BUILD)/ds4_bridge.o $(DS4_OBJ) $(THRUM_LIB) | darwin
	$(CC) -o $@ $(filter %.o,$^) $(THRUM_LIBS) $(DS4_LDLIBS)
	@rm -rf $(BUILD)/metal && cp -R $(DS4_DIR)/metal $(BUILD)/metal
	@echo "built $@"

check: $(STUB)

$(STUB): $(BEE_OBJ) $(BUILD)/stub/ds4_stub.o $(THRUM_LIB)
	$(CC) -o $@ $(filter %.o,$^) $(THRUM_LIBS)
	@echo "built $@"

test: $(TESTS)
	@$(TESTS)

$(TESTS): $(BUILD)/tests/test_chat.o $(BUILD)/chat.o
	$(CC) -o $@ $^ $(THRUM_DEPS_LIBS)

conform: $(CONFORM)
	@$(CONFORM)

$(CONFORM): $(BUILD)/tests/conform_with_ds4.o $(BUILD)/tests/ds4_reference.o $(BUILD)/chat.o $(DS4_SERVER_OBJ) $(DS4_OBJ) | darwin
	$(CC) -o $@ $(filter %.o,$^) $(THRUM_DEPS_LIBS) $(DS4_LDLIBS)

$(BEE_OBJ) $(BUILD)/shaders.o: $(BUILD)/%.o: src/%.c $(BEE_HDR)
	@mkdir -p $(dir $@)
	$(CC) $(BEE_CFLAGS) $(THRUM_CFLAGS) -c -o $@ $<

$(BUILD)/stub/ds4_stub.o: src/ds4_stub.c $(BEE_HDR)
	@mkdir -p $(dir $@)
	$(CC) $(BEE_CFLAGS) $(THRUM_CFLAGS) -c -o $@ $<

$(BUILD)/tests/test_chat.o $(BUILD)/tests/conform_with_ds4.o: $(BUILD)/tests/%.o: tests/%.c src/chat.h tests/ds4_reference.h
	@mkdir -p $(dir $@)
	$(CC) $(BEE_CFLAGS) $(THRUM_CFLAGS) -Isrc -c -o $@ $<

$(BUILD)/tests/ds4_reference.o: tests/ds4_reference.c tests/ds4_reference.h $(DS4_DIR)/ds4_server.c | darwin
	@mkdir -p $(dir $@)
	$(CC) $(DS4_CFLAGS) -Wno-unused-function -I$(DS4_DIR) -I$(DS4_DIR)/metal -c -o $@ $<

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
