prefix ?= /usr

all:
	cargo xtask build

install:
	install -D -m 0755 -t $(DESTDIR)$(prefix)/bin target/release/bootc-kit
