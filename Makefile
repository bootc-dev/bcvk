prefix ?= /usr

SOURCE_DATE_EPOCH ?= $(shell git log -1 --pretty=%ct)
# https://reproducible-builds.org/docs/archives/
TAR_REPRODUCIBLE = tar --mtime="@${SOURCE_DATE_EPOCH}" --sort=name --owner=0 --group=0 --numeric-owner --pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime

all: bin manpages

.PHONY: bin
bin:
	cargo build --release

# Generate man pages from markdown sources
MAN8_SOURCES := $(wildcard docs/src/man/*.md)
TARGETMAN := target/man
MAN8_TARGETS := $(patsubst docs/src/man/%.md,$(TARGETMAN)/%.8,$(MAN8_SOURCES))

# Single rule for generating man pages
$(TARGETMAN)/%.8: docs/src/man/%.md
	@mkdir -p $(TARGETMAN)
	@# Create temp file with synced content
	@cp $< $<.tmp
	@# Generate man page using go-md2man
	@go-md2man -in $<.tmp -out $@
	@# Fix apostrophe handling
	@sed -i -e "1i .ds Aq \\\\(aq" -e "/\\.g \\.ds Aq/d" -e "/.el .ds Aq \'/d" $@
	@rm -f $<.tmp
	@echo "Generated $@"

# Sync CLI options before generating man pages
.PHONY: manpages
manpages: sync-cli-options $(MAN8_TARGETS)

# Hidden target to sync CLI options once
sync-cli-options:
	@cargo xtask sync-manpages >/dev/null 2>&1 || true

# This gates CI by default. Note that for clippy, we gate on
# only the clippy correctness and suspicious lints, plus a select
# set of default rustc warnings.
# We intentionally don't gate on this for local builds in cargo.toml
# because it impedes iteration speed.
CLIPPY_CONFIG = -A clippy::all -D clippy::correctness -D clippy::suspicious -D clippy::disallowed-methods -Dunused_imports -Ddead_code
validate:
	cargo fmt -- --check -l
	cargo test --no-run --workspace
	cargo clippy -- $(CLIPPY_CONFIG)
	env RUSTDOCFLAGS='-D warnings' cargo doc --lib
.PHONY: validate

install:
	install -D -m 0755 -t $(DESTDIR)$(prefix)/bin target/release/bcvk
	if [ -n "$(MAN8_TARGETS)" ]; then \
	  install -D -m 0644 -t $(DESTDIR)$(prefix)/share/man/man8 $(MAN8_TARGETS); \
	fi

makesudoinstall:
	make
	sudo make install

sync-manpages:
	cargo xtask sync-manpages

update-manpages:
	cargo xtask update-manpages

update-generated: sync-manpages manpages

.PHONY: all bin install manpages update-generated makesudoinstall sync-manpages update-manpages sync-cli-options nbd-server

# Cross-compile NBD server for podman machine (needed on macOS/Windows, no-op on Linux)
NBD_ARCH := $(shell rustc -vV | awk '/^host:/ {split($$2, a, "-"); print a[1]}')
NBD_TARGET := $(NBD_ARCH)-unknown-linux-gnu

.PHONY: nbd-server
nbd-server:
ifneq ($(shell uname -s),Linux)
	PATH="$(HOME)/.rustup/toolchains/stable-$(shell rustc -vV | awk '/^host:/ {print $$2}')/bin:$(HOME)/.cargo/bin:$(PATH)" \
	  cargo zigbuild --target $(NBD_TARGET) --release -p bcvk-nbd
	mkdir -p target/nbd-server
	cp target/$(NBD_TARGET)/release/bcvk-nbd target/nbd-server/bcvk-nbd
endif
