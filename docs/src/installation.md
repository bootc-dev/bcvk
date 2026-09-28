# Installation

## From Packages

bcvk is available in Fedora 42+ and EPEL 9/10:

```bash
sudo dnf install bcvk
```

## Prerequisites

### Host Requirements

For building from source:
- [Rust](https://www.rust-lang.org/)
- Git

For running bcvk:
- QEMU/KVM
- qemu-img
- virtiofsd
- Podman
- openssh-clients (for libvirt SSH operations)
- binutils (for extracting UKIs, including ukiboot payloads)
- android-tools (for extracting Android boot images)
- kernel-tools (for initramfs bootconfig and ARM64 zstd EFI zboot kernels)

Optional:
- libvirt (for persistent VM features)
  ```bash
  sudo systemctl enable --now libvirtd
  sudo usermod -a -G libvirt $USER
  ```

### Target Bootc Image Requirements

For `bcvk ephemeral` operations, the bootc container images you run must contain:
- systemctl (systemd)
- mount (util-linux), chroot (coreutils)
- ssh, ssh-keygen (openssh-clients)

## Development Binaries

Pre-built binaries from `main` are available as OCI artifacts:

```bash
# Requires ORAS (https://oras.land/)
# Note: This command pulls the x86_64 architecture binary
oras pull ghcr.io/bootc-dev/bcvk-binary:x86_64-latest
tar -xzf bcvk-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 bcvk-x86_64-unknown-linux-gnu /usr/local/bin/bcvk
```

## Building from Source

Without cloning the repo:

```bash
cargo install --locked --git https://github.com/bootc-dev/bcvk bcvk
```

Inside a clone of the repo:

```bash
cargo install --locked --path crates/kit
```

## Toolbox, distrobox and remote podman

bcvk is designed to be installed on the host, alongside podman and QEMU.
To launch a VM, it asks podman to bind-mount the bcvk binary itself and
the host's `/usr` (which provides QEMU and virtiofsd) into a new
container, so podman must see the same filesystem as bcvk.

When bcvk runs inside a [toolbox](https://containertoolbx.org/) or
[distrobox](https://distrobox.it/) container with podman installed in
that container too, podman sees bcvk's paths and the error described
below doesn't occur, though nested podman has limitations of its own
(for example around networking). However, podman is often forwarded to
the host instead (for example via a `flatpak-spawn --host podman`
wrapper or the podman socket), and the same applies to a remote podman
client. Paths that bcvk passes to podman are then resolved on the host,
so a bcvk binary installed only in the toolbox cannot be found, and
podman fails with an error like `statfs /usr/bin/bcvk: no such file or
directory`. bcvk explains this in its error when it sees that podman
could not find a path that bcvk itself can see.

The simplest fix is to install bcvk on the host and run it there. From
inside a toolbox you can still invoke the host's copy with
`flatpak-spawn --host bcvk ...`, and from a distrobox with
`distrobox-host-exec bcvk ...`.

Beware of version skew: if different bcvk binaries exist at the same
path on the host and in the toolbox (e.g. `/usr/bin/bcvk` installed from
packages in both), the error above doesn't happen, but the host's copy is
the one podman mounts into the VM's container. The bcvk you ran and the
one that sets up the VM are then different versions, which can fail in
confusing ways. Keep them in sync, or install bcvk only on the host.

See [#5](https://github.com/bootc-dev/bcvk/issues/5) for discussion of
better support for this case.

## Platform Support

- Linux: Supported
- macOS: Not supported, use [podman-bootc](https://github.com/containers/podman-bootc/)
- Windows: Not supported

See the [Quick Start Guide](./quick-start.md) to begin using bcvk.
