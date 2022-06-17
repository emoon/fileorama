use crate::{InternalError, LoadStatus, Progress, VfsDriver, VfsDriverType, FilesDirs};
use std::{fs::File, io::Read, path::PathBuf};
use walkdir::WalkDir;

#[cfg(not(test))]
use log::{trace};

#[cfg(test)]
use std::{println as trace};

#[derive(Debug)]
pub struct LocalFs {
    pub root: PathBuf,
}

impl LocalFs {
    pub fn new() -> LocalFs {
        LocalFs {
            root: PathBuf::new(),
        }
    }
}

impl VfsDriver for LocalFs {
    fn is_remote(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "local_fs"
    }

    // Supports loading all file extensions
    fn supports_url(&self, path: &str) -> bool {
        trace!("supports_url: {}", path);
        // we don't support url style paths like ftp:// http:// etc.
        !path.contains(":/")
    }
    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_url(&self, url: &str) -> bool {
        let t = std::fs::metadata(url).is_ok();
        trace!("Can load from path {} : {}", url, t);
        t
    }

    // Local filesystem can't be loaded from data
    fn can_load_from_data(&self, _data: &[u8]) -> bool {
        false
    }

    fn create_instance(&self) -> VfsDriverType {
        Box::new(LocalFs { root: "".into() })
    }

    fn create_from_url(&self, path: &str) -> Option<VfsDriverType> {
        trace!("Created driver at {}", path);
        Some(Box::new(LocalFs { root: path.into() }))
    }

    // As above function will always return true this will never be called
    fn create_from_data(&self, _data: Box<[u8]>) -> Option<VfsDriverType> {
        None
    }

    /// Read a file from the local filesystem.
    fn load_url(
        &mut self,
        path: &str,
        progress: &mut Progress,
    ) -> Result<LoadStatus, InternalError> {
        let path = if path.is_empty() {
            self.root.clone()
        } else {
            self.root.join(path)
        };

        trace!("trying loading from {:?}", path);

        let metadata = std::fs::metadata(&path)?;

        if metadata.is_dir() {
            trace!("load_url: {:?} is a directory", path);
            return Ok(LoadStatus::Directory);
        }

        let mut output_data = Vec::new();
        let len = metadata.len() as usize;
        let mut file = File::open(&path)?;

        // if file is small than 5 meg we just load it fully directly to memory
        if len < 5 * 1024 * 1024 {
            progress.set_step(1);
            file.read_to_end(&mut output_data)?;
            progress.step()?;
        } else {
            output_data = vec![0u8; len];
            // above 5 meg we read in 10 chunks
            let loop_count = 10;
            let block_len = len / loop_count;
            progress.set_step(loop_count);

            for i in 0..loop_count + 1 {
                let block_offset = i * block_len;
                let read_amount = usize::min(len - block_offset, block_len);
                file.read_exact(&mut output_data[block_offset..block_offset + read_amount])?;
                progress.step()?;
            }
        }

        trace!("load_url: Loaded file {:?} to memory", path);

        Ok(LoadStatus::Data(output_data.into_boxed_slice()))
    }

    fn get_directory_list(
        &mut self,
        path: &str,
        progress: &mut Progress,
    ) -> Result<FilesDirs, InternalError> {
        progress.set_step(1);
        let mut dirs = Vec::with_capacity(256);
        let mut files = Vec::with_capacity(256);

        trace!("Getting directory listing for {:?}", &self.root.join(path));

        // skip 1 skips the inital directory as it's included otherwise
        for e in WalkDir::new(self.root.join(path)).max_depth(1).into_iter().skip(1) {
            let file = e?;
            let metadata = file.metadata()?;

            if let Some(filename) = file.path().file_name() {
                let name = filename.to_string_lossy().into();
                if metadata.is_file() {
                    files.push(name);
                } else {
                    dirs.push(name);
                }
            }
        }

        progress.step()?;

        dirs.sort();
        files.sort();

        Ok(FilesDirs::new(files, dirs))
    }
}
