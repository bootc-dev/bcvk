# NAME

bcvk-project-rm - Remove the project VM and its resources

# SYNOPSIS

**bcvk project rm** [*OPTIONS*]

# DESCRIPTION

Permanently removes the project VM and its associated disk images. This is equivalent
to `bcvk project down --remove` but provides more granular control with options for
forcing removal and handling running VMs.

By default, this command will ask for confirmation before removing the VM. Use `--force`
to skip the confirmation prompt, which is useful for automated scripts.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**--connect**=*CONNECT*

    Libvirt connection URI (defaults to qemu:///session)

**-f**, **--force**

    Force removal without confirmation

**--stop**

    Remove domain even if it's running

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Remove a stopped project VM:

    bcvk project rm

This will prompt for confirmation before removing the VM.

Force removal without confirmation:

    bcvk project rm --force

Useful for automated cleanup scripts.

Remove a running VM:

    bcvk project rm --stop --force

This will stop the VM if it's running, then remove it without prompting for confirmation.

# SEE ALSO

**bcvk**(8), **bcvk-project**(8), **bcvk-project-down**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
