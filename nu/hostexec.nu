# Verify that host execution succeeds
export def check [] {
    let r = /usr/libexec/bootc-kit-backend hostexec "true" o+e>/dev/null | complete | get exit_code
    ($r == 0)
}

# Execute a child process in the host environment, synchronously and
# checking for errors.
export def --wrapped main [...args] {
    do --capture-errors { /usr/libexec/bootc-kit-backend hostexec ...$args }
}
