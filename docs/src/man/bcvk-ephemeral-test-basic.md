# NAME

bcvk-ephemeral-test-basic - Boot an ephemeral VM and check that systemd reaches the running state

# SYNOPSIS

**bcvk ephemeral test-basic** \[*OPTIONS*\] *IMAGE*

# DESCRIPTION

Boot **IMAGE** as an ephemeral VM and check that systemd reaches the
"running" state, as a quick smoke test for bootc container images.

This is shorthand for:

    bcvk ephemeral run-ssh IMAGE -- systemctl is-system-running --wait

(run through **/bin/sh -c** in the VM, with the handling below). It prints
the final system state, so on success its only output is "running". If the
state is anything else (e.g. "degraded" because a unit failed), it also lists
the failed units. As with **bcvk-ephemeral-run-ssh**(8), the VM is removed
when the check completes.

# EXIT STATUS

**0**

:   systemd reached the "running" state

**1**

:   systemd finished booting in another state, such as "degraded"

**124**

:   boot did not finish within 10 minutes of SSH becoming available (e.g. a
    start job with no timeout); the pending jobs are listed

Failures of **bcvk-ephemeral-run-ssh**(8) itself, for example when the VM
fails to boot or SSH never becomes available, also exit non-zero (usually 1)
and print an error on stderr.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as ephemeral VM

    This argument is required.

**--itype**=*ITYPE*

    Instance type (e.g., u1.nano, u1.small, u1.medium). Overrides vcpus/memory if specified.

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--vcpus**=*VCPUS*

    Number of vCPUs (overridden by --itype if specified)

**--console**

    Connect the QEMU console to the container's stdio (visible via podman logs/attach)

**--debug**

    Enable debug mode (drop to shell instead of running QEMU)

**--virtio-serial-out**=*NAME:FILE*

    Add virtio-serial device with output to file (format: name:/path/to/file)

**--execute**=*EXECUTE*

    Execute command inside VM via systemd and capture output

**-K**, **--ssh-keygen**

    Generate SSH keypair and inject via systemd credentials

**--virtiofsd**=*VIRTIOFSD_BINARY*

    Path to virtiofsd binary (overrides auto-detection)

**--output**=*OUTPUT*

    Select how VM output is presented

    Possible values:
    - console
    - journal

    Default: console

**--log-dir**=*STREAMS=DIR*

    Write VM log streams to files in DIR

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

**--ignition**=*IGNITION_CONFIG*

    Path to Ignition config file (JSON format) to inject via fw_cfg

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Smoke test a Fedora bootc image:

    bcvk ephemeral test-basic quay.io/fedora/fedora-bootc:42

Test a locally built image:

    podman build -t localhost/mybootc .
    bcvk ephemeral test-basic localhost/mybootc

Test with more memory (the default is 4G) and CPUs:

    bcvk ephemeral test-basic --memory 8G --vcpus 4 localhost/mybootc

# SEE ALSO

**bcvk-ephemeral**(8), **bcvk-ephemeral-run-ssh**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
