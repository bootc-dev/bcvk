# NAME

bcvk-ephemeral - Manage ephemeral VMs for bootc containers

# SYNOPSIS

**bcvk ephemeral** [*OPTIONS*]

**bcvk e** [*OPTIONS*]

# DESCRIPTION

Manage ephemeral VMs for bootc containers

<!-- BEGIN GENERATED OPTIONS -->
<!-- END GENERATED OPTIONS -->

# EXAMPLES

Run an ephemeral VM in the background:

    bcvk ephemeral run -d --rm --name mytestvm quay.io/fedora/fedora-bootc:42

Run an ephemeral VM with custom resources:

    bcvk ephemeral run --memory 4096 --cpus 4 --name bigvm quay.io/fedora/fedora-bootc:42

Run an ephemeral VM with automatic SSH key generation:

    bcvk ephemeral run -d --rm -K --name testvm quay.io/fedora/fedora-bootc:42

# SEE ALSO

**bcvk**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
