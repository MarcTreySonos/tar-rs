use std::borrow::Cow;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::path::Path;

use crate::header::{bytes2path, path2bytes, HeaderMode};
use crate::{other, EntryType, Header};

/// A structure for building archives
///
/// This structure has methods for building up an archive from scratch into any
/// arbitrary writer.
pub struct Builder<W: Write> {
    mode: HeaderMode,
    follow: bool,
    finished: bool,
    obj: Option<W>,
}

impl<W: Write> Builder<W> {
    /// Create a new archive builder with the underlying object as the
    /// destination of all data written. The builder will use
    /// `HeaderMode::Complete` by default.
    pub fn new(obj: W) -> Builder<W> {
        Builder {
            mode: HeaderMode::Complete,
            follow: true,
            finished: false,
            obj: Some(obj),
        }
    }

    /// Changes the HeaderMode that will be used when reading fs Metadata for
    /// methods that implicitly read metadata for an input Path. Notably, this
    /// does _not_ apply to `append(Header)`.
    pub fn mode(&mut self, mode: HeaderMode) {
        self.mode = mode;
    }

    /// Follow symlinks, archiving the contents of the file they point to rather
    /// than adding a symlink to the archive. Defaults to true.
    pub fn follow_symlinks(&mut self, follow: bool) {
        self.follow = follow;
    }

    /// Gets shared reference to the underlying object.
    pub fn get_ref(&self) -> &W {
        self.obj.as_ref().unwrap()
    }

    /// Gets mutable reference to the underlying object.
    ///
    /// Note that care must be taken while writing to the underlying
    /// object. But, e.g. `get_mut().flush()` is claimed to be safe and
    /// useful in the situations when one needs to be ensured that
    /// tar entry was flushed to the disk.
    pub fn get_mut(&mut self) -> &mut W {
        self.obj.as_mut().unwrap()
    }

    /// Unwrap this archive, returning the underlying object.
    ///
    /// This function will finish writing the archive if the `finish` function
    /// hasn't yet been called, returning any I/O error which happens during
    /// that operation.
    pub fn into_inner(mut self) -> io::Result<W> {
        if !self.finished {
            self.finish()?;
        }
        Ok(self.obj.take().unwrap())
    }

    /// Adds a new entry to this archive.
    ///
    /// This function will append the header specified, followed by contents of
    /// the stream specified by `data`. To produce a valid archive the `size`
    /// field of `header` must be the same as the length of the stream that's
    /// being written. Additionally the checksum for the header should have been
    /// set via the `set_cksum` method.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all entries have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Errors
    ///
    /// This function will return an error for any intermittent I/O error which
    /// occurs when either reading or writing.
    ///
    /// # Examples
    ///
    /// ```
    /// use tar::{Builder, Header};
    ///
    /// let mut header = Header::new_gnu();
    /// header.set_path("foo").unwrap();
    /// header.set_size(4);
    /// header.set_cksum();
    ///
    /// let mut data: &[u8] = &[1, 2, 3, 4];
    ///
    /// let mut ar = Builder::new(Vec::new());
    /// ar.append(&header, data).unwrap();
    /// let data = ar.into_inner().unwrap();
    /// ```
    pub fn append<R: Read>(&mut self, header: &Header, mut data: R) -> io::Result<()> {
        append(self.get_mut(), header, &mut data)
    }

    /// Adds a new entry to this archive with the specified path.
    ///
    /// This function will set the specified path in the given header, which may
    /// require appending a GNU long-name extension entry to the archive first.
    /// The checksum for the header will be automatically updated via the
    /// `set_cksum` method after setting the path. No other metadata in the
    /// header will be modified.
    ///
    /// Then it will append the header, followed by contents of the stream
    /// specified by `data`. To produce a valid archive the `size` field of
    /// `header` must be the same as the length of the stream that's being
    /// written.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all entries have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Errors
    ///
    /// This function will return an error for any intermittent I/O error which
    /// occurs when either reading or writing.
    ///
    /// # Examples
    ///
    /// ```
    /// use tar::{Builder, Header};
    ///
    /// let mut header = Header::new_gnu();
    /// header.set_size(4);
    /// header.set_cksum();
    ///
    /// let mut data: &[u8] = &[1, 2, 3, 4];
    ///
    /// let mut ar = Builder::new(Vec::new());
    /// ar.append_data(&mut header, "really/long/path/to/foo", data).unwrap();
    /// let data = ar.into_inner().unwrap();
    /// ```
    pub fn append_data<P: AsRef<Path>, R: Read>(
        &mut self,
        header: &mut Header,
        path: P,
        data: R,
    ) -> io::Result<()> {
        prepare_header_path(self.get_mut(), header, path.as_ref())?;
        header.set_cksum();
        self.append(&header, data)
    }

    /// Adds a file on the local filesystem to this archive.
    ///
    /// This function will open the file specified by `path` and insert the file
    /// into the archive with the appropriate metadata set, returning any I/O
    /// error which occurs while writing. The path name for the file inside of
    /// this archive will be the same as `path`, and it is required that the
    /// path is a relative path.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// ar.append_path("foo/bar.txt").unwrap();
    /// ```
    pub fn append_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let mode = self.mode.clone();
        let follow = self.follow;
        append_path_with_name(self.get_mut(), path.as_ref(), None, mode, follow)
    }

    /// Adds a file on the local filesystem to this archive under another name.
    ///
    /// This function will open the file specified by `path` and insert the file
    /// into the archive as `name` with appropriate metadata set, returning any
    /// I/O error which occurs while writing. The path name for the file inside
    /// of this archive will be `name` is required to be a relative path.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Note if the `path` is a directory. This will just add an entry to the archive,
    /// rather than contents of the directory.  
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Insert the local file "foo/bar.txt" in the archive but with the name
    /// // "bar/foo.txt".
    /// ar.append_path_with_name("foo/bar.txt", "bar/foo.txt").unwrap();
    /// ```
    pub fn append_path_with_name<P: AsRef<Path>, N: AsRef<Path>>(
        &mut self,
        path: P,
        name: N,
    ) -> io::Result<()> {
        let mode = self.mode.clone();
        let follow = self.follow;
        append_path_with_name(
            self.get_mut(),
            path.as_ref(),
            Some(name.as_ref()),
            mode,
            follow,
        )
    }

    /// Adds a file to this archive with the given path as the name of the file
    /// in the archive.
    ///
    /// This will use the metadata of `file` to populate a `Header`, and it will
    /// then append the file to the archive with the name `path`.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs::File;
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Open the file at one location, but insert it into the archive with a
    /// // different name.
    /// let mut f = File::open("foo/bar/baz.txt").unwrap();
    /// ar.append_file("bar/baz.txt", &mut f).unwrap();
    /// ```
    pub fn append_file<P: AsRef<Path>>(&mut self, path: P, file: &mut fs::File) -> io::Result<()> {
        let mode = self.mode.clone();
        append_file(self.get_mut(), path.as_ref(), file, mode)
    }

    /// Adds a directory to this archive with the given path as the name of the
    /// directory in the archive.
    ///
    /// This will use `stat` to populate a `Header`, and it will then append the
    /// directory to the archive with the name `path`.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Note this will not add the contents of the directory to the archive.
    /// See `append_dir_all` for recusively adding the contents of the directory.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::fs;
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Use the directory at one location, but insert it into the archive
    /// // with a different name.
    /// ar.append_dir("bardir", ".").unwrap();
    /// ```
    pub fn append_dir<P, Q>(&mut self, path: P, src_path: Q) -> io::Result<()>
    where
        P: AsRef<Path>,
        Q: AsRef<Path>,
    {
        let mode = self.mode.clone();
        append_dir(self.get_mut(), path.as_ref(), src_path.as_ref(), mode)
    }

    /// Adds a directory and all of its contents (recursively) to this archive
    /// with the given path as the name of the directory in the archive.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::fs;
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Use the directory at one location, but insert it into the archive
    /// // with a different name.
    /// ar.append_dir_all("bardir", ".").unwrap();
    /// ```
    pub fn append_dir_all<P, Q>(&mut self, path: P, src_path: Q) -> io::Result<()>
    where
        P: AsRef<Path>,
        Q: AsRef<Path>,
    {
        let mode = self.mode.clone();
        let follow = self.follow;
        append_dir_all(
            self.get_mut(),
            path.as_ref(),
            src_path.as_ref(),
            mode,
            follow,
        )
    }

    /// Finish writing this archive, emitting the termination sections.
    ///
    /// This function should only be called when the archive has been written
    /// entirely and if an I/O error happens the underlying object still needs
    /// to be acquired.
    ///
    /// In most situations the `into_inner` method should be preferred.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.get_mut().write_all(&[0; 1024])
    }
}

fn append(mut dst: &mut dyn Write, header: &Header, mut data: &mut dyn Read) -> io::Result<()> {
    dst.write_all(header.as_bytes())?;
    let len = io::copy(&mut data, &mut dst)?;

    // Pad with zeros if necessary.
    let buf = [0; 512];
    let remaining = 512 - (len % 512);
    if remaining < 512 {
        dst.write_all(&buf[..remaining as usize])?;
    }

    Ok(())
}

fn append_path_with_name(
    dst: &mut dyn Write,
    path: &Path,
    name: Option<&Path>,
    mode: HeaderMode,
    follow: bool,
) -> io::Result<()> {
    let stat = if follow {
        fs::metadata(path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("{} when getting metadata for {}", err, path.display()),
            )
        })?
    } else {
        fs::symlink_metadata(path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("{} when getting metadata for {}", err, path.display()),
            )
        })?
    };
    let ar_name = name.unwrap_or(path);
    if stat.is_file() {
        append_fs(dst, ar_name, &stat, &mut fs::File::open(path)?, mode, None)
    } else if stat.is_dir() {
        append_fs(dst, ar_name, &stat, &mut io::empty(), mode, None)
    } else if stat.file_type().is_symlink() {
        let link_name = fs::read_link(path)?;
        append_fs(
            dst,
            ar_name,
            &stat,
            &mut io::empty(),
            mode,
            Some(&link_name),
        )
    } else {
        Err(other(&format!("{} has unknown file type", path.display())))
    }
}

fn append_file(
    dst: &mut dyn Write,
    path: &Path,
    file: &mut fs::File,
    mode: HeaderMode,
) -> io::Result<()> {
    let stat = file.metadata()?;
    append_fs(dst, path, &stat, file, mode, None)
}

fn append_dir(
    dst: &mut dyn Write,
    path: &Path,
    src_path: &Path,
    mode: HeaderMode,
) -> io::Result<()> {
    let stat = fs::metadata(src_path)?;
    append_fs(dst, path, &stat, &mut io::empty(), mode, None)
}

fn prepare_header(size: u64, entry_type: u8) -> Header {
    let mut header = Header::new_gnu();
    let name = b"././@LongLink";
    header.as_gnu_mut().unwrap().name[..name.len()].clone_from_slice(&name[..]);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    // + 1 to be compliant with GNU tar
    header.set_size(size + 1);
    header.set_entry_type(EntryType::new(entry_type));
    header.set_cksum();
    header
}

fn prepare_header_path(dst: &mut dyn Write, header: &mut Header, path: &Path) -> io::Result<()> {
    // Try to encode the path directly in the header, but if it ends up not
    // working (probably because it's too long) then try to use the GNU-specific
    // long name extension by emitting an entry which indicates that it's the
    // filename.
    if let Err(e) = header.set_path(path) {
        let data = path2bytes(&path)?;
        let max = header.as_old().name.len();
        //  Since e isn't specific enough to let us know the path is indeed too
        //  long, verify it first before using the extension.
        if data.len() < max {
            return Err(e);
        }
        let header2 = prepare_header(data.len() as u64, b'L');
        // null-terminated string
        let mut data2 = data.chain(io::repeat(0).take(1));
        append(dst, &header2, &mut data2)?;
        // Truncate the path to store in the header we're about to emit to
        // ensure we've got something at least mentioned.
        let path = bytes2path(Cow::Borrowed(&data[..max]))?;
        header.set_path(&path)?;
    }
    Ok(())
}

fn prepare_header_link(
    dst: &mut dyn Write,
    header: &mut Header,
    link_name: &Path,
) -> io::Result<()> {
    // Same as previous function but for linkname
    if let Err(e) = header.set_link_name(&link_name) {
        let data = path2bytes(&link_name)?;
        if data.len() < header.as_old().linkname.len() {
            return Err(e);
        }
        let header2 = prepare_header(data.len() as u64, b'K');
        let mut data2 = data.chain(io::repeat(0).take(1));
        append(dst, &header2, &mut data2)?;
    }
    Ok(())
}

fn append_fs(
    dst: &mut dyn Write,
    path: &Path,
    meta: &fs::Metadata,
    read: &mut dyn Read,
    mode: HeaderMode,
    link_name: Option<&Path>,
) -> io::Result<()> {
    let mut header = Header::new_gnu();

    prepare_header_path(dst, &mut header, path)?;
    header.set_metadata_in_mode(meta, mode);
    if let Some(link_name) = link_name {
        prepare_header_link(dst, &mut header, link_name)?;
    }
    header.set_cksum();
    append(dst, &header, read)
}

fn append_dir_all(
    dst: &mut dyn Write,
    path: &Path,
    src_path: &Path,
    mode: HeaderMode,
    follow: bool,
) -> io::Result<()> {
    let mut stack = vec![(src_path.to_path_buf(), true, false)];
    while let Some((src, is_dir, is_symlink)) = stack.pop() {
        let dest = path.join(src.strip_prefix(&src_path).unwrap());
        // In case of a symlink pointing to a directory, is_dir is false, but src.is_dir() will return true
        if is_dir || (is_symlink && follow && src.is_dir()) {
            for entry in fs::read_dir(&src)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                stack.push((entry.path(), file_type.is_dir(), file_type.is_symlink()));
            }
            if dest != Path::new("") {
                append_dir(dst, &dest, &src, mode)?;
            }
        } else if !follow && is_symlink {
            let stat = fs::symlink_metadata(&src)?;
            let link_name = fs::read_link(&src)?;
            append_fs(dst, &dest, &stat, &mut io::empty(), mode, Some(&link_name))?;
        } else {
            append_file(dst, &dest, &mut fs::File::open(src)?, mode)?;
        }
    }
    Ok(())
}

impl<W: Write> Drop for Builder<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

// Hello there, person ! Welcome to my weird hack that I programmed to transform the builder
// into a Read-able struct
//
// It's holding together and should work but it needs major refactoring because I am so not proud
// of this mess.
//
// TODO:
//
// - PaddedReader (for the 512 bytes padding of the data)
// - ChainedReader (to chain the readers of the EntryReader)
// - Refactor TarReader mess
// - Change the stack content into a clear enum (Dir, DirAll, File, Link, ...)
// - Fix symlinks
// - Rename append_file to append_path_with_name
// - Use the builder's test suite on this BuilderReader.

use std::path::PathBuf;

/// TODO DOC
pub struct BuilderReader {
    stack: Vec<(PathBuf, PathBuf, PathBuf, bool, bool)>,
}

impl BuilderReader {
    /// TODO DOC
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// TODO DOC
    pub fn append_dir_all<P, Q>(&mut self, path: P, src_path: Q) -> io::Result<()>
    where
        P: AsRef<Path>,
        Q: AsRef<Path>,
    {
        self.stack.push((
            path.as_ref().to_owned(),
            src_path.as_ref().to_owned(),
            src_path.as_ref().to_owned(),
            true,
            false,
        ));
        Ok(())
    }

    /// TODO DOC
    pub fn append_file<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        path: P,
        src_path: Q,
    ) -> io::Result<()> {
        self.stack.push((
            path.as_ref().to_owned(),
            src_path.as_ref().to_owned(),
            src_path.as_ref().to_owned(),
            false,
            false,
        ));
        Ok(())
    }

    /// TODO DOC
    pub fn into_reader(self) -> TarReader {
        TarReader {
            stack: self.stack,
            mode: HeaderMode::Complete,
            follow: true,
            finished: false,
            current_reader: None,
        }
    }
}

use std::io::Read;

/// TODO DOC
pub struct TarReader {
    stack: Vec<(PathBuf, PathBuf, PathBuf, bool, bool)>,
    mode: HeaderMode,
    follow: bool,
    finished: bool,
    current_reader: Option<Box<dyn Read + Send>>,
}

impl TarReader {
    /*fn pop_stack(&mut self) -> io::Result<Box<dyn Read>> {
        unimplemented!();
    }*/

    /*    fn put_reader_stack(&mut self, reader: Box<dyn Read>) -> io::Result<()> {
        Ok(())
    }*/
}

impl Read for TarReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut total_read_amount = 0;
        let mut to_read = buf;
        'outerloop: loop {
            //while let Some((src, is_dir, is_symlink)) = stack.pop() {
            if let Some(mut reader) = self.current_reader.take() {
                loop {
                    let read_amount = reader.read(to_read)?;
                    total_read_amount += read_amount;
                    if read_amount == 0 {
                        break;
                    }
                    to_read = &mut to_read[read_amount..];
                    if to_read.len() == 0 {
                        self.current_reader = Some(reader);
                        break 'outerloop;
                    }
                }
            }
            let next_reader = if let Some((path, src_path, src, is_dir, is_symlink)) =
                self.stack.pop()
            {
                let dest = path.join(src.strip_prefix(&src_path).unwrap());
                // In case of a symlink pointing to a directory, is_dir is false, but src.is_dir() will return true
                if is_dir || (is_symlink && self.follow && src.is_dir()) {
                    for entry in fs::read_dir(&src)? {
                        let entry = entry?;
                        let file_type = entry.file_type()?;

                        self.stack.push((
                            path.clone(),
                            src_path.clone(),
                            entry.path(),
                            file_type.is_dir(),
                            file_type.is_symlink(),
                        ));
                    }
                    if &dest != Path::new("") {
                        let reader = append_dir_read(&dest, &src, self.mode)?;
                        Some(Box::new(reader) as Box<dyn Read + Send>)
                    } else {
                        Some(Box::new(std::io::empty()) as Box<dyn Read + Send>)
                    }
                }
                //TODO: symlinks
                /*else if !follow && is_symlink {
                    let stat = fs::symlink_metadata(&src)?;
                    let link_name = fs::read_link(&src)?;
                    append_fs(dst, &dest, &stat, &mut io::empty(), mode, Some(&link_name))?;
                }*/
                else {
                    let dest = if src.strip_prefix(&src_path).unwrap() == Path::new("") {
                        &path
                    } else {
                        &dest
                    };
                    let reader = append_file_read(&dest, fs::File::open(&src)?, self.mode)?;
                    Some(Box::new(reader) as Box<dyn Read + Send>)
                }
            } else {
                if !self.finished {
                    self.finished = true;
                    Some(Box::new(std::io::Cursor::new(vec![0u8; 1024])) as Box<dyn Read + Send>)
                } else {
                    None
                }
            };
            if next_reader.is_none() {
                break 'outerloop;
            } else {
                self.current_reader = next_reader;
            }
        }
        Ok(total_read_amount)
    }
}

fn append_dir_read(path: &Path, src_path: &Path, mode: HeaderMode) -> io::Result<EntryReader> {
    let stat = fs::metadata(src_path)?;
    append_fs_read(path, &stat, Box::new(io::empty()), mode, None)
}

fn append_file_read(path: &Path, file: fs::File, mode: HeaderMode) -> io::Result<EntryReader> {
    let stat = file.metadata()?;
    append_fs_read(path, &stat, Box::new(file), mode, None)
}

fn append_fs_read(
    path: &Path,
    meta: &fs::Metadata,
    read: Box<dyn Read + Send>,
    mode: HeaderMode,
    link_name: Option<&Path>,
) -> io::Result<EntryReader> {
    let mut header = Header::new_gnu();

    let header_path = prepare_header_path_read(&mut header, path)?;
    header.set_metadata_in_mode(meta, mode);
    let header_link = if let Some(link_name) = link_name {
        prepare_header_link_read(&mut header, link_name)?
    } else {
        None
    };
    header.set_cksum();
    Ok(EntryReader {
        header,
        header_path: header_path.map(|(header, reader)| {
            Box::new(EntryReader {
                header,
                data: reader,
                header_path: None,
                header_link: None,
                header_done: false,
                data_done: false,
                data_read_byte_amount: 0,
                current_reader: None,
            }) as Box<dyn Read + Send>
        }),
        header_link: header_link.map(|(header, reader)| {
            Box::new(EntryReader {
                header,
                data: reader,
                header_path: None,
                header_link: None,
                header_done: false,
                data_done: false,
                data_read_byte_amount: 0,
                current_reader: None,
            }) as Box<dyn Read + Send>
        }),
        data: read,
        header_done: false,
        data_done: false,
        data_read_byte_amount: 0,
        current_reader: None,
    })
    //append(dst, &header, read)
}

fn prepare_header_path_read(
    header: &mut Header,
    path: &Path,
) -> io::Result<Option<(Header, Box<dyn Read + Send>)>> {
    // Try to encode the path directly in the header, but if it ends up not
    // working (probably because it's too long) then try to use the GNU-specific
    // long name extension by emitting an entry which indicates that it's the
    // filename.
    if let Err(e) = header.set_path(path) {
        let data = path2bytes(&path)?;
        let max = header.as_old().name.len();
        //  Since e isn't specific enough to let us know the path is indeed too
        //  long, verify it first before using the extension.
        if data.len() < max {
            return Err(e);
        }
        let header2 = prepare_header(data.len() as u64, b'L');
        // null-terminated string
        //let mut data2 = data.clone().into_owned().chain(io::repeat(0).take(1));
        let mut data2 = data.clone().into_owned();
        data2.push(0);
        let data2 = std::io::Cursor::new(data2);
        //append(dst, &header2, &mut data2)?;
        // Truncate the path to store in the header we're about to emit to
        // ensure we've got something at least mentioned.
        let path = bytes2path(Cow::Borrowed(&data[..max]))?;
        header.set_path(&path)?;
        Ok(Some((header2, Box::new(data2))))
    } else {
        Ok(None)
    }
}

fn prepare_header_link_read(
    header: &mut Header,
    link_name: &Path,
) -> io::Result<Option<(Header, Box<dyn Read + Send>)>> {
    // Same as previous function but for linkname
    if let Err(e) = header.set_link_name(&link_name) {
        let data = path2bytes(&link_name)?;
        if data.len() < header.as_old().linkname.len() {
            return Err(e);
        }
        let header2 = prepare_header(data.len() as u64, b'K');
        let mut data2 = data.clone().into_owned();
        data2.push(0);
        let data2 = std::io::Cursor::new(data2);
        Ok(Some((header2, Box::new(data2))))
    //append(dst, &header2, &mut data2)?;
    } else {
        Ok(None)
    }
}

struct EntryReader {
    data: Box<dyn Read + Send>,
    header: Header,
    header_path: Option<Box<dyn Read + Send>>,
    header_link: Option<Box<dyn Read + Send>>,
    header_done: bool,
    data_done: bool,
    data_read_byte_amount: u64,
    current_reader: Option<Box<dyn Read + Send>>,
}

impl Read for EntryReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut total_read_amount = 0;
        let mut to_read = buf;
        'outerloop: loop {
            if let Some(mut reader) = self.current_reader.take() {
                loop {
                    let read_amount = reader.read(to_read)?;
                    total_read_amount += read_amount;
                    if read_amount == 0 {
                        break;
                    }
                    to_read = &mut to_read[read_amount..];
                    if to_read.len() == 0 {
                        self.current_reader = Some(reader);
                        break 'outerloop;
                    }
                }
            } else {
                if let Some(reader) = self.header_path.take() {
                    self.current_reader = Some(reader);
                } else if let Some(reader) = self.header_link.take() {
                    self.current_reader = Some(reader);
                } else if !self.header_done {
                    self.current_reader = Some(Box::new(std::io::Cursor::new(
                        self.header
                            .as_bytes()
                            .into_iter()
                            .map(|e| *e)
                            .collect::<Vec<u8>>(),
                    )));
                    self.header_done = true;
                } else {
                    if !self.data_done {
                        loop {
                            let read_amount = self.data.read(to_read)?;
                            total_read_amount += read_amount;
                            self.data_read_byte_amount += read_amount as u64;
                            if read_amount == 0 {
                                break;
                            }
                            to_read = &mut to_read[read_amount..];
                            if to_read.len() == 0 {
                                break 'outerloop;
                            }
                        }
                        self.data_done = true;
                        //TODO: padded reader
                        let remaining_byte_padding =
                            (512 - (self.data_read_byte_amount % 512) as usize) % 512;
                        if remaining_byte_padding > 0 {
                            self.current_reader = Some(Box::new(std::io::Cursor::new(vec![
                                0;
                                remaining_byte_padding
                            ])));
                        }
                    } else {
                        break 'outerloop;
                    }
                }
            }
        }
        Ok(total_read_amount)
    }
}
