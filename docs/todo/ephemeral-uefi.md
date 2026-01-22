# TODO: UEFI Boot for Ephemeral VMs

Tracking issue: https://github.com/bootc-dev/bcvk/issues/161

## Current State

Ephemeral VMs currently use direct kernel boot via QEMU's `-kernel` and
`-initrd` options. For UKI-only images, we extract the kernel and initramfs
from the UKI using `objcopy --dump-section`.

### Where UKIs Live in bootc Images

UKIs can be in either location:
- `/boot/EFI/Linux/*.efi` - ESP location (Boot Loader Specification)
- `/usr/lib/modules/<version>/<version>.efi` - alongside kernel modules

bcvk checks all locations:
1. `/boot/EFI/Linux/*.efi` - UKI in ESP
2. `/usr/lib/modules/<version>/<version>.efi` - UKI alongside modules
3. `/usr/lib/modules/<version>/vmlinuz` + `initramfs.img` - traditional

### Limitations

This works but has limitations:
- Doesn't exercise the real systemd-boot/UKI boot path
- Breaks the UKI signature chain (no Secure Boot)

## Phase 1: systemd-boot + UKI Boot

**Goal**: Support booting ephemeral VMs through systemd-boot + UKI path, matching
more closely the boot process for "full installs".

### Approach

Use modern systemd features to inject bcvk's configuration without modifying
the UKI itself:

1. **`io.systemd.stub.kernel-cmdline-extra`** (SMBIOS credential)
   - systemd-stub reads this from SMBIOS Type 11 strings
   - We already pass credentials via SMBIOS, so this is a natural fit
   - Use this to pass bcvk's kernel command line arguments

2. **System/Config Extensions** for injecting units
   - `*.sysext.raw` or `*.confext.raw` placed on the ESP
   - systemd-stub loads these and makes them available to the initrd
   - Can contain bcvk's systemd units for /etc overlay, /var setup, etc.

### Implementation Steps

1. **Create ESP image dynamically**:
   - Build a small FAT32 disk image using `mtools` (no root required)
   - Copy the UKI from the container to `/EFI/Linux/<name>.efi`
   - Create bcvk confext with our systemd units

2. **Boot via OVMF**:
   - Pass OVMF firmware to QEMU (`-bios` or `-drive if=pflash`)
   - Attach ESP as a disk
   - systemd-boot auto-discovers and boots the UKI
   - Pass the virtiofs mount as a karg, same as we do today

3. **Pass credentials via SMBIOS**:
   - Continue using existing SMBIOS credential mechanism
   - Add `io.systemd.stub.kernel-cmdline-extra` for additional cmdline args

### Requirements

- systemd >= 254 for robust `kernel-cmdline-extra` support
- OVMF firmware available on the host
- `mtools` for ESP creation (or `mkfs.fat` + loop mount with privileges)

## Phase 2: Secure Boot Support (Nice to Have)

**Goal**: Support Secure Boot for ephemeral VMs, maintaining the full trust
chain from firmware through UKI.

### Key Insight: Upstream the Mount Setup to bootc

The cleanest path to Secure Boot support is to **not require bcvk-specific
initramfs modifications at all**. The baseline functionality that bcvk
currently injects (e.g., /etc overlay, /var tmpfs setup) should be handled
by bootc's upstream initramfs code, triggered by kernel command line
arguments or systemd credentials.

This means:
- bootc's initramfs generator includes support for ephemeral/read-only root
- bcvk just passes the right cmdline args via `io.systemd.stub.kernel-cmdline-extra`
- The UKI remains completely unmodified, preserving its signature
- Secure Boot works out of the box

### What Needs Upstreaming to bootc

1. **Ephemeral /etc overlay**: Mount /etc as an overlay with tmpfs upper
   - Triggered by e.g. `bootc.etc=overlay` or a credential
   
2. **Ephemeral /var**: Mount /var as tmpfs instead of persistent storage
   - Triggered by e.g. `bootc.var=tmpfs`

3. **Read-only root awareness**: Handle virtiofs or other read-only root
   filesystems gracefully

Once these are in bootc's initramfs, bcvk ephemeral mode becomes:
1. Boot the UKI via OVMF (no modifications)
2. Pass credentials/cmdline via SMBIOS
3. Done - Secure Boot compatible

### bcvk-Specific Features (Still Need Injection)

Some bcvk features may still need addon EFI or confext injection:
- Journal streaming to host (`--log` functionality)
- Execute command services (`--execute`)
- SSH key injection (though credentials may suffice)

For these, the Phase 1 confext approach works, and signing becomes a
user choice rather than a hard requirement.

The challenge is that anything we inject this way via systemd-stub 
needs signing.

I think what might work here is for us to locally sign our generated
content, and then inject those signing keys into the firmware trust roots
too.

## Technical Details

### systemd-stub Addon Mechanism

From systemd source (`src/boot/stub.c`), addon files named `*.addon.efi`
placed next to the UKI are loaded as PE binaries:

```c
// Addon .initrd sections are appended to the base initrd
if (initrd_addons && PE_SECTION_VECTOR_IS_SET(sections + UNIFIED_SECTION_INITRD)) {
    // ... loads .initrd section from addon
}
```

Addon EFI binaries can contain:
- `.initrd` section - appended to base initrd (measured into PCR 12)
- `.cmdline` section - appended to kernel command line

### UKI Location in Container

bootc images store UKIs at:
```
/boot/EFI/Linux/<kver>.efi
```

For composefs sealed images, bootc uses a subdirectory:
```
/boot/EFI/Linux/bootc/<deployment>.efi
```

### ESP Layout for UEFI Boot

For Phase 1, bcvk would create a virtual ESP with:
```
/EFI/
  BOOT/
    BOOTX64.EFI          # systemd-boot
  Linux/
    <image>.efi          # UKI copied from container's /boot/EFI/Linux/
```

For Phase 2 with addons:
```
/EFI/
  Linux/
    <image>.efi          # The UKI
  systemd/
    addon/
      bcvk.addon.efi     # bcvk addon (signed for Secure Boot)
```

Or for confexts:
```
/loader/
  addons/
    bcvk.confext.raw     # Configuration extension with bcvk units
```

### SMBIOS Credentials

systemd-stub reads these SMBIOS Type 11 strings:
- `io.systemd.credential:<name>=<value>` - arbitrary credentials
- `io.systemd.stub.kernel-cmdline-extra=<args>` - extra kernel arguments

We already use SMBIOS for credentials; extending this is straightforward.

## References

- [systemd-stub(7)](https://man7.org/linux/man-pages/man7/systemd-stub.7.html) - UEFI stub documentation
- [systemd-boot(7)](https://man7.org/linux/man-pages/man7/systemd-boot.7.html) - Boot manager
- [systemd-sysext(8)](https://man7.org/linux/man-pages/man8/systemd-sysext.8.html) - System extensions
- [ukify(1)](https://www.freedesktop.org/software/systemd/man/latest/ukify.html) - UKI build tool
- https://github.com/bootc-dev/bootc/issues/1940 - Related bootc issue
