# Anaconda installer container for bcvk
#
# This container provides anaconda for installing bootc container images
# using kickstart files. It's designed to run as an ephemeral VM that
# boots into anaconda.target.
#
# Build: podman build -t localhost/anaconda-bootc:latest .

FROM quay.io/fedora/fedora-bootc:42

# Install anaconda and required packages
COPY packages.txt /tmp/packages.txt
RUN dnf install -y $(grep -v '^#' /tmp/packages.txt | tr '\n' ' ') && \
    dnf clean all && \
    rm /tmp/packages.txt

# Install bcvk setup service that runs before anaconda
COPY bcvk-anaconda-setup.service /usr/lib/systemd/system/bcvk-anaconda-setup.service
COPY bcvk-anaconda-setup.sh /usr/libexec/bcvk-anaconda-setup.sh
RUN chmod +x /usr/libexec/bcvk-anaconda-setup.sh

# Set anaconda.target as default and enable our setup service
RUN systemctl set-default anaconda.target && \
    systemctl enable bcvk-anaconda-setup.service
