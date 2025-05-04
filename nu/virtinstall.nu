#!/bin/nu

export def "main virtinstall" [
    image: string # container image to install
    --os:  string
] {
    let inspect = /usr/libexec/bootc-kit-backend hostexec podman image inspect $image | from json | first
    print $"Installing ($image) using ($os)"
}
