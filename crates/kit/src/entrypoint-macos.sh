#!/bin/bash
service=bootc-dev.kit
if ! launchctl list ${service} &>/dev/null; then
    launchctl submit -l ${service} -- 
args=()
if [ -t 0 ]; then
    args+=(-t)
fi
podman run --rm -i ${args[@]} --privileged ghcr.io/bootc-dev/kit "$@"
