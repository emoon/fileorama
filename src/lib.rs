use crossbeam_channel::{bounded, unbounded};
use log::*;
use std::sync::Mutex;
use thiserror::Error;
use zip::ZipArchive;

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, current};
use std::time::Duration;

mod local_fs;
mod zip_fs;

pub enum RecvMsg {
    ReadProgress(f32),
    ReadDone(Box<[u8]>),
    Error(VfsError),
    IsDirectory(usize),
}

#[derive(Error, Debug)]
pub enum InternalError {
    #[error("File Error)")]
    FileDirNotFound,
    #[error("File Error)")]
    FileError(#[from] std::io::Error),
    #[error("Send Error")]
    SendError(#[from] crossbeam_channel::SendError<RecvMsg>),
}

#[derive(Error, Debug)]
pub enum VfsError {
    /// Errors from std::io::Error
    #[error("File Error)")]
    FileError(#[from] std::io::Error),
}

/// File system implementations must implement this trait
pub trait VfsDriver: std::fmt::Debug {
    /// This indicates that the file system is remote (such as ftp, https) and has no local path
    fn is_remote(&self) -> bool;
    /// If the driver supports a certain url
    fn supports_url(&self, url: &str) -> bool;
    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_instance(&self) -> Box<dyn VfsDriver>;
    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_data(&self, data: &[u8]) -> bool;
    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_from_data(&self, data: Box<[u8]>) -> Option<VfsDriverType>;
    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_url(&self, url: &str) -> bool;
    /// Used when creating an instance of the driver with a path to load from
    fn create_from_url(&self, url: &str) -> Option<VfsDriverType>;
    /// Returns a handle which updates the progress and returns the loaded data. This will try to
    fn load_url(
        &mut self,
        path: &str,
        msg: &crossbeam_channel::Sender<RecvMsg>,
    ) -> Result<RecvMsg, InternalError>;

    // get a file/directory listing for the driver
    fn get_directory_list(&self, path: &str) -> Vec<Node>;
}

pub struct Handle {
    pub recv: crossbeam_channel::Receiver<RecvMsg>,
}

pub enum VfsType {
    // Used for remote loading (such as ftp, http)
    Remote,
    //
    Streaming,
}

pub enum NodeType {
    Unknown,
    File,
    Directory,
    Archive,
    Other(usize),
}

impl Default for NodeType {
    fn default() -> Self {
        NodeType::Unknown
    }
}

// TODO: Move bunch of data out to arrays to reduce reallocs
#[derive(Default)]
pub struct Node {
    node_type: NodeType,
    name: String,
    driver_index: u32,
    parent: u32,
    nodes: Vec<u32>,
}

impl Node {
    fn new_directory_node(name: &str) -> Node {
        Node {
            node_type: NodeType::Directory,
            name: name.to_owned(),
            ..Default::default()
        }
    }

    fn new_file_node(name: &str) -> Node {
        Node {
            node_type: NodeType::File,
            name: name.to_owned(),
            ..Default::default()
        }
    }
}

type VfsDriverType = Box<dyn VfsDriver>;

#[derive(Default)]
struct VfsState {
    nodes: Vec<Node>,
    node_drivers: Vec<VfsDriverType>,
    drivers: Vec<VfsDriverType>,
}

impl VfsState {
    fn new() -> VfsState {
        let drivers: Vec<VfsDriverType> = vec![
            Box::new(zip_fs::ZipFs::new()),
            Box::new(local_fs::LocalFs::new()),
        ];

        VfsState {
            drivers,
            nodes: vec![Node::new_directory_node("root")],
            ..Default::default()
        }
    }
}

pub struct Vfs {
    //state: Arc<Mutex<VfsState>>,
    /// for sending messages to the main-thread
    main_send: crossbeam_channel::Sender<SendMsg>,
}

//pub type VfsInstance = Box<dyn VfsDriver>;

impl Vfs {
    /// Loads the first file given a url. If an known archive is encounterd the first file will be extracted
    /// (given if it has a file listing) and that will be returned until a file is encountered. If there are
    /// no files an error will/archive will be returned instead and the user code has to handle it
    pub fn load_url(&self, path: &str) -> Handle {
        let (thread_send, main_recv) = unbounded::<RecvMsg>();

        self.main_send
            .send(SendMsg::LoadUrl(path.into(), 0, thread_send))
            .unwrap();

        Handle { recv: main_recv }
    }
}

pub enum SendMsg {
    LoadUrl(String, u32, crossbeam_channel::Sender<RecvMsg>),
}

fn handle_error(e: InternalError, msg: &crossbeam_channel::Sender<RecvMsg>) {
    if let InternalError::FileError(e) = e {
        let file_error = format!("{:#?}", e);
        if let Err(send_err) = msg.send(RecvMsg::Error(e.into())) {
            error!(
                "evfs: Unable to send file error {:#?} to main thread due to {:#?}",
                file_error, send_err
            );
        }
    }
}

fn find_driver_for_path(drivers: &[VfsDriverType], path: &str) -> Option<VfsDriverType> {
    for d in drivers {
        if d.supports_url(path) {
            // Check if we can
            if d.can_load_from_url(path) {
                if let Some(new_driver) = d.create_from_url(path) {
                    return Some(new_driver);
                }
            }
        }
    }

    None
}

// TODO: uses a hashmap instead?
fn find_entry_in_node(node: &Node, nodes: &[Node], name: &str) -> Option<usize> {
    for n in &node.nodes {
        let index = *n as usize;
        let t = &nodes[index];
        if t.name == name {
            return Some(index);
        }
    }

    None
}

fn get_component_name<'a>(component: &'a Component, had_prefix: &mut bool) -> Cow<'a, str> {
    match component {
        Component::RootDir => {
            if !*had_prefix {
                "/".into()
            } else {
                "".into()
            }
        }

        Component::Prefix(t) => {
            *had_prefix = true;
            t.as_os_str().to_string_lossy()
        }
        Component::Normal(t) => t.to_string_lossy(),
        e => unimplemented!("{:?}", e),
    }
}

// Add a new node to the vfs at a specific index and get the new node index back
fn add_new_node(state: &mut VfsState, index: usize, new_node: Node) -> usize {
    //let mut node = &state.nodes[index];
    let new_index = state.nodes.len();
    state.nodes.push(new_node);
    state.nodes[index].nodes.push(new_index as u32);
    new_index
}

#[derive(PartialEq)]
enum LoadState {
    FindNode,
    FindDriverUrl,
    FindDriverData,
    LoadFromDriver,
}

/// Loading of urls works in the following way:
/// 1. First we start with the full urls for example: ftp://dir/foo.zip/test/bar.mod
/// 2. We try to load the url as is. In this case it will fail as only ftp://dir/foo.zip is present on the ftp server
/// 3. We now search backwards until we get get a file loaded (in this case ftp://dir/foo.zip)
/// 4. As we haven't fully resolved the full path yet, we will from this point search backwards again again with foo.zip
///    trying to resolve "foo.zip/test/bar.mod" which should succede in this case.
///    We repeat this process until everthing we are done.
pub(crate) fn load(
    vfs: &mut VfsState,
    path: &str,
    msg: &crossbeam_channel::Sender<RecvMsg>,
) -> bool {
    let mut had_prefix = false;
    let mut node_index = 0;

    // temp
    //let (main_send, thread_recv) = unbounded::<SendMsg>();

    let components: Vec<Component> = Path::new(path).components().collect();
    let comp_len = components.len();

    let mut i = 0;
    let mut driver_index = None;
    let mut state = LoadState::FindNode;
    let mut data: Option<Box<[u8]>> = None;

    loop {
        match state {
            // Search for node in the vfs
            LoadState::FindNode => {
                // Search for node in the vfs
                for (t, c) in components[i..].iter().enumerate() {
                    let node = &vfs.nodes[node_index];
                    let component_name = get_component_name(&c, &mut had_prefix);
                    if let Some(entry) = find_entry_in_node(node, &vfs.nodes, &component_name) {
                        //println!("found entry! {}: {}", component_name, entry);
                        node_index = entry;
                        i += 1;
                        // If we don't have any driver yet and we didn't find a path we must search for a driver
                    } else if driver_index.is_none() {
                        state = LoadState::FindDriverUrl;
                    } else {
                        state = LoadState::LoadFromDriver;
                    }
                }
            }

            // Walk the url backwards and try to find a driver
            LoadState::FindDriverUrl => {
                let mut p: PathBuf = components[i..].iter().collect();

                let mut current_path = p.to_str().unwrap().to_owned();
                let mut level = 0;
                let mut driver = None;

                println!("current path {}", current_path);

                while !current_path.is_empty() {
                    driver = find_driver_for_path(&vfs.drivers, &current_path);

                    println!("driver for path {} {}", current_path, driver.is_some());

                    if driver.is_some() {
                        break;
                    }

                    p.pop();
                    current_path = p.to_str().unwrap().to_owned();
                    level += 1;
                }

                if let Some(d) = driver {
                    // If we found a driver we mount it inside the vfs
                    let mut current_index = node_index;
                    let mut prefix = false;
                    let d_index = vfs.node_drivers.len();

                    driver_index = Some(d_index);
                    vfs.node_drivers.push(d);

                    let comp = p.components();

                    for c in comp {
                        let new_node = Node {
                            node_type: NodeType::Unknown,
                            parent: current_index as _,
                            driver_index: d_index as u32,
                            name: get_component_name(&c, &mut prefix).to_string(),
                            ..Default::default()
                        };

                        current_index = add_new_node(vfs, current_index, new_node);
                    }

                    // Start from the index where the new driver has been mounted
                    node_index = current_index;
                    i += comp_len - level;

                    state = LoadState::LoadFromDriver;
                } else {
                    // Unable to find a driver to load
                    return false;
                }
            }

            LoadState::FindDriverData => {
                let node_data = data.as_ref().unwrap();
                let mut found_driver = true;

                for d in &vfs.drivers {
                    if !d.can_load_from_data(node_data) {
                        continue;
                    }

                    // TODO: Fix this clone
                    // Found a driver for this data. Updated the node index with the new driver
                    // and switch state to load that from the new driver
                    if let Some(new_driver) = d.create_from_data(node_data.clone()) {
                        let d_index = vfs.node_drivers.len();
                        driver_index = Some(d_index);
                        vfs.node_drivers.push(new_driver);
                        vfs.nodes[node_index].driver_index = d_index as _;
                        state = LoadState::LoadFromDriver;
                        found_driver = true;
                        break;
                    }
                }

                // No driver found data. So we just send it back here
                // TODO: Implement scanning here
                if !found_driver {
                    msg.send(RecvMsg::ReadDone(node_data.clone())).unwrap();
                    return true;
                }
            }

            LoadState::LoadFromDriver => {
                let mut p: PathBuf = components[i..].iter().collect();
                let mut current_path = p.to_str().unwrap().to_owned();
                let d_index = driver_index.unwrap();
                let d = &mut vfs.node_drivers[d_index];

                println!(">> Lodaing with driver {}", d_index);

                // walkbackwards from the current path and try to load the data

                let mut p: PathBuf = components[i..].iter().collect();

                let mut current_path = p.to_str().unwrap().to_owned();
                let mut level = 0;

                println!("current path {}", current_path);

                while !current_path.is_empty() {
                    match d.load_url(&current_path, msg) {
                        Err(e) => {
                            println!("{:?}", e);
                            //return false;
                        }

                        Ok(load_msg) => {
                            match load_msg {
                                RecvMsg::IsDirectory(_) => {
                                    println!("path is dir {}", current_path);
                                    // TODO: Insert into tree
                                    return true;
                                }

                                RecvMsg::ReadDone(in_data) => {
                                    println!("Read data to memory: {}", current_path);
                                    // if level is 0 then we are done, otherwise we have to continue
                                    // TODO: If user has "scan" on data we need to continue here as well
                                    if level == 0 {
                                        // TODO: Handle error
                                        msg.send(RecvMsg::ReadDone(in_data)).unwrap();
                                        return true;
                                    } else {
                                        let comp = p.components();
                                        let mut current_index = node_index;
                                        let mut prefix = false;
                                        let mut count = 0;

                                        for c in comp {
                                            let new_node = Node {
                                                node_type: NodeType::Unknown,
                                                parent: current_index as _,
                                                driver_index: d_index as u32,
                                                name: get_component_name(&c, &mut prefix)
                                                    .to_string(),
                                                ..Default::default()
                                            };

                                            current_index =
                                                add_new_node(vfs, current_index, new_node);
                                            count += 1;
                                        }

                                        node_index = current_index;
                                        i += count;

                                        // Add new nodes to the vfs
                                        data = Some(in_data);
                                        state = LoadState::FindDriverData;
                                        break;
                                    }
                                }

                                _ => (),
                            }
                        }
                    }

                    p.pop();
                    current_path = p.to_str().unwrap().to_owned();
                    level += 1;
                }

                if current_path.is_empty() && state == LoadState::LoadFromDriver {
                    println!("Unable to find url that was searched for");
                    return false;
                }

                //return true;
            }
        }
    }

    true
}

fn handle_msg(vfs: &mut VfsState, _name: &str, msg: &SendMsg) {
    match msg {
        SendMsg::LoadUrl(path, _node_index, msg) => {
            let p = Path::new(path);

            load(vfs, path, msg);
            /*

            // TODO: Support parsing the url in sub-chunks i.e mydir/foo.zip/dir/foo.rar/file
            for d in &drivers {
                // Found a loader that works
                let loader = d.sreate_instance();

                match loader.load_url(path, msg) {
                    Err(e) => {
                        handle_error(e, msg);
                        return;
                    }

                    msg.send(rec_msg).unwrap(),
                }
            }
            */
        }
    }
}

impl Vfs {
    //pub fn new(vfs_drivers: Option<&[Box<dyn VfsDriver>]>) -> Vfs {
    pub fn new() -> Vfs {
        let (main_send, thread_recv) = unbounded::<SendMsg>();

        //let state = Arc::new(Mutex::new(vfs_state));
        let thread_recv_0 = thread_recv.clone();

        // Setup worker thread
        thread::Builder::new()
            .name("vfs_worker".to_string())
            .spawn(move || {
                let mut state = VfsState::new();

                while let Ok(msg) = thread_recv_0.recv() {
                    handle_msg(&mut state, "vfs_worker", &msg);
                }
            })
            .unwrap();

        Vfs { main_send }
    }
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /*
    fn print_tree(state: &VfsState, index: u32, parent: u32, indent: usize) {
        let node = &state.nodes[index as usize];

        println!(
            "{:indent$} {} driver {}",
            "",
            node.name,
            node.driver_index,
            indent = indent
        );

        for n in &node.nodes {
            print_tree(state, *n, node.parent, indent + 1);
        }
    }
     */

    /*
    #[test]
    fn vfs_state_load_local() {
        let mut state = VfsState::new();
        let path = std::fs::canonicalize("Cargo.toml").unwrap();
        load(&mut state, &path.to_string_lossy());

        let path = std::fs::canonicalize("Cargo.lock").unwrap();
        load(&mut state, &path.to_string_lossy());

        print_tree(&state, 0, 0, 0);
    }
    */

    #[test]
    fn vfs_state_load_zip() {
        let mut path = std::fs::canonicalize("data/a.zip").unwrap();
        path = path.join("beat.zip/foo/6beat.mod");

        let vfs = Vfs::new();
        let handle = vfs.load_url(&path.to_string_lossy());

        match handle.recv.recv() {
            Ok(msg) => match msg {
                RecvMsg::ReadDone(data) => {
                    println!("Got data {} ", data.len());
                }
                RecvMsg::ReadProgress(_) => todo!(),
                RecvMsg::Error(_) => todo!(),
                RecvMsg::IsDirectory(_) => todo!(),
            },
            Err(e) => panic!("{:?}", e),
        }

        //load(&mut state, &path.to_string_lossy());
        //print_tree(&state, 0, 0, 0);
    }

    /*
    #[test]
    fn check_directory() {
        let vfs = Vfs::new();

        let handle = vfs.load_url("Cargo.toml");

        let mut found_dir = false;

        for _ in 0..10 {
            if let Ok(data) = handle.recv.try_recv() {
                match data {
                    RecvMsg::IsDirectory(_) => found_dir = true,
                    RecvMsg::Error(e) => panic!("{:?}", e),
                    _ => (),
                }
            }

            thread::sleep(Duration::from_millis(10));
        }

        assert!(found_dir);
    }
    */

    /*
    #[test]
    fn test_get_directory_ftp() {
        let vfs = Vfs::new(None);
        let mut got_archive = false;
        let handle = vfs.load_url("ftp://ftp.modland.com/pub/modules");

        for _ in 0..10 {
            match handle.recv.try_recv() {
                Ok(data) => match data {
                    RecvMsg::Archive(_) => got_archive = true,
                },
                _ => (),
            }

            thread::sleep(time::Duration::from_millis(200));
        }

        assert_eq!(got_archive, true);
    }
    */
}
