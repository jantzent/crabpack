# crabpack

`crabpack` is a Rust-native, high-performance reimplementation of the
[`venv-pack`](https://github.com/jcrist/venv-pack) tool for packaging Python
virtual environments. It provides a drop-in compatible command line interface
as well as optional Python bindings powered by [PyO3](https://pyo3.rs/).

The crate produces portable `.tar`, `.tar.gz`, `.tar.bz2`, or `.zip` archives of
existing Python virtual environments. During packing, `crabpack` rewrites
shebangs for executables, optionally relinks Python interpreters, and injects a
portable `activate` script (sourced from CPython).

## Command line usage

```text
Package an existing virtual environment into an archive file.

Usage: crabpack [OPTIONS]

Options:
  -p, --prefix <PATH>           Full path to environment prefix. Default is
                                current environment.
  -o, --output <PATH>           The path of the output file. Defaults to the
                                environment name with a `.tar.gz` suffix.
      --format <FORMAT>         The archival format to use. [default: infer]
                                [possible values: infer, zip, tar.gz, tgz,
                                tar.bz2, tbz2, tar]
      --python-prefix <PATH>    New prefix path for linking python in the
                                packaged environment.
      --compress-level <INT>    Compression level to use (0-9). Ignored for zip
                                archives. [default: 4]
      --compressor <COMPRESSOR>
                                Compressor to use for .tar.gz archives.
                                [default: auto] [possible values: auto, gzip,
                                pigz]
      --pigz-threads <INT>      Number of threads to use with pigz compression.
      --zip-symlinks            Store symbolic links in the zip archive instead
                                of the linked files.
      --no-zip-64               Disable ZIP64 extensions.
      --exclude <PATTERN>       Exclude files matching this pattern (can be
                                repeated).
      --include <PATTERN>       Re-add excluded files matching this pattern.
  -f, --force                   Overwrite any existing archive at the output
                                path.
  -q, --quiet                   Do not report progress.
      --version                 Show version then exit.
      --help                    Print help information.
```

Example:

```bash
crabpack --prefix /opt/envs/my-env --output my-env.tar.gz --format tar.gz \
  --exclude "*.pyc" --force
```

## Python bindings

The optional Python bindings expose an API compatible with `venv_pack.pack`. To
build them, enable the `python` feature:

```bash
maturin develop --features python
# or
cargo build --release --features python
```

Once built, the module can be imported from Python:

```python
import crabpack
crabpack.pack(prefix="/opt/envs/my-env", output="my-env.tar.gz", format="tar.gz")
```

## Development

```bash
cargo fmt
cargo check
cargo test
```

The repository includes integration assets under `assets/scripts` that are
embedded into produced archives. The script is derived from CPython's `venv`
implementation and distributed under the PSF license (see
`assets/scripts/CPYTHON_LICENSE.txt`).
