# NAME

bcvk-ephemeral-test-basic - Boot an ephemeral VM and verify systemd is healthy

# SYNOPSIS

**bcvk ephemeral test-basic** \[*OPTIONS*\] *IMAGE*

# DESCRIPTION

Boot an ephemeral VM from **IMAGE** and verify that systemd reached a healthy
state by running **systemctl is-system-running --wait** via SSH.

This is a quick smoke test for bootc container images. The **--wait** flag
ensures systemd has finished booting before reporting status. The command
exits 0 if systemd reports "running" (all units healthy), or non-zero if
the system is degraded or failed.

Internally this is equivalent to:

    bcvk ephemeral run-ssh IMAGE -- systemctl is-system-running --wait

The VM is automatically cleaned up after the check completes.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
Accepts the same options as **bcvk-ephemeral-run**(8).
<!-- END GENERATED OPTIONS -->

# EXAMPLES

Smoke test a Fedora bootc image:

    bcvk ephemeral test-basic quay.io/fedora/fedora-bootc:42

Test a locally built image:

    podman build -t localhost/mybootc .
    bcvk ephemeral test-basic localhost/mybootc

Test with custom resources:

    bcvk ephemeral test-basic --memory 4096 --vcpus 2 localhost/mybootc

# SEE ALSO

**bcvk-ephemeral**(8), **bcvk-ephemeral-run-ssh**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
