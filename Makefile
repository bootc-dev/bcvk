prefix ?= /usr

all:
	cargo xtask build

install:
	install -D -m 0755 target/release/bootc-kit $(DESTDIR)$(prefix)/libexec/bootc-kit-backend 
	install -D -m 0755 -t $(DESTDIR)$(prefix)/lib/bootc-kit/nu nu/*.nu

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
