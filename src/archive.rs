use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path};

use flate2::write::GzEncoder;
use flate2::Compression as GzCompression;
use zip::write::FileOptions;
use zip::CompressionMethod;

use crate::error::{CrabpackError, Result};

/// Directory that contains executables inside the virtual environment.
pub const BIN_DIR: &str = if cfg!(windows) { "Scripts" } else { "bin" };

/// Supported archive formats once the output target has been inferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveFormat {
    Tar,
    TarGz,
    TarBz2,
    Zip,
}

/// Options that control how archives are written.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveOptions {
    pub compress_level: u32,
    pub zip_symlinks: bool,
    pub zip_64: bool,
}

pub struct Archive {
    inner: ArchiveInner,
}

enum ArchiveInner {
    Tar(tar::Builder<TarWriter>),
    Zip(ZipArchive),
}

impl Archive {
    pub fn new(file: File, format: ArchiveFormat, options: ArchiveOptions) -> Result<Self> {
        let inner = match format {
            ArchiveFormat::Zip => {
                ArchiveInner::Zip(ZipArchive::new(file, options.zip_symlinks, options.zip_64)?)
            }
            _ => {
                let writer = TarWriter::new(file, format, options.compress_level)?;
                let mut builder = tar::Builder::new(writer);
                builder.follow_symlinks(false);
                ArchiveInner::Tar(builder)
            }
        };

        Ok(Archive { inner })
    }

    pub fn add(&mut self, source: &Path, target: &Path) -> Result<()> {
        match &mut self.inner {
            ArchiveInner::Tar(builder) => {
                builder.append_path_with_name(source, target)?;
                Ok(())
            }
            ArchiveInner::Zip(zip) => zip.add(source, target),
        }
    }

    pub fn add_bytes(&mut self, source: &Path, data: &[u8], target: &Path) -> Result<()> {
        match &mut self.inner {
            ArchiveInner::Tar(builder) => {
                let metadata = fs::metadata(source)?;
                let mut header = tar::Header::new_gnu();
                header.set_metadata(&metadata);
                header.set_size(data.len() as u64);
                header.set_cksum();
                builder.append_data(&mut header, target, io::Cursor::new(data))?;
                Ok(())
            }
            ArchiveInner::Zip(zip) => zip.add_bytes(source, data, target),
        }
    }

    pub fn add_bytes_with_mode(
        &mut self,
        data: &[u8],
        target: &Path,
        mode: Option<u32>,
    ) -> Result<()> {
        match &mut self.inner {
            ArchiveInner::Tar(builder) => {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_entry_type(tar::EntryType::Regular);
                let perm = mode.unwrap_or(0o644) & 0o7777;
                header.set_mode(perm);
                header.set_uid(0);
                header.set_gid(0);
                header.set_mtime(current_timestamp());
                header.set_cksum();
                builder.append_data(&mut header, target, io::Cursor::new(data))?;
                Ok(())
            }
            ArchiveInner::Zip(zip) => zip.add_bytes_with_mode(data, target, mode),
        }
    }

    pub fn add_link(&mut self, source: &Path, link_target: &Path, target: &Path) -> Result<()> {
        match &mut self.inner {
            ArchiveInner::Tar(builder) => {
                let metadata = fs::symlink_metadata(source)?;
                let mut header = tar::Header::new_gnu();
                header.set_metadata(&metadata);
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_link_name(link_target)?;
                header.set_cksum();
                builder.append_data(&mut header, target, io::empty())?;
                Ok(())
            }
            ArchiveInner::Zip(zip) => zip.add_link(source, link_target, target),
        }
    }

    pub fn finish(self) -> Result<()> {
        match self.inner {
            ArchiveInner::Tar(builder) => {
                let writer = builder.into_inner()?;
                writer.finish()?;
                Ok(())
            }
            ArchiveInner::Zip(zip) => {
                zip.finish()?;
                Ok(())
            }
        }
    }
}

struct ZipArchive {
    writer: zip::ZipWriter<File>,
    zip_symlinks: bool,
    zip_64: bool,
}

impl ZipArchive {
    fn new(file: File, zip_symlinks: bool, zip_64: bool) -> Result<Self> {
        let writer = zip::ZipWriter::new(file);
        Ok(ZipArchive {
            writer,
            zip_symlinks,
            zip_64,
        })
    }

    fn add(&mut self, source: &Path, target: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(source)?;
        if metadata.file_type().is_symlink() {
            if self.zip_symlinks {
                let link_target = fs::read_link(source)?;
                self.add_link_with_metadata(source, &link_target, target, &metadata)
            } else if metadata.is_dir() || fs::metadata(source).map(|m| m.is_dir()).unwrap_or(false)
            {
                self.copy_directory_following_links(source, target)
            } else {
                self.add_file_with_metadata(source, target, fs::metadata(source)?)
            }
        } else if metadata.is_dir() {
            self.add_directory_entry(target, &metadata)
        } else {
            self.add_file_with_metadata(source, target, fs::metadata(source)?)
        }
    }

    fn add_bytes(&mut self, source: &Path, data: &[u8], target: &Path) -> Result<()> {
        let metadata = fs::metadata(source)?;
        let mut options = self.file_options(&metadata, false);
        options = options.large_file(self.zip_64);
        let name = zip_name(target)?;
        self.writer.start_file(name, options)?;
        self.writer.write_all(data)?;
        Ok(())
    }

    fn add_bytes_with_mode(&mut self, data: &[u8], target: &Path, mode: Option<u32>) -> Result<()> {
        let mut options = FileOptions::default().compression_method(CompressionMethod::Deflated);
        if let Some(mode) = mode {
            options = options.unix_permissions(mode);
        }
        options = options.large_file(self.zip_64);
        let name = zip_name(target)?;
        self.writer.start_file(name, options)?;
        self.writer.write_all(data)?;
        Ok(())
    }

    fn add_link(&mut self, source: &Path, link_target: &Path, target: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(source)?;
        self.add_link_with_metadata(source, link_target, target, &metadata)
    }

    fn add_link_with_metadata(
        &mut self,
        source: &Path,
        link_target: &Path,
        target: &Path,
        metadata: &fs::Metadata,
    ) -> Result<()> {
        if !self.zip_symlinks {
            return self.add_file_with_metadata(source, target, fs::metadata(source)?);
        }

        let mut options = self.file_options(metadata, false);
        options = options.large_file(self.zip_64);
        options = options.unix_permissions(0o120777);
        let name = zip_name(target)?;
        self.writer.start_file(name, options)?;
        let data = link_target.as_os_str().to_string_lossy();
        self.writer.write_all(data.as_bytes())?;
        Ok(())
    }

    fn add_file_with_metadata(
        &mut self,
        source: &Path,
        target: &Path,
        metadata: fs::Metadata,
    ) -> Result<()> {
        let name = zip_name(target)?;
        let mut options = self.file_options(&metadata, false);
        options = options.large_file(self.zip_64);
        self.writer.start_file(name, options)?;
        let mut file = File::open(source)?;
        io::copy(&mut file, &mut self.writer)?;
        Ok(())
    }

    fn add_directory_entry(&mut self, target: &Path, metadata: &fs::Metadata) -> Result<()> {
        let mut name = zip_name(target)?;
        if !name.ends_with('/') {
            name.push('/');
        }
        let mut options = self.file_options(metadata, true);
        options = options.large_file(self.zip_64);
        self.writer.add_directory(name, options)?;
        Ok(())
    }

    fn copy_directory_following_links(&mut self, source: &Path, target: &Path) -> Result<()> {
        let entries: Vec<_> = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        if entries.is_empty() {
            let metadata = fs::metadata(source)?;
            self.add_directory_entry(target, &metadata)?;
            return Ok(());
        }

        for entry in entries {
            let src_path = entry.path();
            let dst_path = target.join(entry.file_name());
            let metadata = fs::metadata(&src_path)?;
            if metadata.is_dir() {
                self.add_directory_entry(&dst_path, &metadata)?;
                self.copy_directory_following_links(&src_path, &dst_path)?;
            } else {
                self.add_file_with_metadata(&src_path, &dst_path, metadata)?;
            }
        }

        Ok(())
    }

    fn file_options(&self, metadata: &fs::Metadata, is_dir: bool) -> FileOptions {
        let mut options = FileOptions::default().compression_method(CompressionMethod::Deflated);
        if let Some(perm) = unix_mode(metadata, is_dir) {
            options = options.unix_permissions(perm);
        }
        options
    }

    fn finish(mut self) -> Result<File> {
        Ok(self.writer.finish()?)
    }
}

fn zip_name(path: &Path) -> Result<String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => components.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => components.push("..".to_string()),
            _ => {
                return Err(CrabpackError::user(format!(
                    "unsupported path component in archive target: {:?}",
                    path
                )))
            }
        }
    }

    Ok(components.join("/"))
}

enum TarWriter {
    Plain(File),
    Gzip(GzEncoder<File>),
    Bzip2(bzip2::write::BzEncoder<File>),
}

impl TarWriter {
    fn new(file: File, format: ArchiveFormat, level: u32) -> Result<Self> {
        let level = level.min(9);
        Ok(match format {
            ArchiveFormat::Tar => TarWriter::Plain(file),
            ArchiveFormat::TarGz => {
                TarWriter::Gzip(GzEncoder::new(file, GzCompression::new(level)))
            }
            ArchiveFormat::TarBz2 => TarWriter::Bzip2(bzip2::write::BzEncoder::new(
                file,
                bzip2::Compression::new(level),
            )),
            ArchiveFormat::Zip => unreachable!(),
        })
    }

    fn finish(self) -> io::Result<File> {
        match self {
            TarWriter::Plain(file) => Ok(file),
            TarWriter::Gzip(writer) => writer.finish(),
            TarWriter::Bzip2(writer) => writer.finish(),
        }
    }
}

impl Write for TarWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            TarWriter::Plain(file) => file.write(buf),
            TarWriter::Gzip(writer) => writer.write(buf),
            TarWriter::Bzip2(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            TarWriter::Plain(file) => file.flush(),
            TarWriter::Gzip(writer) => writer.flush(),
            TarWriter::Bzip2(writer) => writer.flush(),
        }
    }
}

fn unix_mode(metadata: &fs::Metadata, is_dir: bool) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if is_dir {
            Some(mode | 0o040000)
        } else {
            Some(mode)
        }
    }

    #[cfg(not(unix))]
    {
        let mode = if is_dir { 0o40755 } else { 0o100644 };
        Some(mode)
    }
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
