# itest — integration test framework

Reusable integration test infrastructure for bootc-dev projects.
Built on [libtest-mimic] with automatic test registration via
[linkme] distributed slices.

## Quick start

Create a binary crate with `harness = false` and register tests
with macros:

```rust
fn my_test() -> itest::TestResult {
    // your test logic
    Ok(())
}
itest::integration_test!(my_test);

fn main() {
    itest::run_tests();
}
```

## Test types

### Plain tests

```rust
fn test_something() -> itest::TestResult {
    assert_eq!(2 + 2, 4);
    Ok(())
}
itest::integration_test!(test_something);
```

### Parameterized tests

Expanded once per parameter value configured in `TestConfig::parameters`:

```rust
fn test_with_image(image: &str) -> itest::TestResult {
    println!("testing with {image}");
    Ok(())
}
itest::parameterized_integration_test!(test_with_image);

fn main() {
    let config = itest::TestConfig {
        parameters: vec![
            "quay.io/fedora/fedora-bootc:42".into(),
            "quay.io/centos-bootc/centos-bootc:stream10".into(),
        ],
        ..Default::default()
    };
    itest::run_tests_with_config(config);
}
```

### Privileged tests

Tests that need root. When run unprivileged, the harness
automatically dispatches them inside a bcvk ephemeral VM:

```rust
itest::privileged_test!("my-binary", test_needs_root, {
    assert!(rustix::process::getuid().is_root());
    Ok(())
});
```

An optional `itype` parameter specifies the VM instance type
(note the trailing comma):

```rust
itest::privileged_test!("my-binary", big_test, itype = "u1.large", {
    // runs in a VM with 2 vCPU / 8 GiB
    Ok(())
});
```

### Test metadata

Any test can carry metadata that flows into tmt and autopkgtest
output. Metadata is declared inline via a `const` block:

```rust
fn slow_network_test() -> itest::TestResult {
    Ok(())
}
itest::integration_test!(slow_network_test, meta = const {
    itest::TestMeta {
        timeout: Some("1h"),
        needs_root: true,
        isolation: itest::Isolation::Machine,
        tags: &["slow", "network"],
        summary: Some("A test that needs internet and a full VM"),
        needs_internet: true,
        flaky: true,
        ..itest::TestMeta::EMPTY
    }
});
```

Fields and how they map to each format:

| Field | tmt (FMF) | autopkgtest (DEP-8) |
|---|---|---|
| `timeout` | `duration:` | *(global only)* |
| `needs_root` | *(plan-level)* | `Restrictions: needs-root` |
| `isolation` | *(plan-level)* | `Restrictions: isolation-{container,machine}` |
| `tags` | `tag:` | `Classes:` |
| `summary` | `summary:` | *(none)* |
| `needs_internet` | *(plan-level)* | `Restrictions: needs-internet` |
| `flaky` | `result: xfail` | `Restrictions: flaky` |

All fields are optional. Tests without `meta = const { ... }` get
sensible defaults (20m timeout, no restrictions).

### Booted tests

Like privileged tests, but dispatched via `bcvk libvirt run` which
does a full `bootc install to-disk`:

```rust
itest::booted_test!("my-binary", test_ostree, {
    // runs inside a real booted ostree deployment
    Ok(())
});
```

## Running tests

The harness supports multiple test runners. It auto-detects which
runner is active and adapts its behavior.

### cargo test (built-in capture)

```bash
cargo test -p my-tests
```

When no external runner is detected, itest automatically captures
output by re-executing itself per test (fork-exec). Passing test
output is suppressed; failing test output is shown — matching the
default `cargo test` behavior.

Set `ITEST_NOCAPTURE=1` to disable capture for debugging:

```bash
ITEST_NOCAPTURE=1 cargo test -p my-tests
```

### cargo-nextest

```bash
cargo nextest run -P integration -p my-tests
```

[nextest] runs each test as a separate process natively, so the
harness detects this (via the `NEXTEST` env var) and skips its own
fork-exec layer. nextest provides additional features like retries,
timing reports, and better parallelism control.

### tmt

[tmt] discovers tests from FMF metadata files. Generate them from
the test binary:

```bash
my-tests --emit-tmt tmt/tests/
```

This creates a `tests.fmf` file where each registered test becomes
an entry like:

```yaml
/test_something:
  summary: test_something
  test: my-tests --exact test_something
  duration: 20m
```

Then run with tmt:

```bash
tmt run --all provision --how local --feeling-safe
```

The harness detects tmt (via `TMT_TEST_DATA`) and runs tests
directly without the fork-exec capture layer — tmt handles
per-test output isolation itself.

### autopkgtest (DEP-8)

[autopkgtest] discovers tests from a `debian/tests/control` file.
Generate it:

```bash
my-tests --emit-autopkgtest debian/tests/
```

This creates a `control` file with one stanza per test:

```
Test-Command: my-tests --exact test_something
Features: test-name=test_something
Restrictions: needs-root
```

Then run with autopkgtest:

```bash
autopkgtest -- null      # run on localhost
autopkgtest -- qemu ...  # run in a QEMU VM
```

## Environment variables

| Variable | Effect |
|---|---|
| `ITEST_NOCAPTURE=1` | Disable fork-exec output capture |
| `ITEST_SUBPROCESS=1` | Set internally; marks a fork-exec child |
| `ITEST_IN_VM=1` | Set internally; recursion guard for VM dispatch |
| `ITEST_IMAGE` | Container image for VM dispatch (required when not root) |
| `BCVK_PATH` | Path to the `bcvk` binary (default: `bcvk`) |
| `JUNIT_OUTPUT` | Path to write JUnit XML results |

## Architecture

```
┌────────────────────────────────────────────────────┐
│                    test binary                     │
│  ┌──────────────┐  ┌──────────────────────────┐   │
│  │ integration_ │  │ parameterized_           │   │
│  │ test! macros  │  │ integration_test! macros │   │
│  └──────┬───────┘  └────────────┬─────────────┘   │
│         │ linkme distributed    │                  │
│         │ slices                │                  │
│  ┌──────▼───────────────────────▼─────────────┐   │
│  │          run_tests_with_config()           │   │
│  │                                             │   │
│  │  ┌─────────┐  ┌───────────┐  ┌──────────┐ │   │
│  │  │--emit-  │  │--emit-    │  │ fork-exec│ │   │
│  │  │tmt      │  │autopkgtest│  │ capture  │ │   │
│  │  └─────────┘  └───────────┘  └──────────┘ │   │
│  │                                             │   │
│  │  ┌─────────────────────────────────────┐   │   │
│  │  │        libtest-mimic                │   │   │
│  │  │  (filtering, --list, --exact, etc.) │   │   │
│  │  └─────────────────────────────────────┘   │   │
│  └─────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────┘
         │                              │
    ┌────▼─────┐                  ┌─────▼──────┐
    │ nextest  │                  │    tmt /    │
    │ (native) │                  │ autopkgtest │
    └──────────┘                  └────────────┘
```

[libtest-mimic]: https://crates.io/crates/libtest-mimic
[linkme]: https://crates.io/crates/linkme
[nextest]: https://nexte.st
[tmt]: https://tmt.readthedocs.io
[autopkgtest]: https://wiki.debian.org/ContinuousIntegration/autopkgtest
