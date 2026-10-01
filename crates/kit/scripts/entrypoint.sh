#!/bin/bash
set -euo pipefail

SELFEXE=/run/selfexe
TMPROOT=/run/tmproot

# Check for required binaries early
for bin in mount chroot; do
    if ! command -v "$bin" &>/dev/null; then
        echo "Error: $bin (util-linux or coreutils) is required in the target container image" >&2
        exit 1
    fi
done

# Shell script library
init_tmproot() {
    if test -d /run/inner-shared; then return 0; fi
    # Should have been created by podman when initializing
    # the bind mount
    cd "$TMPROOT"

    # Create essential symlinks
    ln -sf usr/bin bin
    ln -sf usr/lib lib
    ln -sf usr/lib64 lib64
    ln -sf usr/sbin sbin
    mkdir -p {etc,var/tmp,dev,proc,run,sys,tmp}
    # Ensure we have /etc/passwd as ssh-keygen wants it for bad reasons
    systemd-sysusers --root $(pwd) &>/dev/null

    # Copy DNS configuration from container's /etc/resolv.conf (configured by podman --dns)
    # into the new root so QEMU's slirp can use it for DNS resolution
    if [ -f /etc/resolv.conf ]; then
        cp /etc/resolv.conf "$TMPROOT/etc/resolv.conf"
    fi

    # We're already privileged in the container's own mount namespace,
    # which is torn down with it, so set up the hybrid root's mounts
    # here; later `podman exec` invocations share them. Bind /run first,
    # so its copy under the new root doesn't also pick up the mounts
    # made below. /tmp needs nothing: /run is already a tmpfs.
    mount --rbind /run "$TMPROOT/run"
    mount -t proc proc "$TMPROOT/proc"
    mount --rbind /dev "$TMPROOT/dev"
    mount --rbind /var/tmp "$TMPROOT/var/tmp"

    # Shared directory between containers; also signals that the root is ready
    mkdir /run/inner-shared
}

# Pass ALL arguments to container-entrypoint
# Default to "run-ephemeral" if no args
if [[ $# -eq 0 ]]; then
    set -- "run-ephemeral"
    # Initialize environment
    init_tmproot
else
    # Other commands should wait for the other process
    # to create the temp root
    while test '!' -d /run/inner-shared; do sleep 0.1; done
fi

# Check systemd version from the container image (not host)
export SYSTEMD_VERSION=$(systemctl --version 2>/dev/null)

# container-entrypoint handles SIGTERM/SIGINT itself, so it can just
# replace us (as PID 1 for the main invocation).
exec chroot "$TMPROOT" "$SELFEXE" container-entrypoint "$@"
