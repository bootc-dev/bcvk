# Installation

## From Packages

bcvk is available in Fedora 42+ and EPEL 9/10:

```bash
sudo dnf install bcvk
```

## Prerequisites

Required:
- [Rust](https://www.rust-lang.org/)
- Git
- QEMU/KVM
- virtiofsd
- Podman

Optional:
- libvirt (for persistent VM features)
  ```bash
  sudo systemctl enable --now libvirtd
  sudo usermod -a -G libvirt $USER
  ```

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

## Platform Support

- Linux: Supported
- macOS: Not supported, use [podman-bootc](https://github.com/containers/podman-bootc/)
- Windows: Not supported

See the [Quick Start Guide](./quick-start.md) to begin using bcvk.
