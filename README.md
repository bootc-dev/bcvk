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
# First, generate an entrypoint script for easy access
podman run --rm -ti --privileged --pid=host ghcr.io/bootc-dev/kit bootc-kit entrypoint --output ~/bin/bootc-kit-wrapper

# Now you can use the wrapper to initialize the infrastructure
~/bin/bootc-kit-wrapper init
```

This sets up some core infrastructure, such as running an instance of
https://github.com/cgwalters/cstor-dist

During initialization, you'll be prompted to set up a shell script alias in
`~/.local/bin/bck` or another location of your choosing to make accessing
bootc-kit easier.

### Run a bootc container in an ephemeral VM

```bash
~/bin/bootc-kit-wrapper run-rmvm <image>
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
- 

