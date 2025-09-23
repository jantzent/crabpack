use std::path::PathBuf;
use std::process;

use clap::{value_parser, Arg, ArgAction, ArgMatches, Command};

use crabpack::error::CrabpackError;
use crabpack::{pack, FilterKind, PackFilter, PackFormat, PackOptions};

fn main() {
    if let Err(err) = run() {
        eprintln!("Crabpack error: {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), CrabpackError> {
    let matches = build_cli().get_matches();

    if matches.get_flag("version") {
        println!("crabpack {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let mut options = PackOptions::default();
    options.prefix = matches
        .get_one::<String>("prefix")
        .map(|p| PathBuf::from(p));
    options.output = matches.get_one::<String>("output").map(PathBuf::from);
    options.format = PackFormat::parse(matches.get_one::<String>("format").unwrap())?;
    options.python_prefix = matches
        .get_one::<String>("python-prefix")
        .map(PathBuf::from);
    options.verbose = !matches.get_flag("quiet");
    options.force = matches.get_flag("force");
    options.compress_level = *matches.get_one::<u32>("compress-level").unwrap();
    options.zip_symlinks = matches.get_flag("zip-symlinks");
    options.zip_64 = !matches.get_flag("no-zip-64");
    options.filters = parse_filters(&matches);

    pack(options)?;
    Ok(())
}

fn build_cli() -> Command {
    Command::new("crabpack")
        .about("Package an existing virtual environment into an archive file.")
        .arg(
            Arg::new("prefix")
                .short('p')
                .long("prefix")
                .value_name("PATH")
                .help("Full path to environment prefix. Default is current environment."),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("PATH")
                .help("The path of the output file. Defaults to the environment name with a .tar.gz suffix."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .default_value("infer")
                .value_parser(["infer", "zip", "tar.gz", "tgz", "tar.bz2", "tbz2", "tar"])
                .help("The archival format to use. By default this is inferred by the output file extension."),
        )
        .arg(
            Arg::new("python-prefix")
                .long("python-prefix")
                .value_name("PATH")
                .help("New prefix path for linking python in the packaged environment."),
        )
        .arg(
            Arg::new("compress-level")
                .long("compress-level")
                .value_parser(value_parser!(u32).range(0..=9))
                .default_value("4")
                .help("Compression level to use (0-9). Ignored for zip archives."),
        )
        .arg(
            Arg::new("zip-symlinks")
                .long("zip-symlinks")
                .action(ArgAction::SetTrue)
                .help("Store symbolic links in the zip archive instead of the linked files."),
        )
        .arg(
            Arg::new("no-zip-64")
                .long("no-zip-64")
                .action(ArgAction::SetTrue)
                .help("Disable ZIP64 extensions."),
        )
        .arg(
            Arg::new("exclude")
                .long("exclude")
                .value_name("PATTERN")
                .action(ArgAction::Append)
                .help("Exclude files matching this pattern."),
        )
        .arg(
            Arg::new("include")
                .long("include")
                .value_name("PATTERN")
                .action(ArgAction::Append)
                .help("Re-add excluded files matching this pattern."),
        )
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(ArgAction::SetTrue)
                .help("Overwrite any existing archive at the output path."),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .action(ArgAction::SetTrue)
                .help("Do not report progress."),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .action(ArgAction::SetTrue)
                .help("Show version then exit."),
        )
}

fn parse_filters(matches: &ArgMatches) -> Vec<PackFilter> {
    let mut filters = Vec::new();

    if let Some(values) = matches.get_many::<String>("exclude") {
        if let Some(indices) = matches.indices_of("exclude") {
            let vals: Vec<String> = values.map(|v| v.to_string()).collect();
            for (idx, value) in indices.zip(vals.into_iter()) {
                filters.push((idx, FilterKind::Exclude, value));
            }
        }
    }

    if let Some(values) = matches.get_many::<String>("include") {
        if let Some(indices) = matches.indices_of("include") {
            let vals: Vec<String> = values.map(|v| v.to_string()).collect();
            for (idx, value) in indices.zip(vals.into_iter()) {
                filters.push((idx, FilterKind::Include, value));
            }
        }
    }

    filters.sort_by_key(|(idx, _, _)| *idx);
    filters
        .into_iter()
        .map(|(_, kind, pattern)| PackFilter { kind, pattern })
        .collect()
}
