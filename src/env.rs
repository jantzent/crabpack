use std::borrow::Cow;
use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use globset::GlobBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;
use regex::bytes::Regex;
use tempfile::NamedTempFile;

use crate::archive::{Archive, ArchiveFormat, ArchiveOptions, Compressor, BIN_DIR};
use crate::error::{CrabpackError, Result};

/// Representation of a file (or directory) to be added to the archive.
#[derive(Clone, Debug)]
pub struct FileRecord {
    pub source: PathBuf,
    pub target: PathBuf,
}

impl FileRecord {
    fn target_string(&self) -> Cow<'_, str> {
        path_to_string(&self.target)
    }
}

/// A virtual environment ready to be packed.
#[derive(Clone, Debug)]
pub struct Env {
    context: Context,
    files: Vec<FileRecord>,
    excluded: Vec<FileRecord>,
}

impl Env {
    pub fn new(prefix: Option<PathBuf>) -> Result<Self> {
        Self::new_with_skip_editable(prefix, false)
    }

    pub fn new_with_skip_editable(prefix: Option<PathBuf>, skip_editable: bool) -> Result<Self> {
        let context = check_prefix(prefix)?;
        check_no_editable_packages(&context, skip_editable)?;
        let files = collect_environment_files(&context)?;
        Ok(Env {
            context,
            files,
            excluded: Vec::new(),
        })
    }

    pub fn prefix(&self) -> &Path {
        &self.context.prefix
    }

    pub fn kind(&self) -> EnvKind {
        self.context.kind
    }

    pub fn orig_prefix(&self) -> &Path {
        &self.context.orig_prefix
    }

    pub fn name(&self) -> String {
        self.context
            .prefix
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "environment".to_string())
    }

    pub fn files(&self) -> &[FileRecord] {
        &self.files
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn exclude(&self, pattern: &str) -> Result<Self> {
        let matcher = build_glob(pattern)?;
        let mut files = Vec::with_capacity(self.files.len());
        let mut excluded = self.excluded.clone();
        for file in &self.files {
            if matcher.is_match(file.target_string().as_ref()) {
                excluded.push(file.clone());
            } else {
                files.push(file.clone());
            }
        }
        Ok(Env {
            context: self.context.clone(),
            files,
            excluded,
        })
    }

    pub fn include(&self, pattern: &str) -> Result<Self> {
        let matcher = build_glob(pattern)?;
        let mut files = self.files.clone();
        let mut excluded = Vec::new();
        for file in &self.excluded {
            if matcher.is_match(file.target_string().as_ref()) {
                files.push(file.clone());
            } else {
                excluded.push(file.clone());
            }
        }
        Ok(Env {
            context: self.context.clone(),
            files,
            excluded,
        })
    }

    pub fn pack(&self, options: &PackOptions) -> Result<PathBuf> {
        if options.compress_level > 9 {
            return Err(CrabpackError::user(
                "compress-level must be between 0 and 9",
            ));
        }

        let (output_path, archive_format) =
            resolve_output_and_format(self, options.output.clone(), options.format)?;

        if options.compressor == Compressor::Pigz && archive_format != ArchiveFormat::TarGz {
            return Err(CrabpackError::user(
                "pigz compression can only be used with the tar.gz format",
            ));
        }

        if let Some(count) = options.pigz_threads {
            if count == 0 {
                return Err(CrabpackError::user(
                    "pigz-threads must be greater than zero",
                ));
            }
        }

        if output_path.exists() && !options.force {
            return Err(CrabpackError::user(format!(
                "File {:?} already exists",
                output_path
            )));
        }

        if options.verbose {
            println!(
                "Packing environment at {:?} to {:?}",
                self.context.prefix, output_path
            );
        }

        let tmp_dir = temp_dir_for_output(&output_path)?;
        let tmp = NamedTempFile::new_in(tmp_dir)?;
        let file = tmp.reopen()?;
        let archive_options = ArchiveOptions {
            compress_level: options.compress_level,
            zip_symlinks: options.zip_symlinks,
            zip_64: options.zip_64,
            compressor: options.compressor,
            pigz_threads: options.pigz_threads,
        };
        let mut archive = Archive::new(file, archive_format, archive_options)?;

        let (python_prefix, rewrites) =
            check_python_prefix(options.python_prefix.clone(), &self.context)?;
        let packer = Packer::new(&self.context, python_prefix, rewrites);

        let progress = if options.verbose {
            let pb = ProgressBar::new(self.files.len() as u64);
            pb.set_style(
                ProgressStyle::with_template(
                    "[{bar:40}] | {percent}% Completed | {elapsed_precise}",
                )
                .unwrap()
                .progress_chars("#>-"),
            );
            Some(pb)
        } else {
            None
        };

        for file in &self.files {
            packer.add(&mut archive, file)?;
            if let Some(pb) = &progress {
                pb.inc(1);
            }
        }

        if let Some(pb) = &progress {
            pb.finish_with_message("Completed");
        }

        packer.finish(&mut archive)?;
        archive.finish()?;

        tmp.persist(&output_path).map_err(|e| e.error)?;

        Ok(output_path)
    }
}

/// Create an archive directly from configuration options.
pub fn pack(options: PackOptions) -> Result<PathBuf> {
    pack_with_skip_editable(options, false)
}

pub fn pack_with_skip_editable(mut options: PackOptions, skip_editable: bool) -> Result<PathBuf> {
    let mut env = Env::new_with_skip_editable(options.prefix.take(), skip_editable)?;
    for filter in &options.filters {
        match filter.kind {
            FilterKind::Exclude => env = env.exclude(&filter.pattern)?,
            FilterKind::Include => env = env.include(&filter.pattern)?,
        }
    }
    env.pack(&options)
}

#[derive(Clone, Debug, Default)]
pub struct PackOptions {
    pub prefix: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub format: PackFormat,
    pub python_prefix: Option<PathBuf>,
    pub verbose: bool,
    pub force: bool,
    pub compress_level: u32,
    pub zip_symlinks: bool,
    pub zip_64: bool,
    pub compressor: Compressor,
    pub pigz_threads: Option<usize>,
    pub filters: Vec<PackFilter>,
}

impl Default for PackFormat {
    fn default() -> Self {
        PackFormat::Infer
    }
}

#[derive(Clone, Debug)]
pub struct PackFilter {
    pub kind: FilterKind,
    pub pattern: String,
}

#[derive(Clone, Copy, Debug)]
pub enum FilterKind {
    Include,
    Exclude,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PackFormat {
    Infer,
    Zip,
    TarGz,
    TarBz2,
    Tar,
}

impl PackFormat {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "infer" => Ok(PackFormat::Infer),
            "zip" => Ok(PackFormat::Zip),
            "tar.gz" | "tgz" => Ok(PackFormat::TarGz),
            "tar.bz2" | "tbz2" => Ok(PackFormat::TarBz2),
            "tar" => Ok(PackFormat::Tar),
            other => Err(CrabpackError::user(format!("Unknown format '{other}'"))),
        }
    }
}

#[derive(Clone, Debug)]
struct Context {
    prefix: PathBuf,
    orig_prefix: PathBuf,
    py_lib: PathBuf,
    py_include: PathBuf,
    kind: EnvKind,
}

impl Context {
    fn py_lib_path(&self) -> PathBuf {
        self.prefix.join(&self.py_lib)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvKind {
    Venv,
    Virtualenv,
}

struct Packer<'a> {
    context: &'a Context,
    python_prefix: Option<PathBuf>,
    rewrites: Vec<(String, String)>,
}

impl<'a> Packer<'a> {
    fn new(
        context: &'a Context,
        python_prefix: Option<PathBuf>,
        rewrites: Vec<(String, String)>,
    ) -> Self {
        Packer {
            context,
            python_prefix,
            rewrites,
        }
    }

    fn add(&self, archive: &mut Archive, file: &FileRecord) -> Result<()> {
        let metadata = fs::symlink_metadata(&file.source)?;
        if metadata.file_type().is_symlink() {
            let link_target = fs::read_link(&file.source)?;
            let link_string = link_target.to_string_lossy();
            for (orig, new) in &self.rewrites {
                if link_string.starts_with(orig) {
                    let replaced = link_string.replacen(orig, new, 1);
                    let new_target = PathBuf::from(replaced);
                    return archive.add_link(&file.source, &new_target, &file.target);
                }
            }
            archive.add(&file.source, &file.target)
        } else if metadata.is_dir() {
            archive.add(&file.source, &file.target)
        } else if is_in_bin_dir(&file.target) {
            let data = fs::read(&file.source)?;
            let (rewritten, changed) = rewrite_shebang(&data, &self.context.prefix);
            if changed {
                archive.add_bytes(&file.source, &rewritten, &file.target)
            } else {
                archive.add(&file.source, &file.target)
            }
        } else {
            archive.add(&file.source, &file.target)
        }
    }

    fn finish(&self, archive: &mut Archive) -> Result<()> {
        let script_target = Path::new(BIN_DIR).join("activate");
        archive.add_bytes_with_mode(ACTIVATE_SCRIPT, &script_target, Some(0o100755))?;

        match self.context.kind {
            EnvKind::Venv => {
                let cfg_path = self.context.prefix.join("pyvenv.cfg");
                if let Some(prefix) = &self.python_prefix {
                    let data = fs::read_to_string(&cfg_path)?;
                    let old_prefix = self.context.orig_prefix.to_string_lossy();
                    let new_prefix = prefix.to_string_lossy();
                    let replaced = data.replace(old_prefix.as_ref(), new_prefix.as_ref());
                    archive.add_bytes_with_mode(
                        replaced.as_bytes(),
                        Path::new("pyvenv.cfg"),
                        Some(0o100644),
                    )?
                } else {
                    archive.add(&cfg_path, Path::new("pyvenv.cfg"))?;
                }
            }
            EnvKind::Virtualenv => {
                let orig_prefix_path = self.context.py_lib.join("orig-prefix.txt");
                let source_path = self.context.prefix.join(&orig_prefix_path);
                if let Some(prefix) = &self.python_prefix {
                    let prefix_string = prefix.to_string_lossy().into_owned();
                    archive.add_bytes_with_mode(
                        prefix_string.as_bytes(),
                        &orig_prefix_path,
                        Some(0o100644),
                    )?;
                } else {
                    archive.add(&source_path, &orig_prefix_path)?;
                }
            }
        }

        Ok(())
    }
}

fn resolve_output_and_format(
    env: &Env,
    output: Option<PathBuf>,
    requested: PackFormat,
) -> Result<(PathBuf, ArchiveFormat)> {
    match requested {
        PackFormat::Infer => {
            if let Some(path) = output {
                infer_format_from_output(path)
            } else {
                let filename = format!("{}.tar.gz", env.name());
                Ok((PathBuf::from(filename), ArchiveFormat::TarGz))
            }
        }
        PackFormat::Zip => Ok((
            output.unwrap_or_else(|| PathBuf::from(format!("{}.zip", env.name()))),
            ArchiveFormat::Zip,
        )),
        PackFormat::TarGz => Ok((
            output.unwrap_or_else(|| PathBuf::from(format!("{}.tar.gz", env.name()))),
            ArchiveFormat::TarGz,
        )),
        PackFormat::TarBz2 => Ok((
            output.unwrap_or_else(|| PathBuf::from(format!("{}.tar.bz2", env.name()))),
            ArchiveFormat::TarBz2,
        )),
        PackFormat::Tar => Ok((
            output.unwrap_or_else(|| PathBuf::from(format!("{}.tar", env.name()))),
            ArchiveFormat::Tar,
        )),
    }
}

fn temp_dir_for_output(output_path: &Path) -> Result<PathBuf> {
    if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Ok(parent.to_path_buf())
    } else {
        Ok(env::current_dir()?)
    }
}

fn infer_format_from_output(path: PathBuf) -> Result<(PathBuf, ArchiveFormat)> {
    let lowered = path_to_string(&path).to_ascii_lowercase();
    if lowered.ends_with(".zip") {
        Ok((path, ArchiveFormat::Zip))
    } else if lowered.ends_with(".tar.gz") || lowered.ends_with(".tgz") {
        Ok((path, ArchiveFormat::TarGz))
    } else if lowered.ends_with(".tar.bz2") || lowered.ends_with(".tbz2") {
        Ok((path, ArchiveFormat::TarBz2))
    } else if lowered.ends_with(".tar") {
        Ok((path, ArchiveFormat::Tar))
    } else {
        Err(CrabpackError::user(format!(
            "Unknown file extension '{}'",
            path_to_string(&path)
        )))
    }
}

fn check_prefix(prefix: Option<PathBuf>) -> Result<Context> {
    let base_prefix = match prefix {
        Some(path) => path,
        None => env::var_os("VIRTUAL_ENV")
            .map(PathBuf::from)
            .ok_or_else(|| {
                CrabpackError::user("Current environment is not a virtual environment")
            })?,
    };

    let prefix = absolutize(&base_prefix)?;

    if !prefix.exists() {
        return Err(CrabpackError::user(format!(
            "Environment path {:?} doesn't exist",
            prefix
        )));
    }

    if let Ok(ctx) = check_venv(&prefix) {
        return Ok(ctx);
    }
    if let Ok(ctx) = check_virtualenv(&prefix) {
        return Ok(ctx);
    }

    Err(CrabpackError::user(format!(
        "{:?} is not a valid virtual environment",
        prefix
    )))
}

fn check_venv(prefix: &Path) -> Result<Context> {
    let cfg_path = prefix.join("pyvenv.cfg");
    if !cfg_path.exists() {
        return Err(CrabpackError::user("not a venv"));
    }

    let mut home = None;
    let mut file = File::open(&cfg_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    for line in contents.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("home") {
                let path = PathBuf::from(value.trim());
                home = Some(path);
                break;
            }
        }
    }

    let home = home.ok_or_else(|| CrabpackError::user("Invalid pyvenv.cfg"))?;
    let orig_prefix = home
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| CrabpackError::user("Invalid pyvenv.cfg"))?;

    let (py_lib, py_include) = find_python_lib_include(prefix)?;

    Ok(Context {
        prefix: prefix.to_path_buf(),
        orig_prefix,
        py_lib,
        py_include,
        kind: EnvKind::Venv,
    })
}

fn check_virtualenv(prefix: &Path) -> Result<Context> {
    let (py_lib, py_include) = find_python_lib_include(prefix)?;
    let orig_prefix_path = prefix.join(&py_lib).join("orig-prefix.txt");
    if !orig_prefix_path.exists() {
        return Err(CrabpackError::user("not a virtualenv"));
    }

    let mut file = File::open(&orig_prefix_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let orig_prefix = PathBuf::from(contents.trim());

    Ok(Context {
        prefix: prefix.to_path_buf(),
        orig_prefix,
        py_lib,
        py_include,
        kind: EnvKind::Virtualenv,
    })
}

fn find_python_lib_include(prefix: &Path) -> Result<(PathBuf, PathBuf)> {
    if cfg!(windows) {
        return Ok((PathBuf::from("Lib"), PathBuf::from("Include")));
    }

    let lib_dir = prefix.join("lib");
    if !lib_dir.exists() {
        return Err(CrabpackError::user(format!(
            "Unexpected failure, no version of python found in prefix {:?}",
            prefix
        )));
    }

    let mut python_dirs = Vec::new();
    for entry in fs::read_dir(&lib_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("python") {
                python_dirs.push(PathBuf::from(name));
            }
        }
    }

    if python_dirs.len() > 1 {
        return Err(CrabpackError::user(format!(
            "Unexpected failure, multiple versions of python found in prefix {:?}",
            prefix
        )));
    } else if python_dirs.is_empty() {
        return Err(CrabpackError::user(format!(
            "Unexpected failure, no version of python found in prefix {:?}",
            prefix
        )));
    }

    let python_dir = python_dirs.pop().unwrap();
    Ok((
        PathBuf::from("lib").join(&python_dir),
        PathBuf::from("include").join(python_dir),
    ))
}

fn check_no_editable_packages(context: &Context, skip_editable: bool) -> Result<()> {
    let site_packages = context.py_lib_path().join("site-packages");
    if !site_packages.exists() {
        return Ok(());
    }

    let mut editable = HashSet::new();
    for entry in fs::read_dir(&site_packages)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry_path
            .extension()
            .map(|ext| ext == "pth")
            .unwrap_or(false)
        {
            let mut file = File::open(&entry_path)?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            let dirname = entry_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| context.prefix.clone());
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let location = PathBuf::from(trimmed);
                let path = if location.is_absolute() {
                    location
                } else {
                    dirname.join(&location)
                };
                if !path.starts_with(&context.prefix) {
                    editable.insert(trimmed.to_string());
                }
            }
        }
    }

    let mut list: Vec<_> = editable.into_iter().collect();
    if list.is_empty() {
        return Ok(());
    }

    list.sort();
    let details = list
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");

    if skip_editable {
        eprintln!(
            "Editable packages found outside the environment will be skipped:\n\n{}",
            details
        );
        return Ok(());
    }

    Err(CrabpackError::user(format!(
        "Cannot pack an environment with editable packages\ninstalled (e.g. from `python setup.py develop` or\n `pip install -e`). Editable packages found:\n\n{}",
        details
    )))
}

fn collect_environment_files(context: &Context) -> Result<Vec<FileRecord>> {
    let mut results = Vec::new();
    for entry in fs::read_dir(&context.prefix)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(&context.prefix).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || metadata.is_file() {
            results.push(rel);
        } else if metadata.is_dir() {
            collect_directory(&context.prefix, &path, &mut results)?;
        }
    }

    let mut remove = HashSet::new();
    for script in ["activate", "activate.csh", "activate.fish"] {
        remove.insert(PathBuf::from(BIN_DIR).join(script));
    }
    match context.kind {
        EnvKind::Virtualenv => {
            remove.insert(context.py_lib.join("orig-prefix.txt"));
        }
        EnvKind::Venv => {
            remove.insert(PathBuf::from("pyvenv.cfg"));
        }
    }

    let mut files = Vec::new();
    for rel in results {
        if remove.contains(&rel) {
            continue;
        }
        let name = rel.to_string_lossy();
        if name.ends_with('~') || name.ends_with(".DS_STORE") {
            continue;
        }
        files.push(FileRecord {
            source: context.prefix.join(&rel),
            target: rel,
        });
    }

    Ok(files)
}

fn collect_directory(prefix: &Path, dir: &Path, results: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)?;
    let mut has_entries = false;
    while let Some(entry) = entries.next() {
        let entry = entry?;
        has_entries = true;
        let path = entry.path();
        let rel = path.strip_prefix(prefix).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            results.push(rel);
        } else if metadata.is_dir() {
            collect_directory(prefix, &path, results)?;
        } else if metadata.is_file() {
            results.push(rel);
        }
    }

    if !has_entries {
        results.push(dir.strip_prefix(prefix).unwrap().to_path_buf());
    }

    Ok(())
}

fn is_in_bin_dir(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Normal(component)) => component == BIN_DIR,
        _ => false,
    }
}

fn absolutize(path: &Path) -> io::Result<PathBuf> {
    let mut absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    if cfg!(windows) {
        while absolute
            .as_os_str()
            .to_string_lossy()
            .ends_with(std::path::MAIN_SEPARATOR)
            && absolute.components().count() > 1
        {
            absolute.pop();
        }
    }
    Ok(absolute)
}

fn build_glob(pattern: &str) -> Result<globset::GlobMatcher> {
    let mut builder = GlobBuilder::new(pattern);
    if cfg!(windows) {
        builder.case_insensitive(true);
    }
    let glob = builder
        .build()
        .map_err(|e| CrabpackError::user(format!("Invalid filter pattern {pattern:?}: {e}")))?;
    Ok(glob.compile_matcher())
}

static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^(?P<whole>#![ ]*(?P<path>/(?:\\ |[^ \n\r\t])*)(?P<opts>.*))$")
        .expect("failed to compile shebang regex")
});

fn rewrite_shebang(data: &[u8], prefix: &Path) -> (Vec<u8>, bool) {
    if !data.starts_with(b"#!") {
        return (data.to_vec(), false);
    }

    let prefix_string = prefix.to_string_lossy().into_owned();
    if prefix_string.is_empty() {
        return (data.to_vec(), false);
    }
    let prefix_bytes = prefix_string.as_bytes();
    if data
        .windows(prefix_bytes.len())
        .filter(|slice| *slice == prefix_bytes)
        .take(2)
        .count()
        > 1
    {
        return (data.to_vec(), false);
    }

    if let Some(caps) = SHEBANG_REGEX.captures(data) {
        let whole = caps.name("whole").unwrap();
        let executable = caps.name("path").unwrap();
        let options = caps.name("opts").map(|m| m.as_bytes()).unwrap_or(b"");
        if executable.as_bytes().starts_with(prefix_bytes) {
            let exec_name = executable
                .as_bytes()
                .rsplit(|b| *b == b'/')
                .next()
                .unwrap_or(executable.as_bytes());
            let mut replacement = b"#!/usr/bin/env ".to_vec();
            replacement.extend_from_slice(exec_name);
            replacement.extend_from_slice(options);
            let mut new_data = data.to_vec();
            new_data.splice(whole.start()..whole.end(), replacement);
            return (new_data, true);
        }
    }

    (data.to_vec(), false)
}

fn check_python_prefix(
    python_prefix: Option<PathBuf>,
    context: &Context,
) -> Result<(Option<PathBuf>, Vec<(String, String)>)> {
    let Some(prefix) = python_prefix else {
        return Ok((None, Vec::new()));
    };

    if !prefix.is_absolute() {
        return Err(CrabpackError::user(
            "python-prefix must be an absolute path",
        ));
    }

    let normalized: PathBuf = prefix.components().collect();

    let mut rewrites = Vec::new();

    match context.kind {
        EnvKind::Venv => {
            let python_component: PathBuf = if cfg!(windows) {
                PathBuf::from("python")
            } else {
                context
                    .py_lib
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("python"))
            };
            let original = context.orig_prefix.join(BIN_DIR).join(&python_component);
            let mut new_target = normalized.join(BIN_DIR);
            new_target.push(&python_component);
            rewrites.push((
                path_to_string(&original).into_owned(),
                path_to_string(&new_target).into_owned(),
            ));
        }
        EnvKind::Virtualenv => {
            rewrites.push((
                ensure_trailing_sep(&context.orig_prefix.join(&context.py_lib)),
                ensure_trailing_sep(&normalized.join(&context.py_lib)),
            ));
            rewrites.push((
                path_to_string(&context.orig_prefix.join(&context.py_include)).into_owned(),
                path_to_string(&normalized.join(&context.py_include)).into_owned(),
            ));
        }
    }

    Ok((Some(normalized), rewrites))
}

fn ensure_trailing_sep(path: &Path) -> String {
    let mut value = path_to_string(path).into_owned();
    if !value.ends_with(std::path::MAIN_SEPARATOR) {
        value.push(std::path::MAIN_SEPARATOR);
    }
    value
}

fn path_to_string(path: &Path) -> Cow<'_, str> {
    path.to_string_lossy()
}

static ACTIVATE_SCRIPT: &[u8] = include_bytes!("../assets/scripts/common/activate");

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn dummy_context(prefix: &Path, kind: EnvKind) -> Context {
        Context {
            prefix: prefix.to_path_buf(),
            orig_prefix: prefix.to_path_buf(),
            py_lib: PathBuf::from("lib/python3.11"),
            py_include: PathBuf::from("include/python3.11"),
            kind,
        }
    }

    fn create_env(prefix_name: &str) -> Env {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join(prefix_name);
        fs::create_dir_all(&prefix).unwrap();
        Env {
            context: dummy_context(&prefix, EnvKind::Venv),
            files: Vec::new(),
            excluded: Vec::new(),
        }
    }

    #[test]
    fn pack_format_parse_recognizes_aliases() {
        assert_eq!(PackFormat::parse("infer").unwrap(), PackFormat::Infer);
        assert_eq!(PackFormat::parse("zip").unwrap(), PackFormat::Zip);
        assert_eq!(PackFormat::parse("tar.gz").unwrap(), PackFormat::TarGz);
        assert_eq!(PackFormat::parse("tgz").unwrap(), PackFormat::TarGz);
        assert_eq!(PackFormat::parse("tar.bz2").unwrap(), PackFormat::TarBz2);
        assert_eq!(PackFormat::parse("tbz2").unwrap(), PackFormat::TarBz2);
        assert_eq!(PackFormat::parse("tar").unwrap(), PackFormat::Tar);
    }

    #[test]
    fn pack_format_parse_rejects_unknown_values() {
        let err = PackFormat::parse("rar").unwrap_err();
        assert!(format!("{err}").contains("Unknown format"));
    }

    #[test]
    fn resolve_output_defaults_to_tar_gz_when_inferred() {
        let env = create_env("dummy");
        let (output, format) = resolve_output_and_format(&env, None, PackFormat::Infer).unwrap();
        assert_eq!(format, ArchiveFormat::TarGz);
        assert_eq!(output.file_name().unwrap(), "dummy.tar.gz");
    }

    #[test]
    fn resolve_output_respects_explicit_extension() {
        let env = create_env("dummy");
        let output_path = PathBuf::from("custom.tgz");
        let (output, format) =
            resolve_output_and_format(&env, Some(output_path.clone()), PackFormat::Infer).unwrap();
        assert_eq!(format, ArchiveFormat::TarGz);
        assert_eq!(output, output_path);
    }

    #[test]
    fn temp_dir_for_output_defaults_to_current_directory() {
        let cwd = env::current_dir().unwrap();
        let output = PathBuf::from("archive.tar.gz");
        let tmp_dir = temp_dir_for_output(&output).unwrap();
        assert_eq!(tmp_dir, cwd);
    }

    #[test]
    fn temp_dir_for_output_uses_parent_directory_when_available() {
        let output = PathBuf::from("dist/archive.tar.gz");
        let tmp_dir = temp_dir_for_output(&output).unwrap();
        assert_eq!(tmp_dir, PathBuf::from("dist"));
    }

    #[test]
    fn infer_format_from_output_handles_unknown_extension() {
        let err = infer_format_from_output(PathBuf::from("archive.bin")).unwrap_err();
        assert!(format!("{err}").contains("Unknown file extension"));
    }

    #[test]
    fn is_in_bin_dir_detects_first_component() {
        let path = Path::new(BIN_DIR).join("python");
        assert!(is_in_bin_dir(&path));

        let other = Path::new("lib").join(BIN_DIR).join("python");
        assert!(!is_in_bin_dir(&other));
    }

    #[test]
    fn rewrite_shebang_rewrites_matching_prefix() {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join("venv");
        fs::create_dir_all(&prefix).unwrap();
        let shebang = format!("#!{}/bin/python -O\n", prefix.display());
        let (rewritten, changed) = rewrite_shebang(shebang.as_bytes(), &prefix);
        assert!(changed);
        let expected = format!("#!/usr/bin/env python -O\n");
        assert_eq!(String::from_utf8(rewritten).unwrap(), expected);
    }

    #[test]
    fn rewrite_shebang_ignores_non_matching_prefix() {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join("venv");
        fs::create_dir_all(&prefix).unwrap();
        let shebang = "#!/opt/python/bin/python\n";
        let (rewritten, changed) = rewrite_shebang(shebang.as_bytes(), &prefix);
        assert!(!changed);
        assert_eq!(String::from_utf8(rewritten).unwrap(), shebang);
    }

    #[test]
    fn rewrite_shebang_with_multiple_prefix_matches_is_ignored() {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join("venv");
        fs::create_dir_all(&prefix).unwrap();
        let repeated = format!("#!{} {}/bin/python\n", prefix.display(), prefix.display());
        let (_, changed) = rewrite_shebang(repeated.as_bytes(), &prefix);
        assert!(!changed);
    }

    #[test]
    fn check_python_prefix_requires_absolute_paths() {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join("venv");
        fs::create_dir_all(&prefix).unwrap();
        let context = dummy_context(&prefix, EnvKind::Venv);
        let err = check_python_prefix(Some(PathBuf::from("relative")), &context).unwrap_err();
        assert!(format!("{err}").contains("must be an absolute path"));
    }

    #[test]
    fn check_python_prefix_rewrites_for_venv() {
        let tmp = tempdir().unwrap();
        let original = tmp.path().join("orig");
        let prefix = tmp.path().join("venv");
        fs::create_dir_all(&prefix).unwrap();
        fs::create_dir_all(&original).unwrap();

        let mut context = dummy_context(&prefix, EnvKind::Venv);
        context.orig_prefix = original.clone();

        let new_prefix = tmp.path().join("newprefix");
        let (actual_prefix, rewrites) =
            check_python_prefix(Some(new_prefix.clone()), &context).unwrap();

        assert_eq!(actual_prefix.unwrap(), new_prefix);
        assert_eq!(rewrites.len(), 1);

        let (old, new) = &rewrites[0];
        let expected_old = original
            .join(BIN_DIR)
            .join(context.py_lib.file_name().unwrap());
        let expected_new = new_prefix
            .join(BIN_DIR)
            .join(context.py_lib.file_name().unwrap());
        assert_eq!(old, &path_to_string(&expected_old).into_owned());
        assert_eq!(new, &path_to_string(&expected_new).into_owned());
    }

    #[test]
    fn check_python_prefix_rewrites_for_virtualenv() {
        let tmp = tempdir().unwrap();
        let original = tmp.path().join("orig");
        let prefix = tmp.path().join("env");
        fs::create_dir_all(&prefix).unwrap();
        fs::create_dir_all(&original).unwrap();

        let mut context = dummy_context(&prefix, EnvKind::Virtualenv);
        context.orig_prefix = original.clone();

        let new_prefix = tmp.path().join("newenv");
        let (actual_prefix, rewrites) =
            check_python_prefix(Some(new_prefix.clone()), &context).unwrap();

        assert_eq!(actual_prefix.unwrap(), new_prefix);
        assert_eq!(rewrites.len(), 2);

        let expected_lib_old = ensure_trailing_sep(&original.join(&context.py_lib));
        let expected_lib_new = ensure_trailing_sep(&new_prefix.join(&context.py_lib));
        let expected_inc_old = path_to_string(&original.join(&context.py_include)).into_owned();
        let expected_inc_new = path_to_string(&new_prefix.join(&context.py_include)).into_owned();

        assert!(rewrites.contains(&(expected_lib_old.clone(), expected_lib_new.clone())));
        assert!(rewrites.contains(&(expected_inc_old.clone(), expected_inc_new.clone())));
    }

    #[test]
    fn check_no_editable_packages_can_be_skipped() {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join("venv");
        let site_packages = prefix.join("lib/python3.11").join("site-packages");
        fs::create_dir_all(&site_packages).unwrap();
        let editable = site_packages.join("editable.pth");
        fs::write(&editable, "/external/package\n").unwrap();

        let context = dummy_context(&prefix, EnvKind::Venv);

        let err = check_no_editable_packages(&context, false).unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("Editable packages found"));
        assert!(message.contains("/external/package"));

        assert!(check_no_editable_packages(&context, true).is_ok());
    }
}
