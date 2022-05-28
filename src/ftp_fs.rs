use crate::{InternalError, LoadStatus, Progress, VfsDriver, VfsDriverType, FilesDirs};
use ftp::{FtpError, FtpStream};
use log::error;
//use std::collections::HashSet;
//use std::fs::File;
//use std::io::{Cursor, Read, Write};

#[derive(Debug)]
pub struct FtpFs {
    data: Option<FtpStream>,
}

impl FtpFs {
    pub fn new() -> FtpFs {
        FtpFs { data: None }
    }
}

impl FtpFs {
    fn find_server_name(url: &str) -> Option<&str> {
        if let Some(url) = url.strip_prefix("ftp:/") {
            // handle if we have name and no path
            if !url.contains('/') {
                return Some(url);
            }

            if let Some(offset) = url.find('/') {
                return Some(&url[..offset]);
            }

        } else {
            if !url.contains('/') {
                return Some(url);
            }

            if let Some(offset) = url.find('/') {
                return Some(&url[..offset]);
            }
        } 

        None
    }
}

impl VfsDriver for FtpFs {
    /// This indicates that the file system is remote (such as ftp, https) and has no local path
    fn is_remote(&self) -> bool {
        true
    }

    /// If the driver supports a certain url
    fn supports_url(&self, url: &str) -> bool {
        // Only supports urls that starts with ftp 
        url.starts_with("ftp:/") || url.starts_with("ftp.")
    }

    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_instance(&self) -> VfsDriverType {
        Box::new(FtpFs::new())
    }

    // FtpFs doesn't to create from data
    fn can_load_from_data(&self, _data: &[u8]) -> bool {
        false
    }

    // Create a new instance given data
    fn create_from_data(&self, _data: Box<[u8]>) -> Option<VfsDriverType> {
        None
    }

    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_url(&self, url: &str) -> bool {
        // Only supports urls that starts with ftp 
        if !url.starts_with("ftp:/") && !url.starts_with("ftp.") {
            return false;
        }

        // Make sure we don't have any slashes in the path except ftp start
        if let Some(url) = url.strip_prefix("ftp:/") {
            !url.contains('/')
        } else {
            !url.contains('/')
        }
    }

    /// Used when creating an instance of the driver with a path to load from
    fn create_from_url(&self, url: &str) -> Option<VfsDriverType> {
        if let Some(url) = Self::find_server_name(url) {
            let url_with_port = if url.contains(":") {
                url.to_owned()
            } else {
                format!("{}:21", url)
            };

            let mut stream = match FtpStream::connect(url_with_port) {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Unable to connect to {:?}", e);
                    return None;
                }
            };

            stream.login("anonymous", "anonymous").unwrap();
            stream.transfer_type(ftp::types::FileType::Binary).unwrap();

            return Some(Box::new(FtpFs { data: Some(stream) }))
        }

        None
    }

    /// Returns a handle which updates the progress and returns the loaded data. This will try to
    fn load_url(
        &mut self,
        path: &str,
        progress: &mut Progress,
    ) -> Result<LoadStatus, InternalError> {
        let conn = self.data.as_mut().unwrap();

        if let Some(file_size) = conn.size(path)? {
            //let cursor = conn.simple_retr(path)?;
            //Ok(LoadStatus::Data(cursor.into_inner().into_boxed_slice()))

            let block_len = 64 * 1024;
            let loop_count = file_size / block_len;
            progress.set_step(loop_count);

            let output_data = conn.retr(path, |reader| {
                let mut output_data = vec![0u8; file_size];
                let mut pro = progress.clone();

                for i in 0..loop_count + 1 {
                    let block_offset = i * block_len;
                    let read_amount = usize::min(file_size - block_offset, block_len);
                    reader.read_exact(&mut output_data[block_offset..block_offset + read_amount]).map_err(FtpError::ConnectionError)?;
                    pro.step().map_err(|op| FtpError::InvalidResponse(op.to_string()))?;
                }

                Ok(output_data)

                })?;

                Ok(LoadStatus::Data(output_data.into_boxed_slice()))

        } else {
            // if we didn't get any size here we assume it's a directory.
            Ok(LoadStatus::Directory)
        }

        //Ok(LoadStatus::Data(buf.into_boxed_slice()))
    }

    fn get_directory_list(
        &self,
        path: &str,
        progress: &mut Progress,
    ) -> Result<FilesDirs, InternalError> {
        Ok(FilesDirs::default())
    }
}
