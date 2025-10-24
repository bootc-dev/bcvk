# Installation

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
