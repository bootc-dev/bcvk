# NAME

bcvk-anaconda - Install bootc containers using anaconda and kickstart

# SYNOPSIS

**bcvk anaconda** \<*subcommand*\>

# DESCRIPTION

The **bcvk anaconda** command installs bootc container images to disk using
anaconda as the installation engine. This provides an alternative to
**bcvk to-disk** that leverages anaconda's kickstart-based configuration
for partitioning and system setup.

Anaconda runs inside an ephemeral VM with access to the host's container
storage via virtiofs. The target disk is attached as a virtio-blk device,
and anaconda installs the bootc image using the **ostreecontainer** kickstart
directive.

## When to Use Anaconda

Use **bcvk anaconda** when you need:

- Custom partitioning layouts via kickstart
- Integration with existing kickstart-based workflows
- Anaconda-specific features (LVM, LUKS encryption, etc.)

For simpler cases where you just need a bootable disk image, **bcvk to-disk**
is faster and requires less setup.

## Prerequisites

The anaconda-bootc container must be built before use:

    podman build -t localhost/anaconda-bootc:latest containers/anaconda-bootc/

<!-- BEGIN GENERATED OPTIONS -->
<!-- END GENERATED OPTIONS -->

# SUBCOMMANDS

bcvk-anaconda-install(8)

:   Install a bootc container to a disk image using anaconda

# EXAMPLES

Install a bootc image with a custom kickstart:

    bcvk anaconda install -k my-kickstart.ks --disk-size 20G \
        quay.io/fedora/fedora-bootc:42 output.img

# SEE ALSO

**bcvk**(8), **bcvk-anaconda-install**(8), **bcvk-to-disk**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
