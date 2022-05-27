use crate::{InternalError, LoadStatus, Progress, VfsDriver, VfsDriverType, FilesDirs};
use ftp::{FtpError, FtpStream};
use log::error;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read};

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
        url.starts_with("ftp:/") || url.starts_with("ftp.")
    }

    /// Used when creating an instance of the driver with a path to load from
    fn create_from_url(&self, url: &str) -> Option<VfsDriverType> {
        if let Some(url) = Self::find_server_name("ftp.modland.com:21") {
            dbg!("Connecting to {}", url);
            dbg!();

            let mut stream = match FtpStream::connect(url) {
                Ok(stream) => stream,
                Err(e) => {
                    dbg!();
                    panic!("Unable to connect to {:?}", e);
                    return None;
                }
            };

            dbg!(&stream);

            stream.login("anonymous", "anonymous").unwrap();

            dbg!();

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
        let path = "readme_welcome.txt";

        dbg!(path);

        //let mut buf = Vec::new();

        /*
        conn.retr(path, |stream| {
            stream.read_to_end(&mut buf).map_err(|e| FtpError::ConnectionError(e))
        })?;
        */
        let cursor = conn.simple_retr(path)?;
        Ok(LoadStatus::Data(cursor.into_inner().into_boxed_slice()))


        /*
        let archive = self.data.as_mut().unwrap();

        if path.is_empty() {
            return Ok(LoadStatus::Directory);
        }

        let mut file = match archive.by_name(path) {
            Err(_) => return Ok(LoadStatus::NotFound),
            Ok(f) => f,
        };

        if file.is_dir() {
            return Ok(LoadStatus::Directory);
        }

        let len = file.size() as usize;
        let mut output_data = vec![0u8; len];

        // if file is small than 10k we just unpack it directly without progress
        if len < 10 * 1024 {
            progress.set_step(1);
            file.read_to_end(&mut output_data)?;
            progress.step()?;
        } else {
            // above 10k we read in 10 chunks
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
        */

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
