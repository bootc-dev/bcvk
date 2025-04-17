# Multi stage build for this Rust based project

# Build bootc from the current git into a c9s-bootc container image.
# Use e.g. --build-arg=base=quay.io/fedora/fedora-bootc:41 to target
# Fedora instead.
#
# You can also generate an image with cloud-init and other dependencies
# with `--build-arg=tmt` which is intended for use particularly via
# https://tmt.readthedocs.io/en/stable/
FROM registry.redhat.io/ubi9/ubi:latest as build
RUN <<EORUN
set -xeuo pipefail
dnf -y install cargo rustc
EORUN
COPY . /src
WORKDIR /src
# See https://www.reddit.com/r/rust/comments/126xeyx/exploring_the_problem_of_faster_cargo_docker/
# We aren't using the full recommendations there, just the simple bits.
RUN --mount=type=cache,target=/build/target \ 
    --mount=type=cache,target=/var/roothome \
    make && make install DESTDIR=/out 

FROM registry.redhat.io/ubi9/ubi:latest
COPY --from=build /out/ /
