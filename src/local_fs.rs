use crate::{InternalError, Node, Progress, RecvMsg, VfsDriver, VfsDriverType, VfsError};
use log::trace;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct LocalFs {
    root: PathBuf,
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

    // Supports loading all file extensions
    fn supports_url(&self, path: &str) -> bool {
        // we don't support url style paths like ftp:// http:// etc.
        !path.contains(":/")
    }
    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_url(&self, url: &str) -> bool {
        std::fs::metadata(url).is_ok()
    }

    // Local filesystem can't be loaded from data
    fn can_load_from_data(&self, _data: &[u8]) -> bool {
        false
    }

    fn create_instance(&self) -> Box<dyn VfsDriver> {
        Box::new(LocalFs { root: "".into() })
    }

    fn create_from_url(&self, path: &str) -> Option<VfsDriverType> {
        println!("LocalFs {}", path);

        Some(Box::new(LocalFs { root: path.into() }))
    }

    // As above function will always return true this will never be called
    fn create_from_data(&self, _data: Box<[u8]>) -> Option<VfsDriverType> {
        None
    }

    /// Read a file from the local filesystem.
    fn load_url(&mut self, path: &str, progress: &mut Progress) -> Result<RecvMsg, InternalError> {
        let path = self.root.join(path);

        let metadata = std::fs::metadata(&path)?;

        if metadata.is_dir() {
            return Ok(RecvMsg::IsDirectory(0));
        }

        let len = metadata.len() as usize;
        let mut file = File::open(&path)?;
        let mut output_data = vec![0u8; len];

        trace!("vfs: reading from {:#?}", path);

        // if file is small than 5 meg we just load it fully directly to memory
        if len < 5 * 1024 * 1024 {
            progress.set_step(1);
            file.read_to_end(&mut output_data)?;
            progress.step()?;
        } else {
            // above 5 meg we read in 10 chunks
            let loop_count = 10;
            let block_len = len / loop_count;
            let percent_step = 1.0 / loop_count as f32;

            for i in 0..loop_count {
                let block_offset = i * block_len;
                let read_amount = usize::min(len - block_offset, block_len);
                file.read_exact(&mut output_data[block_offset..block_offset + read_amount])?;
                progress.step();
            }
        }

        Ok(RecvMsg::ReadDone(output_data.into_boxed_slice()))
    }

    // get a file listing for the driver
    fn get_directory_list(&self, path: &str) -> Vec<Node> {
        let files: Vec<Node> = WalkDir::new(self.root.join(path))
            .max_depth(1)
            .into_iter()
            .filter_map(|e| {
                let file = e.unwrap();
                let metadata = file.metadata().unwrap();

                if let Some(filename) = file.path().file_name() {
                    let name = filename.to_str().unwrap();
                    if metadata.is_dir() {
                        return Some(Node::new_directory_node(name));
                    } else {
                        return Some(Node::new_file_node(name));
                    }
                }

                None
            })
            .collect();
        files
    }
}
