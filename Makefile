prefix ?= /usr

all:
	cargo b --release

install:
	install -D -m 0755 -t $(DESTDIR)$(prefix)/bin target/release/bootc-kit
