# NAME

bcvk-ephemeral-run-ssh - Run ephemeral VM and SSH into it

# SYNOPSIS

**bcvk ephemeral run-ssh** \[*OPTIONS*\] *IMAGE* \[*SSH_ARGS*\]

# DESCRIPTION

Run a bootc container as an ephemeral VM and immediately connect via SSH.
When the SSH session ends, the VM is automatically stopped and cleaned up.

This is the recommended command for quick testing and iteration. It combines
**bcvk ephemeral run** and **bcvk ephemeral ssh** into a single command with
automatic lifecycle management.

## Lifecycle Behavior

Unlike **bcvk ephemeral run -d** which starts a background VM that persists
until explicitly stopped, **run-ssh** ties the VM lifecycle to the SSH session:

1. VM boots and waits for SSH to become available
2. SSH session is established
3. When you exit the SSH session (or the connection drops), the VM is stopped
4. The container is automatically removed

This makes **run-ssh** ideal for:

- Quick one-off testing of container images
- Rapid build-test iteration cycles
- Running a single command in a VM environment
- Demos and exploration

For longer-running VMs where you need to reconnect multiple times, use
**bcvk ephemeral run -d --rm -K** instead.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as ephemeral VM

    This argument is required.

**SSH_ARGS**

    SSH command to execute (optional, defaults to interactive shell)

**--itype**=*ITYPE*

    Instance type (e.g., u1.nano, u1.small, u1.medium). Overrides vcpus/memory if specified.

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--vcpus**=*VCPUS*

    Number of vCPUs (overridden by --itype if specified)

**--console**

    Enable console output to terminal for debugging

**--debug**

    Enable debug mode (drop to shell instead of running QEMU)

**--virtio-serial-out**=*NAME:FILE*

    Add virtio-serial device with output to file (format: name:/path/to/file)

**--execute**=*EXECUTE*

    Execute command inside VM via systemd and capture output

**-K**, **--ssh-keygen**

    Generate SSH keypair and inject via systemd credentials

**-t**, **--tty**

    Allocate a pseudo-TTY for container

**-i**, **--interactive**

    Keep STDIN open for container

**-d**, **--detach**

    Run container in background

**--rm**

    Automatically remove container when it exits

**--name**=*NAME*

    Assign a name to the container

**--network**=*NETWORK*

    Configure the network for the container

**--label**=*LABEL*

    Add metadata to the container in key=value form

**-e**, **--env**=*ENV*

    Set environment variables in the container (key=value)

**--debug-entrypoint**=*DEBUG_ENTRYPOINT*

    Do not run the default entrypoint directly, but instead invoke the provided command (e.g. `bash`)

**--bind**=*HOST_PATH[:NAME]*

    Bind mount host directory (RW) at /run/virtiofs-mnt-<name>

**--ro-bind**=*HOST_PATH[:NAME]*

    Bind mount host directory (RO) at /run/virtiofs-mnt-<name>

**--systemd-units**=*SYSTEMD_UNITS_DIR*

    Directory with systemd units to inject (expects system/ subdirectory)

**--bind-storage-ro**

    Mount host container storage (RO) at /run/virtiofs-mnt-hoststorage

**--add-swap**=*ADD_SWAP*

    Allocate a swap device of the provided size

**--mount-disk-file**=*FILE[:NAME]*

    Mount disk file as virtio-blk device at /dev/disk/by-id/virtio-<name>

**--karg**=*KERNEL_ARGS*

    Additional kernel command line arguments

<!-- END GENERATED OPTIONS -->

# EXAMPLES

## Basic Usage

Test a public bootc image:

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42

Test a CentOS bootc image:

    bcvk ephemeral run-ssh quay.io/centos-bootc/centos-bootc:stream10

## Build and Test Workflow

The most common development pattern is building and immediately testing:

    # Build your container
    podman build -t localhost/mybootc .

    # Boot it and SSH in (auto-cleanup on exit)
    bcvk ephemeral run-ssh localhost/mybootc

For rapid iteration, combine in one command:

    podman build -t localhost/mybootc . && bcvk ephemeral run-ssh localhost/mybootc

## Running Commands

Execute a specific command and exit:

    bcvk ephemeral run-ssh localhost/mybootc 'systemctl status'

Check what services are running:

    bcvk ephemeral run-ssh localhost/mybootc 'systemctl list-units --type=service --state=running'

Verify your custom configuration:

    bcvk ephemeral run-ssh localhost/mybootc 'cat /etc/myapp/config.yaml'

## Resource Allocation

For memory-intensive testing:

    bcvk ephemeral run-ssh --memory 8G --vcpus 4 localhost/mybootc

Using instance types:

    bcvk ephemeral run-ssh --itype u1.medium localhost/mybootc

## With Bind Mounts

Mount source code for testing:

    bcvk ephemeral run-ssh --bind /home/user/project:src localhost/mybootc
    # Inside VM: ls /run/virtiofs-mnt-src

## Debugging

Enable console output to see boot messages:

    bcvk ephemeral run-ssh --console localhost/mybootc

# TIPS

- **Fast iteration**: Keep your container builds small and layered for faster
  rebuilds. The VM boots in seconds, so the container build is usually the
  bottleneck.

- **SSH arguments**: Any arguments after the image name are passed to SSH.
  Use this for commands, port forwarding, or SSH options.

- **Exit cleanly**: Use `exit` or Ctrl+D to cleanly end the SSH session and
  trigger VM cleanup. Ctrl+C may not clean up properly.

# SEE ALSO

**bcvk**(8), **bcvk-ephemeral**(8), **bcvk-ephemeral-run**(8),
**bcvk-ephemeral-ssh**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
