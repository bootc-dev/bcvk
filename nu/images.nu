export def "list" [] {
    /usr/libexec/bootc-kit-backend hostexec podman images --filter label=containers.bootc=1
}
