# Ephemeral VMs

Ephemeral VMs are temporary virtual machines that start quickly from bootc container images and automatically clean up when stopped.

## Basic Usage

```bash
# Quick test with auto-cleanup
bcvk ephemeral run --rm quay.io/fedora/fedora-bootc:42

# Background VM
bcvk ephemeral run -d --name myvm quay.io/fedora/fedora-bootc:42

# With SSH
bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42
```

## Resource Configuration

```bash
# Custom resources
bcvk ephemeral run --memory 4096 --cpus 4 --name myvm quay.io/fedora/fedora-bootc:42
```

## Detecting an ephemeral environment

Conceptually now with `bcvk ephemeral`, there's *four* different ways to run
a bootc container:

- `podman|docker run <image> bash` - directly run a shell (or other process) the container without systemd. Uses the host kernel, not kernel in the container.
- `podman|docker run <image>` - by default runs systemd. See also <https://docs.fedoraproject.org/en-US/bootc/provisioning-container/>. Uses the host kernel, not kernel in the container.
- `bootc install` - Run directly on metal or a virtualized environment. Uses the kernel in the container.
- `bcvk ephemeral` - Run as a virtual machine, but *not* a true "bootc install". Uses the kernel in the container.

Some systemd units may need adaption to work in all of these modes. For example, if you have a systemd generator
which synthesizes mount units for expected partitions, it can use `ConditionVirtualization=!container` to skip
those in the first two cases (ensuring it still runs after a `bootc install`), but that won't be skipped in `bcvk ephemeral`
even though there won't be any block devices (by default).

At the current time there is not a dedicated way to detect `bcvk ephemeral`, but `ConditionKernelCommandLine=!rootfstype=virtiofs`
should work reliably in the future.

## Network Isolation

Use `--network-isolation` to block all outbound network access from the VM.
SSH from the host is preserved, but processes inside the guest cannot reach
the internet.  This is useful for hermetic CI/CD testing where all inputs
should be pre-staged:

```bash
bcvk ephemeral run-ssh --network-isolation --bind-storage-ro \
  localhost/mybootc -- make test
```

## Use Cases

- Quick testing of bootc images
- Development environments
- CI/CD integration
- Hermetic testing with `--network-isolation`

See [ephemeral-ssh](./ephemeral-ssh.md) for SSH access details.