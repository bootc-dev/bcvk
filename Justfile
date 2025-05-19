# Default task: Run checks
default: check

# Run all checks and formatting
all: fmt check test

# Run linters and formatters
fmt:
    cargo fmt --all -- --check

# Apply formatting changes
apply-fmt:
    cargo fmt --all

# Run clippy for linting
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Run tests
test:
    cargo test --all-features

# Check the project for errors
check:
    cargo check --all

# Creates a container image build
build-container:
    podman build -t ghcr.io/bootc-dev/kit .
