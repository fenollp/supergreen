```shell
# Usage:
  cargo green supergreen env [ENV ...]                           Show used values
  cargo green supergreen doc [ENV ...]                           Documentation of said values
  cargo green fetch                                              Pulls images and crates
  cargo green supergreen sync                                    Pulls everything, for offline usage
  cargo green supergreen push                                    Push cache image (all tags)
  cargo green supergreen builder [ { recreate | rm } --clean ]   Manage local/remote builder
  cargo green supergreen -h | --help
  cargo green supergreen -V | --version
  cargo green ...any cargo subcommand...

# Try:
  cargo clean # Start from a clean slate
  cargo green build
  cargo supergreen env CARGOGREEN_BASE_IMAGE 2>/dev/null
  cargo supergreen help

# Suggestion:
  alias cargo='cargo green'
  # Now try, within your project:
  cargo fetch
  cargo test
```


