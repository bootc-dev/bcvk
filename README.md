# A toolkit for developing bootc containers

This repository is a container image which supports
installing bootc container images.

## Example

podman run --rm -ti --privileged ghcr.io/bootc-dev/kit run quay.io/exampleos/myos

This will create a new login shell in an ephemeral VM.
