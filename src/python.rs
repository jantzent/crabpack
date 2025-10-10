use std::path::PathBuf;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::error::CrabpackError;
use crate::{
    pack_with_skip_editable as pack_env_with_skip, Compressor, FilterKind, PackFilter, PackFormat,
    PackOptions,
};

#[pyfunction]
#[pyo3(
    signature = (
        prefix=None,
        output=None,
        format=None,
        python_prefix=None,
        verbose=false,
        force=false,
        compress_level=4,
        zip_symlinks=false,
        zip_64=true,
        filters=None,
        compressor=None,
        pigz_threads=None,
        skip_editable=false
    )
)]
fn pack(
    prefix: Option<&str>,
    output: Option<&str>,
    format: Option<&str>,
    python_prefix: Option<&str>,
    verbose: bool,
    force: bool,
    compress_level: u32,
    zip_symlinks: bool,
    zip_64: bool,
    filters: Option<Vec<(String, String)>>,
    compressor: Option<&str>,
    pigz_threads: Option<usize>,
    skip_editable: bool,
) -> PyResult<String> {
    let mut options = PackOptions::default();
    options.prefix = prefix.map(PathBuf::from);
    options.output = output.map(PathBuf::from);
    options.format = if let Some(fmt) = format {
        PackFormat::parse(fmt).map_err(to_pyerr)?
    } else {
        PackFormat::Infer
    };
    options.python_prefix = python_prefix.map(PathBuf::from);
    options.verbose = verbose;
    options.force = force;
    options.compress_level = compress_level;
    options.zip_symlinks = zip_symlinks;
    options.zip_64 = zip_64;
    if let Some(comp) = compressor {
        options.compressor = Compressor::parse(comp).map_err(to_pyerr)?;
    }
    options.pigz_threads = pigz_threads;

    if let Some(filter_list) = filters {
        let mut parsed = Vec::with_capacity(filter_list.len());
        for (kind, pattern) in filter_list {
            let kind = match kind.as_str() {
                "exclude" => FilterKind::Exclude,
                "include" => FilterKind::Include,
                other => {
                    return Err(PyRuntimeError::new_err(format!(
                        "Unknown filter kind '{other}'"
                    )))
                }
            };
            parsed.push(PackFilter { kind, pattern });
        }
        options.filters = parsed;
    }

    let result = pack_env_with_skip(options, skip_editable).map_err(to_pyerr)?;
    Ok(result.to_string_lossy().into_owned())
}

#[pymodule]
fn crabpack(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(pack, m)?)?;
    Ok(())
}

fn to_pyerr(err: CrabpackError) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}
