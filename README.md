# A toolkit for developing bootc containers

This repository is a container image which supports
installing bootc container images.

## Example

podman run --rm -ti --privileged ghcr.io/bootc-dev/kit run quay.io/exampleos/myos

This will create a new login shell in an ephemeral VM.

## Implementation details

This project works by running the container in privileged
mode, which is then able to execute code in the host
context as necessary.
