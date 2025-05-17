all:
	cargo xtask build

install:
	install -D -m 0755 target/release/bootc-kit $(DESTDIR)/usr/bin/bootc-kit

makesudoinstall:
	make
	sudo make install

# No additional configuration needed anymore
