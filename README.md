# A toolkit for developing bootc containers

This repository is a container image which supports
installing bootc container images.

The core idea is that as much code as possible for this
comes as a container image. It does however run in
privileged mode so that it can access your host's
container storage and execute host services
where needed such as libvirt.

## Usage

There are a default set of required privileges passed to the container image;
this can get tedious to type, so the `entrypoint` command can be used to print
a bash script which you can install.

### Initialize

```bash
# Output the entrypoint script (you need to ensure ~/.local/bin/ is in $PATH).
podman run --rm ghcr.io/bootc-dev/kit entrypoint > ~/.local/bin/bck && chmod a+x ~/.local/bin/bck
```

From here after, `bck` will be used as an alias for this entrypoint script.
However, again it is not required.

### List bootc images

Verify this works to show your bootc images:

`bck images list`

### Run a bootc container in an ephemeral VM

```bash
bck run-rmvm <image>
```

This creates an ephemeral VM instantiated from the provided bootc container
image and logs in over SSH.

### Create a persistent VM

```bash
~/bin/bootc-kit-wrapper virt-install from-srb <image>
```

This creates a persistent libvirt VM using the specified bootc container image.

This will create a new login shell in an ephemeral VM.

## Implementation details

This project works by running the container in privileged
mode, which is then able to execute code in the host
context as necessary.

## Goals

This project aims to implement
<https://gitlab.com/fedora/bootc/tracker/-/issues/2>.

Related projects and content:

- https://github.com/coreos/coreos-assembler/
- https://github.com/ublue-os/bluefin-lts/blob/main/Justfile

## Development

See docs/HACKING.md

