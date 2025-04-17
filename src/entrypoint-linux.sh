#!/bin/bash
args=()
if [ -t 0 ]; then
    args+=(-t)
fi
podman run --rm -i ${args[@]} --privileged ghcr.io/bootc-dev/kit "$@"
