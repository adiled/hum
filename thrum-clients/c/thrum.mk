# thrum.mk: what a C bee's Makefile includes to build against the thrum
# client. It finds itself, asks pkg-config for libsodium and cJSON, and
# knows how to build $(THRUM_LIB).
#
#   include ../../thrum-clients/c/thrum.mk
#
#   my-bee: main.c $(THRUM_LIB)
#   	$(CC) $(CFLAGS) $(THRUM_CFLAGS) -o $@ main.c $(THRUM_LIBS)

THRUM_DIR := $(patsubst %/,%,$(dir $(lastword $(MAKEFILE_LIST))))
THRUM_BUILD ?= $(THRUM_DIR)/out
THRUM_DEPS := libsodium libcjson
PKG_CONFIG ?= pkg-config

ifneq ($(filter-out clean,$(or $(MAKECMDGOALS),all)),)
ifneq ($(shell $(PKG_CONFIG) --exists $(THRUM_DEPS) && echo yes),yes)
$(error the thrum C client needs libsodium and cJSON through pkg-config. \
macOS: brew install pkgconf libsodium cjson. \
Debian/Ubuntu: apt-get install pkg-config libsodium-dev libcjson-dev)
endif
endif

THRUM_DEPS_CFLAGS := $(shell $(PKG_CONFIG) --cflags $(THRUM_DEPS) 2>/dev/null)
THRUM_DEPS_LIBS := $(shell $(PKG_CONFIG) --libs $(THRUM_DEPS) 2>/dev/null)

THRUM_LIB := $(THRUM_BUILD)/libthrum.a
THRUM_CFLAGS := -I$(THRUM_DIR) $(THRUM_DEPS_CFLAGS)
THRUM_LIBS := $(THRUM_LIB) $(THRUM_DEPS_LIBS) -pthread

THRUM_CC_FLAGS ?= -std=c11 -O2 -g -Wall -Wextra -Wpedantic -Wshadow -Wstrict-prototypes
THRUM_SRC := $(addprefix $(THRUM_DIR)/thrum/,helpers.c identity.c client.c spec.c)
THRUM_OBJ := $(patsubst $(THRUM_DIR)/thrum/%.c,$(THRUM_BUILD)/thrum/%.o,$(THRUM_SRC))
THRUM_HDR := $(wildcard $(THRUM_DIR)/thrum/*.h)

$(THRUM_BUILD)/thrum/%.o: $(THRUM_DIR)/thrum/%.c $(THRUM_HDR)
	@mkdir -p $(dir $@)
	$(CC) $(THRUM_CC_FLAGS) $(THRUM_CFLAGS) -c -o $@ $<

$(THRUM_LIB): $(THRUM_OBJ)
	@mkdir -p $(dir $@)
	$(AR) rcs $@ $^
