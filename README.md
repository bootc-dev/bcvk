# A toolkit for developing bootc containers

This repository is a container image which supports
installing bootc container images.

The core idea is that as much code as possible for this
comes as a container image. It does however run in
privileged mode so that it can access your host's
container storage and execute host services
where needed such as libvirt.

## Usage

### Initialize

```bash
podman run --rm -ti --privileged --pid=host ghcr.io/bootc-dev/kit init
```

This sets up some core infrastructure, such as running an instance of
https://github.com/cgwalters/cstor-dist

The user will be prompted to also set up a shell script alias in
`~/.local/bin/bck` or another short name of their choosing that
is an alias for the podman `run` command above.

### Run a bootc container in an ephemeral VM

The `run-rmvm <image>` creates an ephemeral VM instantiated from the provided
bootc container image and logs in over SSH.

### Create a persistent VM

The `virt-install` command creates a libvirt VM.

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