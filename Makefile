all:
	cargo xtask build

install:
	install -D -m 0755 target/release/bootc-kit $(DESTDIR)/usr/libexec/bootc-kit-backend 
	install -d -m 0755 $(DESTDIR)/usr/bin
	ln -s ../lib/bootc-kit/nu/bootckit $(DESTDIR)/usr/bin/bootckit
	install -D -m 0755 -t $(DESTDIR)/usr/lib/bootc-kit/nu nu/*

makesudoinstall:
	make
	sudo make install

ROOTCFG_NUSHELL=$(DESTDIR)/root/.config/nushell
install-nushell-config:
	# Auto create parents
	mkdir -p $$(dirname $(ROOTCFG_NUSHELL))
	# Intentially error if this already exists
	mkdir $(ROOTCFG_NUSHELL)
	install -m 0644 nu/shellconfig.nu $(ROOTCFG_NUSHELL)/config.nu 
	touch $(ROOTCFG_NUSHELL)/env.nu
