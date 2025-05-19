## Building

- There is a `Justfile` which supports commands, read it and use it.
  This wraps generic tools like `cargo check`, `cargo build`, and `cargo test` to verify
  the project compiles and unit tests work. This assumes these tools are in the current
  host environment.
- The actual build process though is via `just build-container`
  as the primary way this project runs is via podman, and then use
  `just test-integration` to run the integration tests.

## Running

Ensure the entrypoint script is in `$PATH`, i.e. that `bck` works.

Then you can invoke `bck`.

## Code formatting

- Always run `cargo fmt` before making a git commit, and in
  general at the end of a series of code edits.

## Commit messages

The commit message should be structured as follows:

<type>[optional scope]: <description>

[optional body]

[optional footer]

The commit contains the following structural elements, to communicate intent to the consumers of your library:

- fix: a commit of the type fix patches a bug in your codebase (this correlates with PATCH in semantic versioning).
- feat: a commit of the type feat introduces a new feature to the codebase (this correlates with MINOR in semantic versioning).
- BREAKING CHANGE: a commit that has the text BREAKING CHANGE: at the beginning of its optional body or footer section introduces a breaking API change (correlating with MAJOR in semantic versioning). A breaking change can be part of commits of any type. e.g., a fix:, feat: & chore: types would all be valid, in addition to any other type.

DO NOT include `Generated with Claude Code` or `Co-authored-by: Claude`.
You should include `Assisted-by: Claude <noreply@anthropic.com>` though
especially for nontrivial changes that did not require substantial assistance from
a human.
