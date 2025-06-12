# ruskel

![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![Crates.io](https://img.shields.io/crates/v/libruskel.svg)](https://crates.io/crates/libruskel)
[![Documentation](https://docs.rs/libruskel/badge.svg)](https://docs.rs/libruskel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Ruskel produces a syntactically correct, single-page skeleton of a crate's
public API. If the crate is not found in the local workspace, it is fetched
from [crates.io](https://crates.io).

Ruskel is great for:

- Quick access to Rust documentation from the command line.
- Exporting the full public API of a crate as a single file to pass to LLMs and
  other tools.

For example, here is the skeleton of the very tiny `termsize` crate. Note that
the entire public API is included, but all implementation is omitted.

````rust
pub mod termsize {
    //! Termsize is a tiny crate that provides a simple
    //! interface for retrieving the current
    //! [terminal interface](http://www.manpagez.com/man/4/tty/) size
    //!
    //! ```rust
    //! extern crate termsize;
    //!
    //! termsize::get().map(|size| println!("rows {} cols {}", size.rows, size.cols));
    //! ```

    /// Container for number of rows and columns
    #[derive(Debug)]
    pub struct Size {
        pub rows: u16,
        pub cols: u16,
    }

    /// Gets the current terminal size
    pub fn get() -> Option<self::super::Size> {}
}
````


---

## Community

Want to contribute? Have ideas or feature requests? Come tell us about it on
[Discord](https://discord.gg/fHmRmuBDxF). 


---


## Features

- Generate a skeletonized view of any Rust crate
- Support for both local crates and remote crates from crates.io
- Syntax highlighting for terminal output 
- Optionally include private items and auto-implemented traits
- Support for custom feature flags and version specification

---

## Installation

To install Ruskel, run:

```sh
cargo install ruskel
```

---

## Usage


Ruskel uses nightly-only features on `cargo doc` for document generation, so you
need to have the nightly toolchain installed to run it, but not to install it.

Basic usage:

```sh
ruskel [TARGET]
```

See the help output for all options:

```sh
ruskel --help
```

Ruskel has a flexible target specification that tries to do the right thing in
a wide set of circumstances.

```sh
# Current project
ruskel

# If we're in a workspace and we have a crate mypacakage
ruskel mypackage

# A dependency of the current project, else we fetch from crates.io 
ruskel serde

# A sub-path within a crate
ruskel serde::de::Deserialize 

# Path to a crate
ruskel /my/path

# A module within that crate
ruskel /my/path::foo

# A crate from crates.io with a specific version
ruskel serde@1.0.0
```


---

## libruskel library

`libruskel` is a library that can be integrated into other Rust projects to
provide Ruskel functionality.

Here's a basic example of using `libruskel` in your Rust code:

```rust
use libruskel::Ruskel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rs = Ruskel::new("/path/to/target")?;
    let rendered = rs.render(false, false)?;
    println!("{}", rendered);
    Ok(())
}
```


