prefix ?= /usr

all:
    cargo build --release

install:
	install -D -m 0755 -t $(DESTDIR)$(prefix)/bin target/release/bootc-kit