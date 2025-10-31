# NAME

bcvk-project-down - Shut down the project VM

# SYNOPSIS

**bcvk project down** [*OPTIONS*]

# DESCRIPTION

Shut down the project VM

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**--connect**=*CONNECT*

    Libvirt connection URI (defaults to qemu:///session)

**--remove**

    Remove the VM after shutting it down

**--force**

    

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Shut down the project VM:

    bcvk project down

The VM is stopped but not removed. You can start it again with `bcvk project up`.

Shut down and remove the project VM:

    bcvk project down --remove

This completely deletes the VM, freeing up all associated storage.

# SEE ALSO

**bcvk**(8), **bcvk-project**(8), **bcvk-project-up**(8), **bcvk-project-ssh**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
