use crate::{InternalError, Node, Progress, RecvMsg, VfsDriver, VfsDriverType};
use log::error;
use std::fs::File;
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[derive(Debug)]
pub struct ZipFs {
    data: Option<ZipArchive<Cursor<Box<[u8]>>>>,
}

impl ZipFs {
    pub fn new() -> ZipFs {
        ZipFs { data: None }
    }
}

impl VfsDriver for ZipFs {
    /// This indicates that the file system is remote (such as ftp, https) and has no local path
    fn is_remote(&self) -> bool {
        false
    }

    /// If the driver supports a certain url
    fn supports_url(&self, url: &str) -> bool {
        // we don't support url style paths like ftp:// http:// etc.
        !url.contains(":/")
    }

    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_instance(&self) -> Box<dyn VfsDriver> {
        Box::new(ZipFs::new())
    }

    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_data(&self, data: &[u8]) -> bool {
        let c = std::io::Cursor::new(data);
        ZipArchive::new(c).is_ok()
    }

    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_from_data(&self, data: Box<[u8]>) -> Option<VfsDriverType> {
        let a = match ZipArchive::new(std::io::Cursor::new(data)) {
            Ok(a) => a,
            Err(e) => {
                error!("ZipFs Error: {:}", e);
                return None;
            }
        };

        Some(Box::new(ZipFs { data: Some(a) }))
    }

    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_url(&self, url: &str) -> bool {
        if std::fs::metadata(url).is_err() {
            return false;
        }

        let read_file = match File::open(url) {
            Ok(f) => f,
            Err(e) => {
                error!("ZipFs File Error: {:}", e);
                return false;
            }
        };

        zip::ZipArchive::new(read_file).is_ok()
    }

    /// Used when creating an instance of the driver with a path to load from
    fn create_from_url(&self, url: &str) -> Option<VfsDriverType> {
        let mut read_file = match File::open(url) {
            Ok(f) => f,
            Err(e) => {
                error!("ZipFs File Error: {:}", e);
                return None;
            }
        };

        let mut data = Vec::new();
        read_file.read_to_end(&mut data).unwrap();

        self.create_from_data(data.into_boxed_slice())
    }

    /// Returns a handle which updates the progress and returns the loaded data. This will try to
    fn load_url(&mut self, path: &str, progress: &Progress) -> Result<RecvMsg, InternalError> {
        let archive = self.data.as_mut().unwrap();

        let mut file = match archive.by_name(path) {
            Err(_) => return Err(InternalError::FileDirNotFound),
            Ok(f) => f,
        };

        let len = file.size() as usize;
        let mut output_data = vec![0u8; len];

        // if file is small than 10k we just unpack it directly without progress
        if len < 10 * 1024 {
            progress.update(0.0);
            file.read_to_end(&mut output_data)?;
        } else {
            // above 10k we read in 10 chunks
            let loop_count = 10;
            let block_len = len / loop_count;
            let mut percent = 0.0;
            let percent_step = 1.0 / loop_count as f32;
            let mut total_read_size = 0;

            for i in 0..loop_count + 1 {
                let block_offset = i * block_len;
                let read_amount = usize::min(len - block_offset, block_len);
                file.read_exact(&mut output_data[block_offset..block_offset + read_amount])?;
                total_read_size += read_amount;
                progress.update(percent);
                percent += percent_step;
            }
        }

        progress.update(1.0);
        Ok(RecvMsg::ReadDone(output_data.into_boxed_slice()))
    }

    // get a file/directory listing for the driver
    fn get_directory_list(&self, _path: &str) -> Vec<Node> {
        Vec::new()
    }
}
