
# Core Guidelines

- Never commit code without explicit user confirmation. Just because the
  user has consented to one commit doesn't mean they consent to all future commits.
- When removing or changing code, never add comments about what was removed or
  changed. Comments in the code should always reflect what's there in the moment.
- You are an autonomous agent. You make use of all the tools available to you.
  You run instrument code and run tests and smoketests to gather information to
  solve problems. You iterate persistently until your requirements are met.
- You may create temporary files and directories as needed to solve problems,
  but always place them in the `./tmp/` directory.
- ALWAYS lint and fix all warnings before returning to the user.
- As a final step, ALWAYS format code before returning to the user.

# Active Code Maintenance

Every time you touch a piece of code, evaluate whether it can be improved
structurally. Ask questions like:

- Is the documentation for this function clear, concise and acccurate?
- Should the code be broken up into smaller pieces?
- Can the code be generalized or made more flexible?
- Is there related code that should be refactored to share functionality?
- Should the code be moved to a different location in the project?

You should improve the code continuously when opportunities arise, even if the
user hasn't explicitly asked for it. When you do active maintenance, include an
"Active Maintenance" section in your response message.

# Checklists

Whenever you're asked to produce a todo list or a checklist, you will use a
Markdown checklist, with numbered sections and items. Each item should be a
single, coherent change that leaves the system in a consistent state. Try not
to leave a broken system after any step, but certainly after a stage all tests
and smoketests must pass. Always include enough information that you could pick
it up again with zero context. Always wrap at 100 chars.

The checklist is a live document, update it as you go - if you discover new
items during your work, or leave items for a later commit, add them to the
checklist. Ensure that any new item you add is a also Markdown checklist item
(i.e. starts with `[ ]`), and has a number in sequence with other items in the
document.

You may batch together todo items that you think belong in the same changeset
without prompting me. After every batch, let me review the code before
committing. 

IMMEDIATELY tick off each item in the original checklist file on disk as you
complete them, so we don't lose track of where we are. Don't confuse your own
checklist with the user's checklist - update both your internal checklist and
the checklist on disk.

Example format:

```markdown
# Task description

Here is the context needed to understand the task, and an outline of its broad
aims.

1. Stage One: Frobnitz the flange

Perhaps some explanation and comments go here.

1. [ ] Do a thing!
2. [ ] Do thing two.

3. Stage Two: Retrofit the turbo-enabulator

Perhaps some explanation and comments go here.

1. [ ] Second section thing.
2. [ ] Second section thing 2.
```

# Git Commits 

Never commit until you're asked to do so, or the user has explicitly confirmed
they want to commit (the user will say "commit" or "do a git commit" or some
variant of that). Make git commit messages concise and clear. In the body of
the message, provide a concise summary of what's been done, but leave out
details like the validation process. Commit as the user - don't add model
attribution or co-authorship.

First, review the actual changes that are being committed.

```sh
# 1) Review, then stage explicitly (paths or -A).
git status --porcelain

# If necessary, review changes before staging:
git diff 
```

Formulate your commit message, based on the actual diff and the user's
instructions that lead up to this point. Make sure your message covers all
changed code, not just the user's latest prompt.

Next, stage and commit:

```sh
# Stage changes; use -A to stage all changes, or specify paths.
git add -A  # or: git add <paths>

# Commit via stdin; Conventional Commit subject (≤50). Body optional; blank
# line before body; quoted heredoc prevents interpolation.
git commit --cleanup=strip -F - <<'MSG'
feat(ui): concise example

Body
MSG
```



# Rust Development Guidelines

## General Guidelines

- Use Rust edition 2024.
- Avoid including code examples in documentation comments.
- Always introduce items from the standard library with a `use` declaration at
  the top of the file; do not reference `std` paths directly in the code body.
- Functions should not be nested inside other functions except in extremely
  rare cases where necessary.
- You should amost never add dead_code annotations. If you find yourself doing
  this, default to removing the code instead, unless there's a very good reason.
- Avoid feature gating unless absolutely necessary.
- Avoid configuring tests or any code component with environment variables. Prefer
  using function parameters or configuration structs.


## Crate structure

- Every crate that has methods that return `Result` should have a custom error
  type defined in `error.rs`, using `thiserror`.
- Each `lib.rs` file should contain the following lints:

```rust
#![warn(missing_docs)]
```

## Linting

Before committing, ensure all code passes lint checks and all warnings are
addressed. Run:

```bash
cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests --examples 2>&1
```

Clippy outputs warnings to stderr, which we merge into stdout so all messages
appear together. Resolve any lingering warnings manually. 

When addressing lint warnings, always step back and see if a deeper fix is
required. Sometimes the lints reveal bugs and weaknesses that should be
repaired. 

- If there are many lines of warnings, do fixes in batches  re-running clippy
  after each batch. Iterate through batches autonomously until all warnings are
  done.
- Do NOT simply allow lints without the user's OK - the lints are configured
  for a reason, and simply over-riding them should be very rare.
- When asked to de-nest deep functions or reduce function complexity, look at
  the function as a whole, and try to logically decompose it in a reusable way. 
- When warned about a result not being used, evaluate whether it SHOULD be used
  (i.e. if it's an error that should be handled), or a value that might be
  important. Do not simply assign to underscore unless it's warranted.
- Run unit tests to ensure that the project still works after every batch.


## Formatting

If you have nightly available, and `rustfmt-nightly.toml` format code with:


```bash
cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml
```

Otherwise, format with:

```bash
cargo +nightly fmt --all
```

ALWAYS format before committing.

## Testing

After completing changes, execute all tests using:

```bash
cargo test --all
```

Tests should always be placed in a "test" module when colocated with code.
There should only ever be a single test module per file. 

## Dependencies

Add dependencies by using the following command rather than editing
`Cargo.toml` directly:

```bash
cargo add <crate_name>
```

When adding a new dependency, do NOT specify a version unless absolutely
necessary. This will mean we pick up the latest version of the crate.

## Ruskel Tool Usage

The **ruskel** command-line utility generates Rust skeletons displaying the API
structure of crates, modules, structs, traits, functions, or any Rust
path—omitting implementation bodies. This tool is useful for reviewing names,
type signatures, derives, and documentation comments during code writing or
review. Always prefer ruskel over other inspection methods for Rust code.

Before any significant tool call (such as invoking ruskel), state in one line
the purpose and minimal required inputs.

Ruskel has to compile all dependencies, so the first run may take a while - run
the command with an extended timeout (e.g., 120 seconds) if needed. 

### When to Use ruskel
- Look up signatures or definitions of functions, traits, or structs.
- Explore public or private APIs.
- Find specific examples or crate documentation are needed.

### ruskel Usage Tips
- Request deep module paths (e.g., `ruskel tokio::sync::mpsc`) to stay within your
  token budget.
- Use the `ruskel --private` flag to view non-public items, which can be useful for
  nspecting your current codebase.

#### Examples

```sh
# Inspect the current project
ruskel

# Query a standard library trait
ruskel std::io::Read

# In a workspace with a crate 'mycrate'
ruskel mycrate

# View a method on a struct in the current crate
ruskel mycrate::Struct::method

# Explore a dependency or fetch from crates.io if not present
ruskel serde

# Look within a crate's module
ruskel serde::de::Deserialize

# Via filesystem path
ruskel /my/path

# Sub-module within a path
ruskel /my/path::foo

# Specific dependency version from crates.io
ruskel serde@1.0.0
```

</rust>


