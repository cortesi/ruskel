# v0.0.9

- Simplify handling of auto traits - they are now all included or not 
based on based on the `--auto-impls` flag.
- Render some trait implementations as derives, rather than impl blocks.

# v0.0.8

- Adapt to rustdoc JSON format changes

# v0.0.7

- Add --quiet flag, and corresponding arguments to libruskel
- Adapt to rustdoc JSON format changes

# v0.0.6

- More robust output paging
- Filters now work for trait impl fns
- Silence cargo output during rendering
- Correct error when running ruskel with no argument outside a crate
- Many bugfixes in target specification and filtering

