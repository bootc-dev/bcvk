prefix ?= /usr

SOURCE_DATE_EPOCH ?= $(shell git log -1 --pretty=%ct)
# https://reproducible-builds.org/docs/archives/
TAR_REPRODUCIBLE = tar --mtime="@${SOURCE_DATE_EPOCH}" --sort=name --owner=0 --group=0 --numeric-owner --pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime

all: bin manpages

# bcvk is bind-mounted and ran inside the container it starts. We build it statically
# so that it does not depend on the image's loader or its libc. An explicit --target
# is required because proc-macros cannot be built statically.
CARGO_BUILD_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')

.PHONY: bin
bin:
	cargo check --workspace
	RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target $(CARGO_BUILD_TARGET)
	install -D -m755 target/$(CARGO_BUILD_TARGET)/release/bcvk target/release/bcvk

# Generate man pages from markdown sources
# This delegates to the xtask which handles:
# - Syncing CLI options into markdown
# - Flexible section number handling
# - Version placeholder replacement
# - go-md2man conversion
# - Apostrophe handling fixes
.PHONY: manpages
manpages:
	cargo xtask manpages

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
	@# Install all generated man pages to their appropriate sections
	@if [ -d target/man ]; then \
	  for manpage in target/man/*.[1-9]; do \
	    section=$$(echo $$manpage | sed 's/.*\.\([0-9]\)$$/\1/'); \
	    install -D -m 0644 $$manpage -t $(DESTDIR)$(prefix)/share/man/man$$section/; \
	  done; \
	fi

makesudoinstall:
	make
	sudo make install

sync-manpages:
	cargo xtask sync-manpages

update-manpages:
	cargo xtask update-manpages

update-generated: sync-manpages manpages

.PHONY: all bin install manpages update-generated makesudoinstall sync-manpages update-manpages
